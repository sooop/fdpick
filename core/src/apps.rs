//! 앱 런처: 시작 메뉴 앱 목록과 윈도 내장 도구를 담는 작은 인덱스.
//!
//! 항목은 수백 개 수준이라 nucleo 대신 자체 점수 계산을 쓴다. 이유는 두 가지다.
//! - 공백을 무시해야 한다("장치관리자" 와 "장치 관리자" 가 같다).
//! - 한글 초성 검색(ㅈㅊㄱㄹㅈ)과 일반 글자를 한 입력에 섞어 쓸 수 있어야 한다.
//!
//! 목록 수집(AppsFolder 열거)은 COM 이 필요해 앱 쪽에서 하고, 여기에는 `set_entries` 로 넘긴다.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows_sys::Win32::UI::Shell::{ShellExecuteW, SE_ERR_ACCESSDENIED};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// 실행 가능한 항목 하나.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    /// 화면에 보이는 이름
    pub name: String,
    /// 이름 말고 검색에 쓸 문자열(영문 이름, 실행 파일 이름 등)
    pub aliases: Vec<String>,
    /// ShellExecute 대상: `shell:AppsFolder\…`, 실행 파일, `.msc`, `.cpl`, `ms-settings:…`
    pub target: String,
    /// ShellExecute 인자(없으면 빈 문자열)
    pub args: String,
    /// 관리자 권한으로 실행할 때 쓸 (실행 파일, 인자). 지원하지 않는 항목(스토어 앱, 설정 페이지 등)은 None.
    pub admin: Option<(String, String)>,
}

/// 검색 결과 한 건. `indices` 는 `name` 안의 char 인덱스(오름차순).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppHit {
    pub name: String,
    pub indices: Vec<u32>,
    /// 최근에 쓴 앱 구역의 항목(같은 앱이 전체 구역에도 다시 나온다)
    pub recent: bool,
}

/// 맨 위 "최근" 구역에 올리는 앱의 최대 개수
pub const RECENT_LIMIT: usize = 5;

/// 시작 메뉴 목록에서 앱이 아닌 항목(제거 프로그램, 웹 링크, 문서·스크립트 파일)을 거른다.
/// `parse` 는 AppsFolder 파싱 이름이다.
pub fn is_launchable(name: &str, parse: &str) -> bool {
    let n = name.to_lowercase();
    if ["uninstall", "삭제", "제거", "언인스톨"].iter().any(|w| n.contains(w)) {
        return false;
    }
    let p = parse.to_lowercase();
    if p.contains("://") {
        return false;
    }
    const NOT_APP: &[&str] = &[".html", ".htm", ".chm", ".ahk", ".url", ".txt", ".md", ".pdf", ".rtf", ".doc", ".docx"];
    !NOT_APP.iter().any(|ext| p.ends_with(ext))
}

/// 시스템 폴더(`%SystemRoot%\System32`, `%SystemRoot%`)에 있는 파일인가. URI(`ms-settings:` 등)는 true.
fn system_file_exists(target: &str) -> bool {
    if target.contains(':') {
        return true;
    }
    let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) else {
        return true;
    };
    let root = PathBuf::from(root);
    root.join("System32").join(target).exists() || root.join(target).exists()
}

/// 윈도 내장 도구와 설정 페이지. AppsFolder 에 없거나 이름이 로캘마다 달라 찾기 어려운 것들.
/// 이 PC에 설치된 도구만 돌려준다(예: Home 에디션에는 gpedit.msc 가 없다).
pub fn builtin_entries() -> Vec<AppEntry> {
    builtin_with(system_file_exists)
}

