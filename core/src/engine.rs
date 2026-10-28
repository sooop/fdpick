//! 질의 엔진: UI 의 `query_changed` 를 받아 fd 실행, 매칭, 경로 탐색을 조율하고
//! 결과를 싱크(콜백)로 내보낸다. UI 프레임워크에 의존하지 않는다.
//!
//! 스레드: fd 이벤트마다 포워더 1개(작업당), 결과 방출용 에미터 1개(상주).
//! 에미터는 매처/fd 가 알릴 때만 깨어나 40ms 로 합쳐서 방출한다(유휴 시 CPU 0).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::config::Config;
use crate::fd::{self, DriveTypes, FdConfig, FdEvent};
use crate::matcher::{Injector, Matcher};
use crate::pathnav::{self, NavKind};
use crate::query;
use crate::recents::{RecentHit, Recents};

// 경로 모드 목록의 최대 항목 수(설정 대상이 아닌 내부 상한)
const PATH_LIMIT: usize = 500;
// 경로 모드 입력 중에 목록 위에 붙이는 최근 항목 수
const PATH_RECENT_LIMIT: usize = 5;
const EMIT_COALESCE: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Fuzzy,
    Path,
    Recent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Idle,
    Running,
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResultItem {
    pub path: String,
    pub name_start: usize,
    pub indices: Vec<u32>,
    pub is_dir: bool,
    /// 최근 사용 목록에서 온 항목
    pub recent: bool,
    /// 경로가 더 이상 존재하지 않음(최근 항목만)
    pub missing: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResultsUpdated {
    pub seq: u64,
    pub mode: Mode,
    pub items: Vec<ResultItem>,
    pub matched: u32,
    pub scanned: u32,
    pub state: RunState,
    pub elapsed_ms: u64,
    pub message: Option<String>,
    pub cache_note: Option<String>,
}

/// 결과 전달 콜백. 엔진의 여러 스레드에서 호출되므로 가볍게 처리하고 UI 스레드로 넘길 것.
pub type Sink = Arc<dyn Fn(ResultsUpdated) + Send + Sync>;

struct State {
    result_limit: usize,
    empty_limit: usize,
    matcher: Matcher,
    mode: Mode,
    seq: u64,
    gen: u64,
    fd_key: Option<String>,
    job: Option<fd::FdJob>,
    run: RunState,
    started: Instant,
    finished_at: Option<Instant>,
    elapsed_ms: Option<u64>,
    perm_errors: usize,
    lossy: u64,
    error: Option<String>,
    dirty: bool,
}

/// 에미터를 깨우는 신호. nucleo 는 항목 1건마다 알리므로 이미 신호가 서 있으면
/// 락을 잡지 않고 바로 돌아간다.
struct Wake {
    pending: AtomicBool,
    lock: Mutex<()>,
    cv: Condvar,
}

impl Wake {
    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _g = self.lock.lock().unwrap();
            self.cv.notify_one();
        }
    }

    fn wait(&self) {
        let mut g = self.lock.lock().unwrap();
        while !self.pending.load(Ordering::Acquire) {
            g = self.cv.wait(g).unwrap();
        }
        self.pending.store(false, Ordering::Release);
    }
}

pub struct Engine {
    sink: Sink,
    cfg: FdConfig,
    cache_ttl: Duration,
    deny_top: Vec<String>,
    record_recent: bool,
    recents: Arc<Mutex<Recents>>,
    state: Arc<Mutex<State>>,
    wake: Arc<Wake>,
}

impl Engine {
    pub fn new(sink: Sink, config: &Config) -> Self {
        let wake = Arc::new(Wake { pending: AtomicBool::new(false), lock: Mutex::new(()), cv: Condvar::new() });
        let w = wake.clone();
        let matcher = Matcher::new(Arc::new(move || w.notify()));
        let state = Arc::new(Mutex::new(State {
            result_limit: config.result_limit,
            empty_limit: config.empty_limit,
            matcher,
            mode: Mode::Fuzzy,
            seq: 0,
            gen: 0,
            fd_key: None,
            job: None,
            run: RunState::Idle,
            started: Instant::now(),
            finished_at: None,
            elapsed_ms: None,
            perm_errors: 0,
            lossy: 0,
            error: None,
            dirty: false,
        }));
        let engine = Engine {
            sink,
            cfg: fd_config(config),
            cache_ttl: Duration::from_secs(config.cache_ttl_minutes * 60),
            deny_top: config.deny_top.clone(),
            record_recent: config.record_recent,
            recents: Arc::new(Mutex::new(Recents::load(Recents::default_path(), config.recent_limit))),
            state,
            wake,
        };
        engine.spawn_emitter();
        engine
    }

