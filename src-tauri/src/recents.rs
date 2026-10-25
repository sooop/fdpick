//! fdpick "최근 사용 경로" 저장소.
//!
//! 복사/열기 등으로 쓴 경로를 기록하고, 최신순 목록과 퍼지 검색을 제공한다.
//! 스레드/백그라운드 작업 없이 모두 동기로 동작한다(항목 최대 수천 개 규모 전제).
//!
//! 필요한 의존성: `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`,
//! `nucleo-matcher = "0.3"` (nucleo 0.5 가 쓰는 버전).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;

/// 저장 파일 형식 버전.
const FILE_VERSION: u64 = 1;

/// 목록/검색 결과 한 건.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentHit {
    pub path: String,
    /// path 안의 매치된 글자 위치(코드포인트 인덱스), 오름차순, 중복 없음. list 에서는 빈 Vec.
    pub indices: Vec<u32>,
    pub is_dir: bool,
    pub exists: bool,
}

/// 메모리 상의 항목. `Recents.items` 는 최신이 앞(index 0)이다.
#[derive(Clone, Debug)]
struct Item {
    path: String,
    ts: u64,
    is_dir: bool,
    /// 중복 판정용 소문자 키(Windows 는 대소문자 무시).
    key: String,
    /// 비 ASCII 경로의 char 배열(검색 때마다 다시 만들지 않도록 캐시). ASCII 면 비어 있다.
    chars: Vec<char>,
}

impl Item {
    fn new(path: String, ts: u64, is_dir: bool) -> Self {
        let key = path.to_lowercase();
        let chars = if path.is_ascii() {
            Vec::new()
        } else {
            path.chars().collect()
        };
        Self {
            path,
            ts,
            is_dir,
            key,
            chars,
        }
    }

    fn haystack(&self) -> Utf32Str<'_> {
        if self.path.is_ascii() {
            Utf32Str::Ascii(self.path.as_bytes())
        } else {
            Utf32Str::Unicode(&self.chars)
        }
    }
}

#[derive(Serialize)]
struct FileItemOut<'a> {
    path: &'a str,
    ts: u64,
    is_dir: bool,
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u64,
    items: Vec<FileItemOut<'a>>,
}

pub struct Recents {
    file: Option<PathBuf>,
    max: usize,
    items: Vec<Item>,
    last_error: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 경로 정규화. 반환값은 (정규화된 경로, 끝 구분자가 있었는지). 빈 문자열이면 None.
///
/// 규칙: 앞뒤 공백 trim, `/` -> `\`, `\\?\` 접두사 제거(`\\?\UNC\` 는 `\\` 로),
/// 끝 구분자 제거(드라이브 루트 `C:\` 와 루트 `\` 는 유지).
fn normalize_path(raw: &str) -> Option<(String, bool)> {
    let s = raw.trim().replace('/', "\\");
    if s.is_empty() {
        return None;
    }
    let mut s: String = if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        s
    };
    let had_trailing = s.ends_with('\\');
    if had_trailing {
        // 여러 개의 끝 구분자를 모두 제거하되 루트는 보존한다.
        let trimmed_len = s.trim_end_matches('\\').len();
        if trimmed_len == 0 {
            // "\" 또는 "\\" 같은 입력: 루트 하나만 유지.
            s = "\\".to_string();
        } else {
            s.truncate(trimmed_len);
            // 드라이브 문자만 남았으면(`C:`) 루트 구분자를 복원한다.
            let b = s.as_bytes();
            if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                s.push('\\');
            }
        }
    }
    if s.is_empty() {
        return None;
    }
    Some((s, had_trailing))
}

impl Recents {
    /// `file` 이 None 이면 저장 없이 메모리만 쓴다. 파일이 없거나 손상돼도 빈 목록으로 시작한다.
    pub fn load(file: Option<PathBuf>, max: usize) -> Self {
        let mut r = Self {
            file,
            max,
            items: Vec::new(),
            last_error: None,
        };
        if let Some(path) = r.file.clone() {
            match std::fs::read(&path) {
                Ok(bytes) => r.parse_file(&bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => r.last_error = Some(format!("최근 목록 읽기 실패: {e}")),
            }
        }
        r
    }

    /// `%APPDATA%\fdpick\recent.json`. APPDATA 가 없거나 비어 있으면 None.
    pub fn default_path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA")?;
        if appdata.is_empty() {
            return None;
        }
        Some(PathBuf::from(appdata).join("fdpick").join("recent.json"))
    }