fn builtin_with(exists: impl Fn(&str) -> bool) -> Vec<AppEntry> {
    // (이름, 별칭(공백으로 구분), 대상, 인자)
    const T: &[(&str, &str, &str, &str)] = &[
        ("장치 관리자", "device manager devmgmt", "devmgmt.msc", ""),
        ("디스크 관리", "disk management diskmgmt", "diskmgmt.msc", ""),
        ("서비스", "services", "services.msc", ""),
        ("이벤트 뷰어", "event viewer eventvwr", "eventvwr.msc", ""),
        ("작업 관리자", "task manager taskmgr", "taskmgr.exe", ""),
        ("작업 스케줄러", "task scheduler taskschd", "taskschd.msc", ""),
        ("컴퓨터 관리", "computer management compmgmt", "compmgmt.msc", ""),
        ("로컬 그룹 정책 편집기", "group policy gpedit", "gpedit.msc", ""),
        ("로컬 보안 정책", "local security policy secpol", "secpol.msc", ""),
        ("로컬 사용자 및 그룹", "local users groups lusrmgr", "lusrmgr.msc", ""),
        ("성능 모니터", "performance monitor perfmon", "perfmon.msc", ""),
        ("리소스 모니터", "resource monitor resmon", "resmon.exe", ""),
        ("공유 폴더", "shared folders fsmgmt", "fsmgmt.msc", ""),
        ("고급 보안이 포함된 Windows 방화벽", "firewall advanced wf", "wf.msc", ""),
        ("제어판", "control panel control", "control.exe", ""),
        ("프로그램 및 기능", "programs and features appwiz uninstall", "appwiz.cpl", ""),
        ("네트워크 연결", "network connections ncpa adapter", "ncpa.cpl", ""),
        ("시스템 속성", "system properties sysdm", "sysdm.cpl", ""),
        (
            "환경 변수",
            "environment variables env",
            "rundll32.exe",
            "sysdm.cpl,EditEnvironmentVariables",
        ),
        ("전원 옵션", "power options powercfg", "powercfg.cpl", ""),
        ("소리 (제어판)", "sound mmsys", "mmsys.cpl", ""),
        ("마우스 속성", "mouse main", "main.cpl", ""),
        ("날짜 및 시간 (제어판)", "date time timedate", "timedate.cpl", ""),
        ("Windows 방화벽 (제어판)", "firewall", "firewall.cpl", ""),
        ("레지스트리 편집기", "registry regedit", "regedit.exe", ""),
        ("시스템 정보", "system information msinfo32", "msinfo32.exe", ""),
        ("디스크 정리", "disk cleanup cleanmgr", "cleanmgr.exe", ""),
        ("원격 데스크톱 연결", "remote desktop rdp mstsc", "mstsc.exe", ""),
        ("명령 프롬프트", "command prompt cmd", "cmd.exe", ""),
        ("Wi-Fi 설정", "wifi wireless network", "ms-settings:network-wifi", ""),
        ("네트워크 설정", "network internet", "ms-settings:network", ""),
        ("블루투스 설정", "bluetooth devices", "ms-settings:bluetooth", ""),
        ("디스플레이 설정", "display screen resolution", "ms-settings:display", ""),
        ("소리 설정", "sound audio volume", "ms-settings:sound", ""),
        ("프린터 및 스캐너 설정", "printers scanners", "ms-settings:printers", ""),
        ("앱 및 기능 설정", "apps features installed", "ms-settings:appsfeatures", ""),
        ("기본 앱 설정", "default apps", "ms-settings:defaultapps", ""),
        ("시작 프로그램 설정", "startup apps", "ms-settings:startupapps", ""),
        ("Windows 업데이트 설정", "windows update", "ms-settings:windowsupdate", ""),
        ("저장 공간 설정", "storage sense", "ms-settings:storagesense", ""),
        ("개인 설정", "personalization theme background", "ms-settings:personalization", ""),
        ("전원 및 절전 설정", "power sleep", "ms-settings:powersleep", ""),
        ("날짜 및 시간 설정", "date time", "ms-settings:dateandtime", ""),
        ("언어 및 지역 설정", "language region", "ms-settings:regionlanguage", ""),
        ("계정 정보 설정", "account user info", "ms-settings:yourinfo", ""),
        ("시스템 정보 설정", "about pc version", "ms-settings:about", ""),
    ];
    T.iter()
        .filter(|(_, _, target, _)| exists(target))
        .map(|(name, aliases, target, args)| {
            // .cpl 은 연결 프로그램에 기대지 않고 control.exe 로 연다(ShellExecute 가 오류 31 을 낼 수 있다)
            let (target, args) = if target.ends_with(".cpl") { ("control.exe", *target) } else { (*target, *args) };
            // .msc 는 runas 동사가 보장되지 않아 mmc.exe 로 연다. 설정 페이지(URI)는 관리자 실행이 없다.
            let admin = if target.ends_with(".msc") {
                Some(("mmc.exe".to_string(), target.to_string()))
            } else if target.ends_with(".exe") {
                Some((target.to_string(), args.to_string()))
            } else {
                None
            };
            AppEntry {
                name: name.to_string(),
                aliases: aliases.split_whitespace().map(str::to_string).collect(),
                target: target.to_string(),
                args: args.to_string(),
                admin,
            }
        })
        .collect()
}