    /// 프런트의 입력 변경 진입점
    pub fn query_changed(&self, text: &str, mode: &str, force: bool) {
        if mode == "path" {
            self.path_query(text);
        } else if mode == "recent" {
            self.recent_query(text);
        } else {
            self.fuzzy_query(text, force);
        }
    }

    fn fuzzy_query(&self, text: &str, force: bool) {
        let parsed = query::split_query(text);
        let args = match query::tokenize_args(&parsed.fd_args).and_then(query::expand_search_paths) {
            Ok(a) => a,
            Err(e) => {
                let mut st = self.state.lock().unwrap();
                st.mode = Mode::Fuzzy;
                st.run = RunState::Error;
                st.error = Some(format!("fd 옵션 오류: {e}"));
                st.dirty = true;
                drop(st);
                self.wake.notify();
                return;
            }
        };
        let key = format!("{}|{:?}", parsed.use_defaults, args);
        let mut st = self.state.lock().unwrap();
        st.mode = Mode::Fuzzy;
        let expired = st.finished_at.map_or(false, |t| t.elapsed() > self.cache_ttl);
        let need_restart = force || st.fd_key.as_deref() != Some(key.as_str()) || expired;
        if need_restart {
            self.restart_fd(&mut st, args, parsed.use_defaults, key);
        } else if st.run == RunState::Error && st.job.is_none() {
            // 같은 옵션의 오류 상태는 그대로 보여준다(재시도는 Ctrl+R)
        }
        st.matcher.set_pattern(&parsed.pattern);
        st.dirty = true;
        drop(st);
        self.wake.notify();
    }

    fn restart_fd(&self, st: &mut State, args: Vec<String>, use_defaults: bool, key: String) {
        if let Some(job) = st.job.take() {
            job.cancel();
        }
        st.gen += 1;
        st.matcher.clear();
        st.run = RunState::Running;
        st.started = Instant::now();
        st.finished_at = None;
        st.elapsed_ms = None;
        st.perm_errors = 0;
        st.lossy = 0;
        st.error = None;
        st.fd_key = Some(key);

        let (tx, rx) = mpsc::channel();
        match fd::start(&self.cfg, args, use_defaults, tx) {
            Ok(job) => {
                st.job = Some(job);
                self.spawn_forwarder(st.gen, rx, st.matcher.injector());
            }
            Err(e) => {
                st.run = RunState::Error;
                st.error = Some(e);
            }
        }
    }

    fn spawn_forwarder(&self, gen: u64, rx: mpsc::Receiver<FdEvent>, injector: Injector) {
        let state = self.state.clone();
        let wake = self.wake.clone();
        thread::spawn(move || {
            for ev in rx {
                if let FdEvent::Paths(v) = ev {
                    injector.push_many(v);
                    continue;
                }
                let mut st = state.lock().unwrap();
                if st.gen != gen {
                    break;
                }
                match ev {
                    FdEvent::StderrSummary { permission_errors, .. } => {
                        st.perm_errors = permission_errors;
                    }
                    FdEvent::Done { exit_ok, elapsed_ms, lossy_count, .. } => {
                        st.run = if exit_ok { RunState::Done } else { RunState::Error };
                        if !exit_ok {
                            st.error = Some("fd 가 비정상 종료했습니다(일부 결과만 표시)".into());
                        }
                        st.elapsed_ms = Some(elapsed_ms);
                        st.finished_at = Some(Instant::now());
                        st.lossy = lossy_count;
                    }
                    FdEvent::Failed(msg) => {
                        st.run = RunState::Error;
                        st.error = Some(msg);
                        st.elapsed_ms = Some(st.started.elapsed().as_millis() as u64);
                    }
                    FdEvent::Paths(_) => {}
                }
                st.dirty = true;
                drop(st);
                wake.notify();
            }
        });
    }