    fn parse_file(&mut self, bytes: &[u8]) {
        // UTF-8 BOM 허용.
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
        let text = match std::str::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                self.last_error = Some(format!("최근 목록이 UTF-8 이 아님: {e}"));
                return;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                self.last_error = Some(format!("최근 목록 JSON 파싱 오류: {e}"));
                return;
            }
        };
        match value.get("version").and_then(|v| v.as_u64()) {
            Some(FILE_VERSION) => {}
            other => {
                self.last_error = Some(format!("알 수 없는 최근 목록 version: {other:?}"));
                return;
            }
        }
        let Some(arr) = value.get("items").and_then(|v| v.as_array()) else {
            self.last_error = Some("최근 목록에 items 배열이 없음".to_string());
            return;
        };
        let mut skipped = 0usize;
        let mut items: Vec<Item> = Vec::with_capacity(arr.len().min(self.max.max(1)));
        let mut seen = std::collections::HashSet::new();
        for e in arr {
            let path = e.get("path").and_then(|v| v.as_str());
            let ts = e.get("ts").and_then(|v| v.as_u64());
            let is_dir = e.get("is_dir").and_then(|v| v.as_bool());
            let (Some(path), Some(ts), Some(is_dir)) = (path, ts, is_dir) else {
                skipped += 1;
                continue;
            };
            let Some((norm, _)) = normalize_path(path) else {
                skipped += 1;
                continue;
            };
            let item = Item::new(norm, ts, is_dir);
            // 앞쪽(최신)이 우선이므로 이미 본 키는 버린다.
            if !seen.insert(item.key.clone()) {
                continue;
            }
            items.push(item);
            if items.len() >= self.max {
                break;
            }
        }
        if skipped > 0 {
            self.last_error = Some(format!("최근 목록에서 유효하지 않은 항목 {skipped}개를 건너뜀"));
        }
        self.items = items;
    }

    /// 각 경로를 입력 순서대로 push 한다(마지막 경로가 가장 최신). 변경되면 즉시 저장한다.
    pub fn record(&mut self, paths: &[String]) {
        let mut changed = false;
        for raw in paths {
            let Some((norm, had_trailing)) = normalize_path(raw) else {
                continue;
            };
            let is_dir = match std::fs::metadata(&norm) {
                Ok(m) => m.is_dir(),
                Err(_) => had_trailing,
            };
            let item = Item::new(norm, now_ms(), is_dir);
            if let Some(pos) = self.items.iter().position(|i| i.key == item.key) {
                self.items.remove(pos);
            }
            self.items.insert(0, item);
            changed = true;
        }
        if !changed {
            return;
        }
        self.items.truncate(self.max);
        self.save();
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// 최신순 상위 `limit` 개. indices 는 비어 있다.
    pub fn list(&self, limit: usize) -> Vec<RecentHit> {
        self.items
            .iter()
            .take(limit)
            .map(|i| make_hit(i, Vec::new()))
            .collect()
    }

    /// 퍼지 검색. query 가 trim 후 비면 `list` 와 같다.
    ///
    /// 정렬은 점수 내림차순, 같으면 최신 우선. indices 는 상위 `limit` 개에 대해서만 계산한다.
    pub fn search(&self, query: &str, limit: usize) -> Vec<RecentHit> {
        let query = query.trim();
        if query.is_empty() {
            return self.list(limit);
        }
        if limit == 0 {
            return Vec::new();
        }
        let query = query.replace('/', "\\");
        let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());

        // 1단계: 점수만 계산(indices 계산이 더 비싸므로 상위만 따로 구한다).
        let mut scored: Vec<(u32, usize)> = Vec::new();
        for (idx, item) in self.items.iter().enumerate() {
            if let Some(score) = pattern.score(item.haystack(), &mut matcher) {
                scored.push((score, idx));
            }
        }
        // 점수 내림차순, 동점이면 idx 오름차순(= 최신 우선).
        scored.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        // 2단계: limit 경계 점수와 같은 동점 구간까지 후보로 삼아 indices 를 구하고,
        // 동점이면 파일명(마지막 세그먼트)에 매치 글자가 있는 쪽을 우선한다(그다음 최신).
        // 항목의 haystack 은 코드포인트당 1글자(Utf32Str::Unicode(&[char]))로 직접 구성하므로
        // nucleo 가 돌려주는 인덱스가 곧 char 인덱스다(그래핌 변환 불필요). 단 결합 문자 시퀀스는
        // 그래핌으로 합쳐 비교되지 않고 코드포인트 단위로 비교된다.
        let boundary = scored.get(limit - 1).map(|x| x.0);
        if let Some(b) = boundary {
            scored.retain(|x| x.0 >= b);
        }
        struct Cand {
            score: u32,
            file_hit: bool,
            idx: usize,
            indices: Vec<u32>,
        }
        let mut cands: Vec<Cand> = scored
            .into_iter()
            .map(|(score, idx)| {
                let item = &self.items[idx];
                let mut ind: Vec<u32> = Vec::new();
                let _ = pattern.indices(item.haystack(), &mut matcher, &mut ind);
                ind.sort_unstable();
                ind.dedup();
                let fs = filename_start_chars(&item.path);
                Cand {
                    score,
                    file_hit: ind.iter().any(|&i| i as usize >= fs),
                    idx,
                    indices: ind,
                }
            })
            .collect();
        cands.sort_unstable_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then(b.file_hit.cmp(&a.file_hit))
                .then(a.idx.cmp(&b.idx))
        });
        cands.truncate(limit);
        cands
            .into_iter()
            .map(|c| make_hit(&self.items[c.idx], c.indices))
            .collect()
    }

    pub fn remove(&mut self, path: &str) {
        let Some((norm, _)) = normalize_path(path) else {
            return;
        };
        let key = norm.to_lowercase();
        let before = self.items.len();
        self.items.retain(|i| i.key != key);
        if self.items.len() != before {
            self.save();
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 줄이면 오래된 것부터 잘라내고 저장한다.
    pub fn set_max(&mut self, max: usize) {
        self.max = max;
        if self.items.len() > max {
            self.items.truncate(max);
            self.save();
        }
    }

    /// 원자적 저장: 같은 디렉터리의 임시 파일에 쓰고 rename 한다. 실패는 last_error 에 남긴다.
    fn save(&mut self) {
        let Some(file) = self.file.clone() else {
            return;
        };
        match self.write_atomic(&file) {
            Ok(()) => self.last_error = None,
            Err(e) => self.last_error = Some(format!("최근 목록 저장 실패: {e}")),
        }
    }

    fn write_atomic(&self, file: &Path) -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let out = FileOut {
            version: FILE_VERSION,
            items: self
                .items
                .iter()
                .map(|i| FileItemOut {
                    path: &i.path,
                    ts: i.ts,
                    is_dir: i.is_dir,
                })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&out)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let mut tmp_name = file.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp = file.with_file_name(tmp_name);
        let res = std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, file));
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        res
    }
}