// ---------------------------------------------------------------- 한글 처리

const CHOSEONG: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ', 'ㅌ', 'ㅍ', 'ㅎ',
];

fn syllable(c: char) -> Option<u32> {
    let u = c as u32;
    (0xAC00..=0xD7A3).contains(&u).then(|| u - 0xAC00)
}

fn is_choseong(c: char) -> bool {
    CHOSEONG.contains(&c)
}

/// 질의 글자 `q` 가 키 글자 `k` 와 맞는가.
/// - 같은 글자
/// - `q` 가 초성 자모이고 `k` 가 그 초성으로 시작하는 음절
/// - 마지막 질의 글자가 조합 중인 음절이면(받침 없음 또는 같은 받침) 같은 초성·중성의 음절
fn char_eq(q: char, k: char, last: bool) -> bool {
    if q == k {
        return true;
    }
    if is_choseong(q) {
        return syllable(k).is_some_and(|s| CHOSEONG[(s / 588) as usize] == q);
    }
    if last {
        if let (Some(a), Some(b)) = (syllable(q), syllable(k)) {
            return a / 28 == b / 28 && (a % 28 == 0 || a % 28 == b % 28);
        }
    }
    false
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

// ---------------------------------------------------------------- 점수

const BASE: i32 = 10;
const WORD_START: i32 = 16;
const CONSEC: i32 = 14;
const GAP: i32 = 2;
const LEAD: i32 = 1;
const NEG: i32 = i32::MIN / 2;

/// 검색용으로 가공한 문자열: 공백·구두점을 뺀 소문자 글자와 각 글자의 단어 시작 여부, 원문 인덱스.
struct Key {
    chars: Vec<char>,
    starts: Vec<bool>,
    orig: Vec<u32>,
}

impl Key {
    fn new(s: &str) -> Self {
        let mut k = Key { chars: Vec::new(), starts: Vec::new(), orig: Vec::new() };
        let mut boundary = true;
        let mut prev: Option<char> = None;
        for (i, c) in s.chars().enumerate() {
            if c.is_whitespace() || "-_.()[]/\\:".contains(c) {
                boundary = true;
                prev = None;
                continue;
            }
            let camel = prev.is_some_and(|p| p.is_lowercase()) && c.is_uppercase();
            k.chars.push(lower(c));
            k.starts.push(boundary || camel);
            k.orig.push(i as u32);
            boundary = false;
            prev = Some(c);
        }
        k
    }
}

/// 질의 `q`(가공 완료)를 키에 맞춘 최고 점수와 매치 위치(키 안의 인덱스). 맞지 않으면 None.
fn score_key(q: &[char], k: &Key) -> Option<(i32, Vec<usize>)> {
    let n = q.len();
    let m = k.chars.len();
    if n == 0 || m < n {
        return None;
    }
    let mut dp = vec![vec![NEG; m]; n];
    let mut back = vec![vec![usize::MAX; m]; n];
    for i in 0..n {
        let last = i == n - 1;
        // run: 지금까지 본 j' 중 dp[i-1][j'] + GAP*j' 의 최댓값과 그 위치
        let mut run = (NEG, usize::MAX);
        for j in 0..m {
            if i > 0 && j > 0 && dp[i - 1][j - 1] > NEG {
                let v = dp[i - 1][j - 1] + GAP * (j as i32 - 1);
                if v > run.0 {
                    run = (v, j - 1);
                }
            }
            if !char_eq(q[i], k.chars[j], last) {
                continue;
            }
            let (from, prev) = if i == 0 {
                (-LEAD * j as i32, usize::MAX)
            } else {
                let mut cand = (NEG, usize::MAX);
                if run.0 > NEG {
                    cand = (run.0 - GAP * (j as i32 - 1), run.1);
                }
                // 바로 앞 글자에서 이어지면 연속 보너스
                if j > 0 && dp[i - 1][j - 1] > NEG && dp[i - 1][j - 1] + CONSEC > cand.0 {
                    cand = (dp[i - 1][j - 1] + CONSEC, j - 1);
                }
                if cand.0 <= NEG {
                    continue;
                }
                cand
            };
            dp[i][j] = from + BASE + if k.starts[j] { WORD_START } else { 0 };
            back[i][j] = prev;
        }
    }
    let (mut best, mut bj) = (NEG, usize::MAX);
    for j in 0..m {
        if dp[n - 1][j] > best {
            best = dp[n - 1][j];
            bj = j;
        }
    }
    if best <= NEG {
        return None;
    }
    let mut idx = vec![0; n];
    let mut j = bj;
    for i in (0..n).rev() {
        idx[i] = j;
        j = back[i][j];
    }
    // 이름 전체가 정확히 일치하거나 앞에서부터 이어지면 가산
    let contiguous = idx.windows(2).all(|w| w[1] == w[0] + 1);
    let mut score = best;
    if idx[0] == 0 && contiguous {
        score += 60;
        if m == n {
            score += 100;
        }
    }
    Some((score, idx))
}

// ---------------------------------------------------------------- 인덱스

struct Item {
    entry: AppEntry,
    name_key: Key,
    alias_keys: Vec<Key>,
    usage_key: String,
}

pub struct AppIndex {
    items: Vec<Item>,
    /// 앱 이름(소문자) -> 마지막으로 실행한 시각(Unix 밀리초)
    usage: HashMap<String, u64>,
    usage_file: Option<PathBuf>,
    loaded_at: Option<Instant>,
}

impl AppIndex {
    /// `usage_file` 은 실행 횟수 저장 파일(None 이면 메모리만).
    pub fn new(usage_file: Option<PathBuf>) -> Self {
        let usage = usage_file
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<HashMap<String, u64>>(&b).ok())
            .unwrap_or_default();
        Self { items: Vec::new(), usage, usage_file, loaded_at: None }
    }

    /// `%APPDATA%\fdpick\apps_recent.json`
    pub fn default_usage_path() -> Option<PathBuf> {
        let appdata = std::env::var_os("APPDATA").filter(|a| !a.is_empty())?;
        Some(PathBuf::from(appdata).join("fdpick").join("apps_recent.json"))
    }

    /// 목록을 통째로 교체한다. 이름이 같은 항목은 앞의 것만 남긴다.
    pub fn set_entries(&mut self, entries: Vec<AppEntry>) {
        let mut seen = std::collections::HashSet::new();
        self.items = entries
            .into_iter()
            .filter(|e| !e.name.trim().is_empty() && seen.insert(lower_all(&e.name)))
            .map(|entry| Item {
                name_key: Key::new(&entry.name),
                alias_keys: entry.aliases.iter().map(|a| Key::new(a)).collect(),
                usage_key: lower_all(&entry.name),
                entry,
            })
            .collect();
        self.loaded_at = Some(Instant::now());
    }

    /// 다음 `age` 조회에서 오래된 것으로 보이게 한다(내장 항목만 미리 채워 둘 때).
    pub fn invalidate(&mut self) {
        self.loaded_at = None;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 마지막으로 목록을 채운 뒤 지난 시간. 한 번도 채우지 않았으면 None.
    pub fn age(&self) -> Option<std::time::Duration> {
        self.loaded_at.map(|t| t.elapsed())
    }

    fn last_used(&self, key: &str) -> u64 {
        self.usage.get(key).copied().unwrap_or(0)
    }

    /// 위쪽에 최근에 쓴 앱(최대 `RECENT_LIMIT` 개), 아래쪽에 전체(최대 `limit` 개).
    /// 최근 구역의 앱도 전체 구역에 다시 나온다. 질의가 있으면 질의에 맞는 것만 두 구역에 담는다.
    /// 최근 구역은 질의가 비면 최근 실행순, 있으면 점수순이다. 전체 구역은 질의가 비면 이름순, 있으면 점수순이다.
    pub fn search(&self, query: &str, limit: usize) -> Vec<AppHit> {
        let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(lower).collect();
        let mut scored: Vec<(i32, &Item, Vec<u32>)> = Vec::new();
        for it in &self.items {
            if q.is_empty() {
                scored.push((0, it, Vec::new()));
                continue;
            }
            let by_name = score_key(&q, &it.name_key);
            let by_alias = it.alias_keys.iter().filter_map(|k| score_key(&q, k)).map(|(s, _)| s - 5).max();
            match (by_name, by_alias) {
                (Some((s, idx)), a) if a.map_or(true, |a| s >= a) => {
                    let indices = idx.iter().map(|&j| it.name_key.orig[j]).collect();
                    scored.push((s, it, indices));
                }
                (_, Some(a)) => scored.push((a, it, Vec::new())),
                _ => {}
            }
        }
        // 전체 구역: 점수 내림차순(질의가 비면 모두 0 이라 이름순)
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.entry.name.cmp(&b.1.entry.name)));

        let mut recent: Vec<&(i32, &Item, Vec<u32>)> =
            scored.iter().filter(|(_, it, _)| self.last_used(&it.usage_key) > 0).collect();
        if q.is_empty() {
            recent.sort_by(|a, b| self.last_used(&b.1.usage_key).cmp(&self.last_used(&a.1.usage_key)));
        }
        recent.truncate(RECENT_LIMIT);

        let hit = |(_, it, indices): &(i32, &Item, Vec<u32>), recent: bool| AppHit {
            name: it.entry.name.clone(),
            indices: indices.clone(),
            recent,
        };
        let mut out: Vec<AppHit> = recent.into_iter().map(|r| hit(r, true)).collect();
        out.extend(scored.iter().take(limit).map(|r| hit(r, false)));
        out
    }

    pub fn find(&self, name: &str) -> Option<&AppEntry> {
        let key = lower_all(name);
        self.items.iter().find(|i| i.usage_key == key).map(|i| &i.entry)
    }

    /// 실행 시각을 지금으로 갱신하고 저장한다.
    pub fn record_use(&mut self, name: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_millis() as u64)
            .max(1);
        // 같은 밀리초에 연속 실행해도 순서가 갈리도록 직전 최댓값보다 크게 한다
        let newest = self.usage.values().copied().max().unwrap_or(0);
        self.usage.insert(lower_all(name), now.max(newest + 1));
        let Some(path) = self.usage_file.clone() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec(&self.usage) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