    fn spawn_emitter(&self) {
        let state = self.state.clone();
        let wake = self.wake.clone();
        let sink = self.sink.clone();
        thread::spawn(move || loop {
            wake.wait();
            // 짧게 모아서 한 번만 방출
            thread::sleep(EMIT_COALESCE);
            let payload = {
                let mut st = state.lock().unwrap();
                if st.mode != Mode::Fuzzy {
                    continue;
                }
                let changed = st.matcher.tick(5);
                if !changed && !st.dirty {
                    continue;
                }
                st.dirty = false;
                Some(build_fuzzy_payload(&mut st))
            };
            if let Some(p) = payload {
                sink(p);
            }
        });
    }

    /// 복사/열기에 쓴 경로를 최근 사용 목록에 기록한다(설정으로 끌 수 있음).
    pub fn record_recent(&self, paths: &[String]) {
        if !self.record_recent {
            return;
        }
        self.recents.lock().unwrap().record(paths);
    }

    /// 최근 사용 목록(사용 이력)을 모두 지운다.
    pub fn clear_recents(&self) {
        self.recents.lock().unwrap().clear();
    }

    /// 최근 모드: 입력이 비면 최신순 전체, 있으면 퍼지 검색.
    fn recent_query(&self, text: &str) {
        let limit = self.state.lock().unwrap().result_limit;
        let hits = self.recents.lock().unwrap().search(&text.replace('/', "\\"), limit);
        let total = hits.len() as u32;
        let items: Vec<ResultItem> = hits.into_iter().map(recent_item).collect();
        let payload = {
            let mut st = self.state.lock().unwrap();
            st.mode = Mode::Recent;
            st.seq += 1;
            ResultsUpdated {
                seq: st.seq,
                mode: Mode::Recent,
                items,
                matched: total,
                scanned: total,
                state: RunState::Done,
                elapsed_ms: 0,
                message: None,
                cache_note: None,
            }
        };
        (self.sink)(payload);
    }

    fn path_query(&self, text: &str) {
        let nav = pathnav::parse_input(text);
        let mut error = None;
        let entries = match nav.mode {
            NavKind::Drives => Ok(pathnav::filter_entries(pathnav::list_drives(), &nav.filter)),
            NavKind::DriveRoot | NavKind::Dir => {
                let parent = nav.parent.clone().unwrap_or_default();
                let deny: Vec<&str> = self.deny_top.iter().map(String::as_str).collect();
                pathnav::list_dirs(&parent, &deny)
                    .map(|v| pathnav::filter_entries(v, &nav.filter))
            }
        };
        let entries = entries.unwrap_or_else(|e| {
            error = Some(format!("폴더를 열 수 없습니다: {e}"));
            Vec::new()
        });
        // 입력 중이면 최근 항목을 목록 위에 붙인다(폴더 목록과 겹치는 경로는 폴더 쪽에서 뺀다)
        let hits: Vec<RecentHit> = if text.trim().is_empty() {
            Vec::new()
        } else {
            self.recents
                .lock()
                .unwrap()
                .search(&text.replace('/', "\\"), PATH_RECENT_LIMIT)
        };
        let in_hits = |p: &str| hits.iter().any(|h| h.path.eq_ignore_ascii_case(p));
        let entries: Vec<_> = entries.into_iter().filter(|e| !in_hits(&e.path)).collect();
        let total = (entries.len() + hits.len()) as u32;
        let items: Vec<ResultItem> = hits
            .iter()
            .cloned()
            .map(recent_item)
            .chain(entries.iter().take(PATH_LIMIT).map(|e| nav_item(&e.path, &e.name, &nav.filter)))
            .collect();

        let payload = {
            let mut st = self.state.lock().unwrap();
            st.mode = Mode::Path;
            st.seq += 1;
            ResultsUpdated {
                seq: st.seq,
                mode: Mode::Path,
                items,
                matched: total,
                scanned: total,
                state: if error.is_some() { RunState::Error } else { RunState::Done },
                elapsed_ms: 0,
                message: error,
                cache_note: None,
            }
        };
        (self.sink)(payload);
    }
}

/// 설정을 fd 실행 설정으로 변환한다.
fn fd_config(c: &Config) -> FdConfig {
    let path = c.fd_path.trim();
    let has = |name: &str| c.drive_types.iter().any(|t| t.eq_ignore_ascii_case(name));
    FdConfig {
        exe: if path.is_empty() { None } else { Some(path.into()) },
        default_args: c.default_fd_args.clone(),
        deny_top: c.deny_top.clone(),
        drive_types: DriveTypes {
            fixed: has("Fixed"),
            removable: has("Removable"),
            network: has("Network"),
        },
    }
}