/// 파일명(마지막 경로 세그먼트, 끝 구분자 무시) 시작 위치(char 인덱스).
fn filename_start_chars(path: &str) -> usize {
    let trimmed = path.trim_end_matches('\\');
    let byte_start = trimmed.rfind('\\').map_or(0, |p| p + 1);
    path[..byte_start].chars().count()
}

/// exists 확인은 list/search 가 돌려주는 상위 limit 개에 대해서만 수행한다.
/// 네트워크 경로(UNC 등)는 `Path::exists` 가 느릴 수 있어, 저장된 전체 항목이 아니라
/// 반환되는 항목에 한정해 호출 횟수를 limit 으로 묶는다. 없는 경로도 목록에서 지우지 않는다
/// (표시 쪽에서 회색 처리).
fn make_hit(item: &Item, indices: Vec<u32>) -> RecentHit {
    RecentHit {
        path: item.path.clone(),
        indices,
        is_dir: item.is_dir,
        exists: Path::new(&item.path).exists(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 테스트별 고유 임시 디렉터리.
    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fdpick_recents_{}_{}_{}",
            std::process::id(),
            tag,
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn paths(h: &[RecentHit]) -> Vec<&str> {
        h.iter().map(|x| x.path.as_str()).collect()
    }

    #[test]
    fn 정규화_규칙() {
        assert_eq!(normalize_path("  C:/a/b/  ").unwrap(), (r"C:\a\b".into(), true));
        assert_eq!(normalize_path(r"\\?\C:\a\b").unwrap().0, r"C:\a\b");
        assert_eq!(normalize_path(r"\\?\UNC\srv\share\x").unwrap().0, r"\\srv\share\x");
        assert_eq!(normalize_path(r"C:\").unwrap(), (r"C:\".into(), true));
        assert_eq!(normalize_path("C:/").unwrap().0, r"C:\");
        assert_eq!(normalize_path(r"C:\\\").unwrap().0, r"C:\");
        assert_eq!(normalize_path("   "), None);
        assert_eq!(normalize_path(""), None);
        assert_eq!(normalize_path(r"C:\a").unwrap(), (r"C:\a".into(), false));
    }

    #[test]
    fn record_정규화와_대소문자_중복_제거() {
        let mut r = Recents::load(None, 100);
        r.record(&s(&["C:/Work/Report/", "  ", ""]));
        assert_eq!(r.len(), 1);
        r.record(&s(&[r"c:\work\REPORT"]));
        assert_eq!(r.len(), 1);
        let l = r.list(10);
        // 최신 입력의 표기가 유지된다.
        assert_eq!(l[0].path, r"c:\work\REPORT");
    }

    #[test]
    fn 최신순과_max_제한() {
        let mut r = Recents::load(None, 3);
        for p in ["C:\\a", "C:\\b", "C:\\c", "C:\\d"] {
            r.record(&s(&[p]));
        }
        assert_eq!(paths(&r.list(10)), vec!["C:\\d", "C:\\c", "C:\\b"]);
        // 기존 항목을 다시 기록하면 맨 위로.
        r.record(&s(&["C:\\B"]));
        assert_eq!(paths(&r.list(10)), vec!["C:\\B", "C:\\d", "C:\\c"]);
        assert_eq!(paths(&r.list(2)), vec!["C:\\B", "C:\\d"]);
    }

    #[test]
    fn 다중_record_순서() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&["C:\\1", "C:\\2", "C:\\3"]));
        assert_eq!(paths(&r.list(10)), vec!["C:\\3", "C:\\2", "C:\\1"]);
        // 한 호출 안의 중복은 마지막 위치 기준.
        r.record(&s(&["C:\\1", "C:\\9", "C:\\1"]));
        assert_eq!(paths(&r.list(10)), vec!["C:\\1", "C:\\9", "C:\\3", "C:\\2"]);
    }

    #[test]
    fn 저장_후_재로드가_같은_내용() {
        let d = tmp_dir("reload");
        let f = d.join("sub").join("recent.json");
        let mut r = Recents::load(Some(f.clone()), 10);
        r.record(&s(&["C:\\a\\1.txt", "C:\\a\\2.txt"]));
        assert_eq!(r.last_error(), None);
        let r2 = Recents::load(Some(f.clone()), 10);
        assert_eq!(r2.last_error(), None);
        assert_eq!(r.list(10), r2.list(10));
        let ts1: Vec<u64> = r.items.iter().map(|i| i.ts).collect();
        let ts2: Vec<u64> = r2.items.iter().map(|i| i.ts).collect();
        assert_eq!(ts1, ts2);
        // 파일 형식 확인.
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["items"][0]["path"], "C:\\a\\2.txt");
        assert!(v["items"][0]["ts"].is_u64());
        assert!(v["items"][0]["is_dir"].is_boolean());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 저장_중_임시_파일이_남지_않는다() {
        let d = tmp_dir("tmpleft");
        let f = d.join("recent.json");
        let mut r = Recents::load(Some(f.clone()), 10);
        for i in 0..5 {
            r.record(&[format!("C:\\x\\{i}")]);
        }
        r.remove("C:\\x\\1");
        r.set_max(2);
        let names: Vec<String> = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["recent.json".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 저장_실패는_무시하고_last_error() {
        let d = tmp_dir("saveerr");
        // 부모 경로가 "파일"이라 디렉터리를 만들 수 없다.
        let blocker = d.join("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let mut r = Recents::load(Some(blocker.join("recent.json")), 10);
        r.record(&s(&["C:\\a"]));
        assert_eq!(r.len(), 1);
        assert!(r.last_error().is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 파일_없음_손상_version_bom() {
        let d = tmp_dir("bad");
        // 파일 없음
        let r = Recents::load(Some(d.join("none.json")), 10);
        assert_eq!(r.len(), 0);
        assert_eq!(r.last_error(), None);
        // 손상 JSON
        let f = d.join("bad.json");
        std::fs::write(&f, b"{ not json").unwrap();
        let r = Recents::load(Some(f.clone()), 10);
        assert_eq!(r.len(), 0);
        assert!(r.last_error().is_some());
        // 비 UTF-8
        std::fs::write(&f, [0xFF, 0xFE, 0x00]).unwrap();
        let r = Recents::load(Some(f.clone()), 10);
        assert_eq!(r.len(), 0);
        assert!(r.last_error().is_some());
        // 알 수 없는 version
        std::fs::write(&f, br#"{"version":2,"items":[{"path":"C:\\a","ts":1,"is_dir":false}]}"#)
            .unwrap();
        let r = Recents::load(Some(f.clone()), 10);
        assert_eq!(r.len(), 0);
        assert!(r.last_error().unwrap().contains("version"));
        // 항목 필드 누락: 유효한 항목만
        std::fs::write(
            &f,
            br#"{"version":1,"items":[{"path":"C:\\ok","ts":1,"is_dir":false},{"ts":1,"is_dir":false},{"path":"C:\\nots","is_dir":true},{"path":5,"ts":1,"is_dir":true}]}"#,
        )
        .unwrap();
        let r = Recents::load(Some(f.clone()), 10);
        assert_eq!(paths(&r.list(10)), vec!["C:\\ok"]);
        assert!(r.last_error().is_some());
        // BOM
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(br#"{"version":1,"items":[{"path":"C:\\bom","ts":1,"is_dir":true}]}"#);
        std::fs::write(&f, b).unwrap();
        let r = Recents::load(Some(f.clone()), 10);
        assert_eq!(r.last_error(), None);
        assert_eq!(paths(&r.list(10)), vec!["C:\\bom"]);
        assert!(r.list(1)[0].is_dir);
        // 로드 시 max 적용
        std::fs::write(
            &f,
            br#"{"version":1,"items":[{"path":"C:\\1","ts":3,"is_dir":false},{"path":"C:\\2","ts":2,"is_dir":false},{"path":"C:\\3","ts":1,"is_dir":false}]}"#,
        )
        .unwrap();
        let r = Recents::load(Some(f), 2);
        assert_eq!(paths(&r.list(10)), vec!["C:\\1", "C:\\2"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 한글_공백_대괄호_경로_왕복() {
        let d = tmp_dir("uni");
        let f = d.join("recent.json");
        let list = [
            r"C:\내 문서\보고서 [최종] (1).docx",
            r"D:\data\a b\c#d&e%f.txt",
            r"\\서버\공유\폴더",
            "C:\\emoji\\😀.txt",
        ];
        let mut r = Recents::load(Some(f.clone()), 10);
        r.record(&list.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        let r2 = Recents::load(Some(f), 10);
        let mut got: Vec<&str> = r2.items.iter().map(|i| i.path.as_str()).collect();
        got.reverse();
        assert_eq!(got, list.to_vec());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn search_파일명_매치_우선() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\work\report\x.txt", r"C:\other\report.txt"]));
        // 입력 순서상 other 가 더 최신이지만, 순서 무관하게 파일명 매치가 위여야 한다.
        let h = r.search("report", 10);
        assert_eq!(paths(&h), vec![r"C:\other\report.txt", r"C:\work\report\x.txt"]);
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\other\report.txt", r"C:\work\report\x.txt"]));
        let h = r.search("report", 10);
        assert_eq!(paths(&h), vec![r"C:\other\report.txt", r"C:\work\report\x.txt"]);
    }

    #[test]
    fn search_동점이면_최신_우선과_limit() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\a\foo", r"C:\b\foo", r"C:\c\foo"]));
        // 점수가 같을 만큼 구조가 같으므로 최신(c) 우선.
        let h = r.search("foo", 10);
        assert_eq!(paths(&h), vec![r"C:\c\foo", r"C:\b\foo", r"C:\a\foo"]);
        assert_eq!(r.search("foo", 2).len(), 2);
        assert_eq!(r.search("foo", 0).len(), 0);
        assert!(r.search("zzzz", 10).is_empty());
    }

    #[test]
    fn search_스마트_케이스() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\docs\readme.md", r"C:\Docs2\Readme.md"]));
        assert_eq!(r.search("readme", 10).len(), 2);
        assert_eq!(paths(&r.search("Readme", 10)), vec![r"C:\Docs2\Readme.md"]);
        assert!(r.search("README", 10).is_empty());
    }

    #[test]
    fn search_슬래시_입력도_매칭() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\work\report\x.txt", r"C:\zzz\q"]));
        let h = r.search("work/report", 10);
        assert_eq!(paths(&h), vec![r"C:\work\report\x.txt"]);
        // 앞뒤 공백 trim
        assert_eq!(r.search("  work/report  ", 10).len(), 1);
    }

    #[test]
    fn search_indices_는_char_단위() {
        let mut r = Recents::load(None, 10);
        // 앞에 한글/비 BMP 이모지를 두어 바이트/UTF-16/char 인덱스가 달라지게 한다.
        r.record(&s(&["C:\\내문서\\😀보고서_최종.docx", r"C:\other\a.txt"]));
        let h = r.search("보고서", 10);
        assert_eq!(h.len(), 1);
        let chars: Vec<char> = h[0].path.chars().collect();
        let got: String = h[0].indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(got, "보고서");
        let start = chars.iter().position(|&c| c == '보').unwrap() as u32;
        assert_eq!(h[0].indices, vec![start, start + 1, start + 2]);

        // ASCII 경로: 오름차순, 중복 없음.
        let mut r = Recents::load(None, 10);
        r.record(&s(&[r"C:\work\report\x.txt"]));
        let h = r.search("wrk/rpt", 10);
        assert_eq!(h.len(), 1);
        assert!(h[0].indices.windows(2).all(|w| w[0] < w[1]));
        let chars: Vec<char> = h[0].path.chars().collect();
        let got: String = h[0].indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(got.replace('\\', "/"), "wrk/rpt");
    }

    #[test]
    fn search_nfd_한글도_동작() {
        let nfd = "C:\\\u{1112}\u{1161}\u{11AB}\\\u{1100}\u{1161}.txt";
        let mut r = Recents::load(None, 10);
        r.record(&[nfd.to_string()]);
        let h = r.search("txt", 10);
        assert_eq!(h.len(), 1);
        let chars: Vec<char> = h[0].path.chars().collect();
        let got: String = h[0].indices.iter().map(|&i| chars[i as usize]).collect();
        assert_eq!(got, "txt");
    }

    #[test]
    fn 빈_query는_list와_같다() {
        let mut r = Recents::load(None, 10);
        r.record(&s(&["C:\\a", "C:\\b"]));
        assert_eq!(r.search("", 10), r.list(10));
        assert_eq!(r.search("   ", 1), r.list(1));
        assert!(r.list(10).iter().all(|h| h.indices.is_empty()));
    }

    #[test]
    fn remove_동작() {
        let d = tmp_dir("rm");
        let f = d.join("recent.json");
        let mut r = Recents::load(Some(f.clone()), 10);
        r.record(&s(&["C:\\a", "C:\\B", "C:\\c"]));
        r.remove("c:/b/"); // 정규화 + 대소문자 무시
        assert_eq!(paths(&r.list(10)), vec!["C:\\c", "C:\\a"]);
        r.remove("C:\\none");
        assert_eq!(r.len(), 2);
        let r2 = Recents::load(Some(f), 10);
        assert_eq!(paths(&r2.list(10)), vec!["C:\\c", "C:\\a"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn set_max_줄이면_오래된_것부터_제거하고_저장() {
        let d = tmp_dir("setmax");
        let f = d.join("recent.json");
        let mut r = Recents::load(Some(f.clone()), 10);
        r.record(&s(&["C:\\1", "C:\\2", "C:\\3", "C:\\4"]));
        r.set_max(2);
        assert_eq!(paths(&r.list(10)), vec!["C:\\4", "C:\\3"]);
        let r2 = Recents::load(Some(f), 10);
        assert_eq!(paths(&r2.list(10)), vec!["C:\\4", "C:\\3"]);
        // 늘려도 이후 record 에 반영.
        r.set_max(3);
        r.record(&s(&["C:\\5", "C:\\6"]));
        assert_eq!(paths(&r.list(10)), vec!["C:\\6", "C:\\5", "C:\\4"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn exists_와_is_dir_판정() {
        let d = tmp_dir("exists");
        let file = d.join("a b.txt");
        std::fs::write(&file, b"x").unwrap();
        let sub = d.join("폴더");
        std::fs::create_dir_all(&sub).unwrap();
        let missing = d.join("missing.txt");
        let missing_dir = format!("{}\\nodir\\", d.display());

        let mut r = Recents::load(None, 10);
        r.record(&[
            file.display().to_string(),
            // 끝 구분자 + 슬래시 표기
            format!("{}/", sub.display().to_string().replace('\\', "/")),
            missing.display().to_string(),
            missing_dir,
        ]);
        let l = r.list(10);
        assert_eq!(l.len(), 4);
        // 최신순: nodir, missing, 폴더, a b.txt
        assert!(!l[0].exists && l[0].is_dir); // 없는 경로 + 끝 구분자 -> is_dir true
        assert!(!l[1].exists && !l[1].is_dir);
        assert!(l[2].exists && l[2].is_dir);
        assert!(l[3].exists && !l[3].is_dir);
        // 없는 경로도 목록에서 제거되지 않는다.
        assert_eq!(r.len(), 4);
        // search 결과에도 exists 가 채워진다.
        let h = r.search("missing", 10);
        assert_eq!(h.len(), 1);
        assert!(!h[0].exists);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn default_path_형태() {
        if let Some(p) = Recents::default_path() {
            assert!(p.ends_with(Path::new("fdpick").join("recent.json")));
        }
    }

    /// 성능 측정(`cargo test --release -- --nocapture perf`). 임계값은 넉넉히 잡는다.
    #[test]
    fn perf_search_시간_측정() {
        for n in [1000usize, 5000] {
            let mut r = Recents::load(None, n);
            let v: Vec<String> = (0..n)
                .map(|i| match i % 3 {
                    0 => format!(r"C:\Users\someone\Documents\project{}\src\module{}\file{}.rs", i % 40, i % 17, i),
                    1 => format!(r"D:\데이터\보고서{}\최종본\문서_{}.docx", i % 50, i),
                    _ => format!(r"\\server\share\team{}\reports\2024\report_{}.xlsx", i % 9, i),
                })
                .collect();
            r.record(&v);
            assert_eq!(r.len(), n);
            for q in ["rep", "report", "file12", "보고서", "docs/proj"] {
                // 워밍업 후 20회 평균.
                let _ = r.search(q, 50);
                let t = std::time::Instant::now();
                let mut cnt = 0;
                for _ in 0..20 {
                    cnt = r.search(q, 50).len();
                }
                let per = t.elapsed().as_secs_f64() * 1000.0 / 20.0;
                println!("PERF n={n} q={q:?} hits={cnt} {per:.2} ms/search (exists 50회 포함)");
            }
        }
    }
}