fn lower_all(s: &str) -> String {
    s.chars().map(lower).collect()
}

/// 항목을 실행한다(ShellExecute). 기다리지 않는다. `admin` 이면 관리자 권한으로 실행하며
/// UAC 확인 창은 사용자가 직접 처리한다.
pub fn launch(e: &AppEntry, admin: bool) -> Result<(), String> {
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (target, target_args) = if admin {
        match &e.admin {
            Some((t, a)) => (t.as_str(), a.as_str()),
            None => return Err(format!("{} 은(는) 관리자 권한 실행을 지원하지 않습니다", e.name)),
        }
    } else {
        (e.target.as_str(), e.args.as_str())
    };
    let verb = wide(if admin { "runas" } else { "open" });
    let file = wide(target);
    let args = wide(target_args);
    // ShellExecute 는 COM 이 초기화된 스레드에서 불러야 연결 프로그램 조회가 안정적이다
    let com = unsafe { CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) } >= 0;
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            if target_args.is_empty() { std::ptr::null() } else { args.as_ptr() },
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if com {
        unsafe { CoUninitialize() };
    }
    if admin && r == SE_ERR_ACCESSDENIED as isize {
        // UAC 에서 사용자가 취소하면 이 코드가 돌아온다
        Err(format!("{} 관리자 권한 실행이 취소되었습니다", e.name))
    } else if r <= 32 {
        Err(format!("{} 을(를) 실행하지 못했습니다 (오류 코드 {r}): {target} {target_args}", e.name).trim_end().to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, aliases: &[&str]) -> AppEntry {
        AppEntry {
            admin: None,
            name: name.into(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            target: format!("t:{name}"),
            args: String::new(),
        }
    }

    fn index(names: &[&str]) -> AppIndex {
        let mut i = AppIndex::new(None);
        i.set_entries(names.iter().map(|n| entry(n, &[])).collect());
        i
    }

    fn names(h: &[AppHit]) -> Vec<&str> {
        h.iter().map(|x| x.name.as_str()).collect()
    }

    #[test]
    fn spacing_is_ignored_in_both_directions() {
        let i = index(&["장치 관리자", "디스크 관리"]);
        assert_eq!(names(&i.search("장치관리자", 10))[0], "장치 관리자");
        assert_eq!(names(&i.search("장치 관리자", 10))[0], "장치 관리자");
        let j = index(&["장치관리자"]);
        assert_eq!(names(&j.search("장치 관리", 10)), vec!["장치관리자"]);
    }

    #[test]
    fn choseong_search() {
        let i = index(&["장치 관리자", "디스크 관리", "계산기"]);
        assert_eq!(names(&i.search("ㅈㅊㄱㄹㅈ", 10)), vec!["장치 관리자"]);
        assert_eq!(names(&i.search("ㄱㅅㄱ", 10)), vec!["계산기"]);
        // 초성과 글자를 섞는다
        assert_eq!(names(&i.search("장ㅊㄱ", 10)), vec!["장치 관리자"]);
    }

    #[test]
    fn partial_syllable_at_the_end_matches() {
        let i = index(&["장치 관리자", "계산기"]);
        assert_eq!(names(&i.search("자", 10)), vec!["장치 관리자"]);
        assert_eq!(names(&i.search("장치", 10)), vec!["장치 관리자"]);
    }

    #[test]
    fn latin_case_insensitive_and_acronym() {
        let i = index(&["Windows Terminal", "Visual Studio Code", "Notepad"]);
        assert_eq!(names(&i.search("wt", 10))[0], "Windows Terminal");
        assert_eq!(names(&i.search("vsc", 10))[0], "Visual Studio Code");
        assert_eq!(names(&i.search("NOTE", 10))[0], "Notepad");
    }

    #[test]
    fn prefix_beats_scattered() {
        let i = index(&["Remote Desktop", "Code", "Visual Studio Code"]);
        assert_eq!(names(&i.search("code", 10))[0], "Code");
    }

    #[test]
    fn alias_matches_without_highlight() {
        let mut i = AppIndex::new(None);
        i.set_entries(vec![entry("장치 관리자", &["device", "manager", "devmgmt"])]);
        let h = i.search("devmgmt", 10);
        assert_eq!(names(&h), vec!["장치 관리자"]);
        assert!(h[0].indices.is_empty());
    }

    #[test]
    fn indices_point_into_the_original_name() {
        let i = index(&["장치 관리자"]);
        let h = i.search("관리", 10);
        assert_eq!(h[0].indices, vec![3, 4]);
    }

    fn flat(h: &[AppHit]) -> Vec<(String, bool)> {
        h.iter().map(|x| (x.name.clone(), x.recent)).collect()
    }

    #[test]
    fn empty_query_lists_recent_first_then_everything_by_name() {
        let mut i = index(&["b", "a", "c"]);
        i.record_use("c");
        i.record_use("b");
        let want: Vec<(String, bool)> = [("b", true), ("c", true), ("a", false), ("b", false), ("c", false)]
            .iter()
            .map(|(n, r)| (n.to_string(), *r))
            .collect();
        assert_eq!(flat(&i.search("", 10)), want);
    }

    #[test]
    fn recent_section_is_capped_and_filtered_by_query() {
        let all: Vec<String> = (0..8).map(|n| format!("app{n}")).chain(["other".to_string()]).collect();
        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
        let mut i = index(&refs);
        for n in &refs {
            i.record_use(n);
        }
        let h = i.search("", 100);
        assert_eq!(h.iter().filter(|x| x.recent).count(), RECENT_LIMIT);
        let h = i.search("app", 100);
        assert!(h.iter().all(|x| x.name.starts_with("app")));
    }

    #[test]
    fn duplicates_and_blanks_are_dropped() {
        let mut i = AppIndex::new(None);
        i.set_entries(vec![entry("Foo", &[]), entry("foo", &[]), entry("  ", &[])]);
        assert_eq!(i.len(), 1);
    }

    #[test]
    fn cpl_entries_go_through_control_exe() {
        let b = builtin_entries();
        let e = b.iter().find(|e| e.name == "전원 옵션").unwrap();
        assert_eq!((e.target.as_str(), e.args.as_str()), ("control.exe", "powercfg.cpl"));
        assert!(b.iter().all(|e| !e.target.ends_with(".cpl")));
    }

    #[test]
    fn non_apps_are_filtered() {
        assert!(is_launchable("Visual Studio Code", "Microsoft.VisualStudioCode"));
        assert!(is_launchable("서비스", "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\services.msc"));
        assert!(!is_launchable("Uninstall Node.js", "Microsoft.AutoGenerated.{66BB}"));
        assert!(!is_launchable("RemoteView 에이전트 삭제", "Microsoft.AutoGenerated.{1}"));
        assert!(!is_launchable("Vim Online", "https://www.vim.org/"));
        assert!(!is_launchable("Python Manuals", "C:\\Python\\Doc\\html\\index.html"));
        assert!(!is_launchable("7-Zip Help", "C:\\7zip\\7-zip.chm"));
        assert!(!is_launchable("Recorder", "C:\\tools\\ahk\\Recorder.ahk"));
    }

    #[test]
    fn builtin_skips_tools_missing_on_this_pc() {
        let b = builtin_with(|t| t != "gpedit.msc");
        assert!(b.iter().all(|e| e.name != "로컬 그룹 정책 편집기"));
        assert!(b.iter().any(|e| e.name == "장치 관리자"));
    }

    #[test]
    fn builtin_admin_targets() {
        let b = builtin_entries();
        let get = |n: &str| b.iter().find(|e| e.name == n).unwrap().admin.clone();
        assert_eq!(get("장치 관리자"), Some(("mmc.exe".into(), "devmgmt.msc".into())));
        assert_eq!(get("레지스트리 편집기"), Some(("regedit.exe".into(), String::new())));
        assert_eq!(get("전원 옵션"), Some(("control.exe".into(), "powercfg.cpl".into())));
        assert_eq!(get("Wi-Fi 설정"), None);
    }

    #[test]
    fn builtin_has_device_manager() {
        let b = builtin_entries();
        assert!(b.iter().any(|e| e.target == "devmgmt.msc"));
    }
}