fn build_fuzzy_payload(st: &mut State) -> ResultsUpdated {
    let snap = st.matcher.snapshot(st.result_limit, st.empty_limit);
    let items = snap
        .items
        .into_iter()
        .map(|m| {
            let (path, is_dir, name_start) = clean_path(&m.path);
            ResultItem { path, name_start, indices: m.indices, is_dir, recent: false, missing: false }
        })
        .collect();
    st.seq += 1;

    let mut notes: Vec<String> = Vec::new();
    if st.perm_errors > 0 {
        notes.push(format!("접근 불가 {}건", st.perm_errors));
    }
    if st.lossy > 0 {
        notes.push(format!("잘못된 UTF-8 경로 {}건(손실 변환)", st.lossy));
    }
    let message = match (&st.error, notes.is_empty()) {
        (Some(e), true) => Some(e.clone()),
        (Some(e), false) => Some(format!("{e} · {}", notes.join(" · "))),
        (None, false) => Some(notes.join(" · ")),
        (None, true) => None,
    };
    let elapsed_ms = st.elapsed_ms.unwrap_or_else(|| st.started.elapsed().as_millis() as u64);
    ResultsUpdated {
        seq: st.seq,
        mode: Mode::Fuzzy,
        items,
        matched: snap.matched,
        scanned: snap.scanned,
        state: st.run,
        elapsed_ms,
        message,
        cache_note: None,
    }
}

/// fd 출력 경로를 표시용으로 정리한다: (정리된 경로, 디렉터리 여부, 파일명 시작 char 인덱스).
/// 끝 구분자는 제거하고(드라이브 루트 `C:\` 는 유지), indices 는 앞부분이 그대로라 바뀌지 않는다.
fn clean_path(raw: &str) -> (String, bool, usize) {
    let is_dir = raw.ends_with('\\') || raw.ends_with('/');
    let trimmed = raw.trim_end_matches(['\\', '/']);
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        return (format!("{trimmed}\\"), true, 0);
    }
    let path = if trimmed.is_empty() { raw.to_string() } else { trimmed.to_string() };
    let name_start = match path.rfind(['\\', '/']) {
        Some(i) => path[..=i].chars().count(),
        None => 0,
    };
    (path, is_dir, name_start)
}

/// 최근 사용 항목. 저장된 경로는 이미 정규화돼 있어 indices 가 그대로 유효하다.
fn recent_item(h: RecentHit) -> ResultItem {
    let (path, _, name_start) = clean_path(&h.path);
    ResultItem { path, name_start, indices: h.indices, is_dir: h.is_dir, recent: true, missing: !h.exists }
}

/// 경로 모드 항목. 필터와 일치한 이름 부분을 indices 로 표시한다.
fn nav_item(path: &str, name: &str, filter: &str) -> ResultItem {
    let name_start = if path.ends_with('\\') { 0 } else { path.chars().count() - name.chars().count() };
    let mut indices = Vec::new();
    if !filter.is_empty() {
        let lname = name.to_lowercase();
        let lfilter = filter.to_lowercase();
        // 소문자 변환으로 글자 수가 달라지는 드문 경우는 강조하지 않는다
        if lname.chars().count() == name.chars().count() {
            if let Some(byte_pos) = lname.find(&lfilter) {
                let start = lname[..byte_pos].chars().count();
                let len = lfilter.chars().count();
                indices = (start..start + len).map(|i| (name_start + i) as u32).collect();
            }
        }
    }
    ResultItem { path: path.to_string(), name_start, indices, is_dir: true, recent: false, missing: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_strips_trailing_separator() {
        assert_eq!(clean_path(r"C:\Users\yepben\"), (r"C:\Users\yepben".to_string(), true, 9));
        assert_eq!(clean_path(r"C:\a\b.txt"), (r"C:\a\b.txt".to_string(), false, 5));
        assert_eq!(clean_path(r"C:\"), (r"C:\".to_string(), true, 0));
        assert_eq!(clean_path(r"D:\한글\보고서.txt"), (r"D:\한글\보고서.txt".to_string(), false, 6));
    }

    #[test]
    fn nav_item_highlight_uses_char_indices() {
        let it = nav_item(r"D:\프로젝트\proj", "proj", "pro");
        assert_eq!(it.name_start, 8);
        assert_eq!(it.indices, vec![8, 9, 10]);
        let drive = nav_item(r"D:\", "D:", "d");
        assert_eq!(drive.name_start, 0);
    }
}
