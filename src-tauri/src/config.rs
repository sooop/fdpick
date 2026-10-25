//! 설정 로더: `%APPDATA%\fdpick\config.toml`
//!
//! - 일부 항목만 적어도 나머지는 기본값으로 채운다.
//! - 파싱 오류·잘못된 값은 앱을 죽이지 않고 기본값으로 대체하며 `warnings` 로 알린다.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 알려진 최상위 키(오타 발견용)
const KNOWN_KEYS: &[&str] = &[
    "hotkey",
    "fd_path",
    "default_fd_args",
    "deny_top",
    "drive_types",
    "cache_ttl_minutes",
    "empty_limit",
    "result_limit",
    "recent_limit",
    "record_recent",
    "hide_on_blur",
    "theme",
    "autostart",
    "copy",
    "action",
];
/// `[[action]]` 항목의 알려진 키
const KNOWN_ACTION_KEYS: &[&str] = &[
    "name", "key", "ext", "target", "run", "mode", "cwd", "window", "close_popup",
];
/// 내장 단축키(사용자 액션이 쓸 수 없다). 정규화된 표기.
const BUILTIN_KEYS: &[&str] = &[
    "Enter",
    "Tab",
    "Escape",
    "ArrowUp",
    "ArrowDown",
    "PageUp",
    "PageDown",
    "Ctrl+P",
    "Ctrl+N",
    "Ctrl+D",
    "Ctrl+O",
    "Ctrl+R",
    "Ctrl+L",
    "Ctrl+K",
    "Ctrl+Space",
    "Alt+Enter",
    "Ctrl+A",
    "Ctrl+C",
    "Ctrl+V",
    "Ctrl+X",
    "Ctrl+Z",
    "Ctrl+Backspace",
];
/// `[copy]` 테이블의 알려진 키
const KNOWN_COPY_KEYS: &[&str] = &["quote", "forward_slash"];

const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";
const DEFAULT_TTL: u64 = 15;
const DEFAULT_EMPTY_LIMIT: usize = 2000;
const DEFAULT_RECENT_LIMIT: usize = 200;
const MAX_RECENT_LIMIT: usize = 5000;
const MAX_EMPTY_LIMIT: usize = 100_000;
const DEFAULT_RESULT_LIMIT: usize = 200;
const MAX_RESULT_LIMIT: usize = 10_000;
const DRIVE_KINDS: &[&str] = &["Fixed", "Removable", "Network"];
const THEMES: &[&str] = &["system", "light", "dark"];

/// 복사 동작 설정(`[copy]`)
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CopyConfig {
    /// 경로를 큰따옴표로 감싼다
    pub quote: bool,
    /// 역슬래시 대신 슬래시를 쓴다
    pub forward_slash: bool,
}

impl Default for CopyConfig {
    fn default() -> Self {
        Self { quote: false, forward_slash: false }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub fd_path: String,
    pub default_fd_args: Vec<String>,
    pub deny_top: Vec<String>,
    pub drive_types: Vec<String>,
    pub cache_ttl_minutes: u64,
    pub empty_limit: usize,
    pub result_limit: usize,
    pub recent_limit: usize,
    pub record_recent: bool,
    pub hide_on_blur: bool,
    pub theme: String,
    pub autostart: bool,
    pub copy: CopyConfig,
    /// 사용자 정의 액션(`[[action]]` 테이블 배열)
    #[serde(rename = "action")]
    pub actions: Vec<ActionConfig>,
}

fn default_target() -> String {
    "any".to_string()
}
fn default_batch_mode() -> String {
    "each".to_string()
}
fn default_true() -> bool {
    true
}

/// 사용자 정의 액션 한 개(`[[action]]`)
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ActionConfig {
    /// 메뉴에 보일 이름(필수, 비면 제외)
    pub name: String,
    /// 단축키 "Ctrl+Shift+V" 형식. 비면 메뉴로만 실행
    #[serde(default)]
    pub key: String,
    /// 허용 확장자(점 없이, 대소문자 무시). 비면 제한 없음
    #[serde(default)]
    pub ext: Vec<String>,
    /// "file" | "dir" | "any"(기본)
    #[serde(default = "default_target")]
    pub target: String,
    /// [실행파일, 인자...]. 인자에 {path} {dir} {name} {stem} {ext} {paths} 치환
    pub run: Vec<String>,
    /// "each"(항목마다 1회, 기본) | "batch"(선택 전체를 1회, {paths} 확장)
    #[serde(default = "default_batch_mode")]
    pub mode: String,
    /// 작업 폴더(템플릿 치환 가능, 비면 대상의 폴더)
    #[serde(default)]
    pub cwd: String,
    /// true 면 콘솔 창을 보인다(기본 false = 창 없음)
    #[serde(default)]
    pub window: bool,
    /// 실행 후 팝업을 닫는다
    #[serde(default = "default_true")]
    pub close_popup: bool,
}

/// 단축키 문자열을 정규화한다("ctrl+shift+v" -> "Ctrl+Shift+V").
/// 수식키는 Ctrl, Alt, Shift 순서로 고정. Ctrl 또는 Alt 가 없으면 거부한다.
pub fn normalize_key(raw: &str) -> Result<String, String> {
    let parts: Vec<&str> = raw.split('+').map(|p| p.trim()).collect();
    // "Ctrl++" 처럼 키가 '+' 인 경우: 마지막 두 조각이 빈 문자열
    let n = parts.len();
    let (mods, key_raw): (&[&str], &str) = if n >= 2 && parts[n - 1].is_empty() && parts[n - 2].is_empty() {
        (&parts[..n - 2], "+")
    } else {
        (&parts[..n - 1], parts[n - 1])
    };
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            other => return Err(format!("알 수 없는 수식키 \"{other}\"")),
        }
    }
    if key_raw.is_empty() {
        return Err("키 이름이 없습니다".to_string());
    }
    let key = canonical_key_name(key_raw)
        .ok_or_else(|| format!("알 수 없는 키 이름 \"{key_raw}\""))?;
    if !ctrl && !alt {
        return Err("Ctrl 또는 Alt 수식키가 하나 이상 필요합니다(입력과 충돌)".to_string());
    }
    let mut out = String::new();
    if ctrl {
        out.push_str("Ctrl+");
    }
    if alt {
        out.push_str("Alt+");
    }
    if shift {
        out.push_str("Shift+");
    }
    out.push_str(&key);
    Ok(out)
}

/// 키 이름을 정규 표기로 바꾼다. 모르는 이름이면 None.
fn canonical_key_name(k: &str) -> Option<String> {
    let mut chars = k.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        // 한 글자: 영문은 대문자, 숫자·ASCII 기호는 그대로
        if first.is_ascii_alphabetic() {
            return Some(first.to_ascii_uppercase().to_string());
        }
        if first.is_ascii_digit() || first.is_ascii_punctuation() {
            return Some(first.to_string());
        }
        return None;
    }
    let lower = k.to_ascii_lowercase();
    // F1~F24 ("F01" 같은 표기는 받지 않는다)
    if let Some(num) = lower.strip_prefix('f') {
        if num.bytes().all(|b| b.is_ascii_digit()) && !num.is_empty() {
            return match num.parse::<u32>() {
                Ok(n) if (1..=24).contains(&n) && n.to_string() == num => Some(format!("F{n}")),
                _ => None,
            };
        }
    }
    let alias = match lower.as_str() {
        "esc" => Some("Escape"),
        "return" => Some("Enter"),
        "del" => Some("Delete"),
        "ins" => Some("Insert"),
        "up" => Some("ArrowUp"),
        "down" => Some("ArrowDown"),
        "left" => Some("ArrowLeft"),
        "right" => Some("ArrowRight"),
        "pgup" => Some("PageUp"),
        "pgdn" => Some("PageDown"),
        _ => None,
    };
    if let Some(a) = alias {
        return Some(a.to_string());
    }
    const NAMES: &[&str] = &[
        "Enter", "Tab", "Escape", "Space", "Backspace", "Delete", "Insert", "Home", "End",
        "PageUp", "PageDown", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
    ];
    NAMES.iter().find(|n| n.eq_ignore_ascii_case(&lower)).map(|n| n.to_string())
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            fd_path: String::new(),
            default_fd_args: strs(&[
                "--hidden",
                "--exclude",
                ".git",
                "--exclude",
                "node_modules",
                "--exclude",
                "__pycache__",
                "--exclude",
                "AppData",
            ]),
            deny_top: strs(&[
                "Windows",
                "Program Files",
                "Program Files (x86)",
                "ProgramData",
                "$Recycle.Bin",
                "System Volume Information",
                "Recovery",
                "PerfLogs",
                "$WinREAgent",
                "Config.Msi",
                "Documents and Settings",
                "$SysReset",
            ]),
            drive_types: strs(&["Fixed"]),
            cache_ttl_minutes: DEFAULT_TTL,
            empty_limit: DEFAULT_EMPTY_LIMIT,
            result_limit: DEFAULT_RESULT_LIMIT,
            recent_limit: DEFAULT_RECENT_LIMIT,
            record_recent: true,
            hide_on_blur: true,
            theme: "system".to_string(),
            autostart: false,
            copy: CopyConfig::default(),
            actions: Vec::new(),
        }
    }
}

impl Config {
    /// 값을 검증하고 잘못된 항목을 보정한다. 보정 내역을 한국어 경고로 돌려준다.
    pub fn sanitize(&mut self) -> Vec<String> {
        let mut w = Vec::new();

        // drive_types: 대소문자 무시, 정규화 + 중복 제거
        let mut kinds: Vec<String> = Vec::new();
        for raw in std::mem::take(&mut self.drive_types) {
            match DRIVE_KINDS.iter().find(|k| k.eq_ignore_ascii_case(raw.trim())) {
                Some(k) => {
                    if !kinds.iter().any(|x| x == k) {
                        kinds.push(k.to_string());
                    }
                }
                None => w.push(format!(
                    "drive_types 의 알 수 없는 값 \"{raw}\" 을(를) 제거했습니다 (허용: Fixed, Removable, Network)"
                )),
            }
        }
        if kinds.is_empty() {
            kinds.push("Fixed".to_string());
            w.push("drive_types 가 비어 있어 [\"Fixed\"] 로 대체했습니다".to_string());
        }
        self.drive_types = kinds;

        // theme
        let t = self.theme.trim().to_ascii_lowercase();
        if THEMES.contains(&t.as_str()) {
            self.theme = t;
        } else {
            w.push(format!(
                "theme \"{}\" 은(는) 알 수 없는 값이라 \"system\" 으로 대체했습니다 (허용: system, light, dark)",
                self.theme
            ));
            self.theme = "system".to_string();
        }

        if self.cache_ttl_minutes == 0 {
            w.push(format!(
                "cache_ttl_minutes 는 1 이상이어야 합니다(0 은 잘못된 값). {DEFAULT_TTL} 으로 대체했습니다"
            ));
            self.cache_ttl_minutes = DEFAULT_TTL;
        }

        if self.result_limit == 0 || self.result_limit > MAX_RESULT_LIMIT {
            w.push(format!(
                "result_limit {} 은(는) 범위(1~{MAX_RESULT_LIMIT})를 벗어나 기본값 {DEFAULT_RESULT_LIMIT} 으로 대체했습니다",
                self.result_limit
            ));
            self.result_limit = DEFAULT_RESULT_LIMIT;
        }
        if self.recent_limit == 0 || self.recent_limit > MAX_RECENT_LIMIT {
            w.push(format!(
                "recent_limit {} 은(는) 범위(1~{MAX_RECENT_LIMIT})를 벗어나 기본값 {DEFAULT_RECENT_LIMIT} 으로 대체했습니다",
                self.recent_limit
            ));
            self.recent_limit = DEFAULT_RECENT_LIMIT;
        }

        if self.empty_limit == 0 {
            w.push(format!(
                "empty_limit 0 은 잘못된 값이라 기본값 {DEFAULT_EMPTY_LIMIT} 으로 대체했습니다"
            ));
            self.empty_limit = DEFAULT_EMPTY_LIMIT;
        } else if self.empty_limit > MAX_EMPTY_LIMIT {
            w.push(format!(
                "empty_limit {} 이(가) 상한을 넘어 {MAX_EMPTY_LIMIT} 으로 제한했습니다",
                self.empty_limit
            ));
            self.empty_limit = MAX_EMPTY_LIMIT;
        }

        if self.hotkey.trim().is_empty() {
            w.push(format!("hotkey 가 비어 있어 기본값 \"{DEFAULT_HOTKEY}\" 을 사용합니다"));
            self.hotkey = DEFAULT_HOTKEY.to_string();
        }

        self.sanitize_actions(&mut w);

        w
    }

    /// `[[action]]` 검증·정규화
    fn sanitize_actions(&mut self, w: &mut Vec<String>) {
        let mut used_keys: Vec<String> = Vec::new();
        let mut kept: Vec<ActionConfig> = Vec::new();
        for (i, mut a) in std::mem::take(&mut self.actions).into_iter().enumerate() {
            if a.name.trim().is_empty() {
                w.push(format!("action #{}: name 이 비어 있어 제외했습니다", i + 1));
                continue;
            }
            let label = format!("action \"{}\"", a.name);
            if a.run.is_empty() || a.run[0].trim().is_empty() {
                w.push(format!("{label}: run 이 비어 있어 제외했습니다"));
                continue;
            }

            let t = a.target.trim().to_ascii_lowercase();
            if ["file", "dir", "any"].contains(&t.as_str()) {
                a.target = t;
            } else {
                w.push(format!(
                    "{label}: target \"{}\" 은(는) 알 수 없는 값이라 \"any\" 로 대체했습니다 (허용: file, dir, any)",
                    a.target
                ));
                a.target = default_target();
            }
            let m = a.mode.trim().to_ascii_lowercase();
            if ["each", "batch"].contains(&m.as_str()) {
                a.mode = m;
            } else {
                w.push(format!(
                    "{label}: mode \"{}\" 은(는) 알 수 없는 값이라 \"each\" 로 대체했습니다 (허용: each, batch)",
                    a.mode
                ));
                a.mode = default_batch_mode();
            }

            // 확장자: 앞의 '.' 제거, 소문자, 빈 값·중복 제거
            let mut exts: Vec<String> = Vec::new();
            for e in std::mem::take(&mut a.ext) {
                let e = e.trim().trim_start_matches('.').to_ascii_lowercase();
                if !e.is_empty() && !exts.contains(&e) {
                    exts.push(e);
                }
            }
            a.ext = exts;

            // 단축키
            if a.key.trim().is_empty() {
                a.key.clear();
            } else {
                match normalize_key(&a.key) {
                    Ok(k) if BUILTIN_KEYS.contains(&k.as_str()) => {
                        w.push(format!(
                            "{label}: key \"{}\" 은(는) 내장 단축키와 충돌해 비웠습니다",
                            a.key
                        ));
                        a.key.clear();
                    }
                    Ok(k) if used_keys.contains(&k) => {
                        w.push(format!("{label}: key \"{k}\" 이(가) 앞의 액션과 중복돼 비웠습니다"));
                        a.key.clear();
                    }
                    Ok(k) => {
                        used_keys.push(k.clone());
                        a.key = k;
                    }
                    Err(e) => {
                        w.push(format!("{label}: key \"{}\" 을(를) 비웠습니다: {e}", a.key));
                        a.key.clear();
                    }
                }
            }

            if a.mode == "batch" {
                if !a.run.iter().any(|x| x.contains("{paths}")) {
                    w.push(format!(
                        "{label}: mode = \"batch\" 인데 run 에 {{paths}} 가 없습니다(선택 전체가 전달되지 않음)"
                    ));
                } else if !a.run.iter().skip(1).any(|x| x == "{paths}") {
                    w.push(format!(
                        "{label}: {{paths}} 는 인자 하나가 정확히 \"{{paths}}\" 일 때만 여러 인자로 확장됩니다"
                    ));
                }
            }
            kept.push(a);
        }
        self.actions = kept;
    }
}

/// 로드 결과
#[derive(Debug, Clone, PartialEq)]
pub struct LoadResult {
    pub config: Config,
    pub warnings: Vec<String>,
    /// 파일이 없어서 기본값으로 시작했다(생성 성공 여부와 무관)
    pub created: bool,
}

/// `%APPDATA%\fdpick\config.toml`. APPDATA 가 없거나 비어 있으면 None.
pub fn config_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    if appdata.is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join("fdpick").join("config.toml"))
}

/// 기본 설정 파일 텍스트(한국어 주석 포함). 파싱하면 `Config::default()` 와 같다.
pub fn default_toml() -> String {
    r#"# fdpick 설정 파일
# 지운 항목은 기본값이 적용됩니다. 잘못된 값은 경고와 함께 기본값으로 대체됩니다.

# 팝업을 여는 전역 단축키
hotkey = "Ctrl+Alt+Space"

# fd 실행 파일 경로. 비워 두면 PATH 에서 찾습니다.
fd_path = ""

# fd 에 항상 붙이는 인자(인자 단위로 나눠서 적습니다)
default_fd_args = ["--hidden", "--exclude", ".git", "--exclude", "node_modules", "--exclude", "__pycache__", "--exclude", "AppData"]

# 드라이브 최상위에서 건너뛸 폴더 이름(대소문자 무시)
deny_top = [
    "Windows",
    "Program Files",
    "Program Files (x86)",
    "ProgramData",
    "$Recycle.Bin",
    "System Volume Information",
    "Recovery",
    "PerfLogs",
    "$WinREAgent",
    "Config.Msi",
    "Documents and Settings",
    "$SysReset",
]

# 검색할 드라이브 종류: Fixed(고정), Removable(이동식), Network(네트워크). 대소문자 무시
drive_types = ["Fixed"]

# 검색 결과 캐시 유지 시간(분). 1 이상이어야 합니다(0 은 캐시 끔이 아니라 잘못된 값).
cache_ttl_minutes = 15

# 검색어가 비었을 때 보여 줄 최대 항목 수 (1~100000)
empty_limit = 2000

# 검색어가 있을 때 보여 줄 최대 결과 수 (1~10000)
result_limit = 200

# 최근 사용 목록에 보관할 최대 경로 수 (1~5000)
recent_limit = 200

# 복사/열기한 경로를 최근 사용 목록에 기록한다
record_recent = true

# 팝업이 포커스를 잃으면 자동으로 숨긴다
hide_on_blur = true

# 테마: system | light | dark
theme = "system"

# 로그인 시 자동 시작
autostart = false

# 경로 복사 동작
[copy]
# 경로를 큰따옴표로 감싼다
quote = false
# 역슬래시(\) 대신 슬래시(/)를 쓴다
forward_slash = false

# 사용자 정의 액션: [[action]] 을 여러 개 적을 수 있습니다. 셸을 거치지 않고 인자 배열로 직접 실행합니다.
#   name        메뉴에 보일 이름(필수)
#   key         단축키 예) "Ctrl+Shift+V". Ctrl 또는 Alt 필수. 내장 키·다른 액션과 겹치면 무시됩니다. 비우면 메뉴로만 실행
#   ext         허용 확장자(점 없이, 대소문자 무시). 비우면 제한 없음. 폴더에는 적용되지 않습니다
#   target      "file" | "dir" | "any"(기본)
#   run         [실행파일, 인자...]. 실행파일은 PATH 에서 찾으며 템플릿 치환을 하지 않습니다
#               인자 치환: {path} 전체 경로, {dir} 폴더, {name} 이름, {stem} 확장자 뺀 이름, {ext} 확장자(점 없음)
#               {paths} 는 인자 하나가 정확히 "{paths}" 일 때만 선택한 모든 경로로 확장됩니다(다른 문자열과 섞으면 확장되지 않음)
#   mode        "each"(항목마다 1회 실행, 기본) | "batch"(선택 전체를 1회 실행, {path} 류는 첫 경로 기준)
#   cwd         작업 폴더(템플릿 가능). 비우면 대상의 폴더
#   window      true 면 콘솔 창을 보입니다(기본 false)
#   close_popup 실행 후 팝업을 닫는다(기본 true)
#
# 예시 1: .psd 파일을 특정 프로그램으로 열기(항목마다 1회)
# [[action]]
# name = "Photoshop 으로 열기"
# key = "Ctrl+Shift+P"
# ext = ["psd"]
# target = "file"
# run = ["C:\\Program Files\\Adobe\\Photoshop\\Photoshop.exe", "{path}"]
#
# 예시 2: 선택한 파일 목록을 스크립트에 한 번에 넘기기
# [[action]]
# name = "목록 처리 스크립트"
# mode = "batch"
# window = true
# run = ["powershell.exe", "-NoProfile", "-File", "C:\\scripts\\process.ps1", "{paths}"]
"#
    .to_string()
}

/// 기본 경로에서 설정을 읽는다(없으면 생성).
pub fn load_or_create() -> LoadResult {
    match config_path() {
        Some(p) => load_from(&p, true),
        None => LoadResult {
            config: Config::default(),
            warnings: vec![
                "APPDATA 환경변수를 찾지 못해 기본 설정으로 시작합니다".to_string(),
            ],
            created: false,
        },
    }
}

/// 바이트 오프셋을 (줄, 열)로 바꾼다(1부터, 열은 문자 단위).
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut off = offset.min(text.len());
    while !text.is_char_boundary(off) {
        off -= 1;
    }
    let before = &text[..off];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line, before[start..].chars().count() + 1)
}

/// 지정 경로에서 설정을 읽는다.
pub fn load_from(path: &Path, create_if_missing: bool) -> LoadResult {
    let mut warnings = Vec::new();

    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if create_if_missing {
                if let Some(dir) = path.parent() {
                    if let Err(e) = std::fs::create_dir_all(dir) {
                        warnings.push(format!(
                            "설정 폴더를 만들지 못했습니다 ({}): {e}",
                            dir.display()
                        ));
                    }
                }
                if let Err(e) = std::fs::write(path, default_toml()) {
                    warnings.push(format!(
                        "기본 설정 파일을 쓰지 못했습니다 ({}): {e}",
                        path.display()
                    ));
                }
            }
            return LoadResult { config: Config::default(), warnings, created: true };
        }
        Err(e) => {
            warnings.push(format!(
                "설정 파일을 읽지 못해 기본값을 사용합니다 ({}): {e}",
                path.display()
            ));
            return LoadResult { config: Config::default(), warnings, created: false };
        }
    };

    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => {
            warnings.push(format!(
                "설정 파일이 UTF-8 이 아니라 기본값을 사용합니다 ({}): 유효하지 않은 바이트 위치 {}",
                path.display(),
                e.utf8_error().valid_up_to()
            ));
            return LoadResult { config: Config::default(), warnings, created: false };
        }
    };
    // 메모장 등이 붙이는 BOM 제거
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);

    let mut config = match toml::from_str::<Config>(text) {
        Ok(c) => c,
        Err(e) => {
            let loc = match e.span() {
                Some(s) => {
                    let (l, c) = line_col(text, s.start);
                    format!("{l}줄 {c}열: ")
                }
                None => String::new(),
            };
            warnings.push(format!(
                "설정 파일 해석 오류, 전체 기본값을 사용합니다 ({}) {loc}{}",
                path.display(),
                e.message()
            ));
            return LoadResult { config: Config::default(), warnings, created: false };
        }
    };

    // 알 수 없는 키(오타 발견용): 경고만 하고 무시
    if let Ok(table) = toml::from_str::<toml::Table>(text) {
        for k in table.keys() {
            if !KNOWN_KEYS.contains(&k.as_str()) {
                warnings.push(format!("알 수 없는 설정 키 \"{k}\" 은(는) 무시합니다"));
            }
        }
        if let Some(toml::Value::Table(c)) = table.get("copy") {
            for k in c.keys() {
                if !KNOWN_COPY_KEYS.contains(&k.as_str()) {
                    warnings.push(format!("알 수 없는 설정 키 \"copy.{k}\" 은(는) 무시합니다"));
                }
            }
        }
        // [[action]] 항목의 알 수 없는 키
        if let Some(toml::Value::Array(items)) = table.get("action") {
            for (i, it) in items.iter().enumerate() {
                if let toml::Value::Table(t) = it {
                    for k in t.keys() {
                        if !KNOWN_ACTION_KEYS.contains(&k.as_str()) {
                            warnings.push(format!(
                                "알 수 없는 설정 키 \"action[{}].{k}\" 은(는) 무시합니다",
                                i + 1
                            ));
                        }
                    }
                }
            }
        }
    }

    warnings.extend(config.sanitize());
    LoadResult { config, warnings, created: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 테스트별 고유 임시 디렉터리
    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fdpick_cfg_test_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn load_text(text: &str) -> LoadResult {
        let d = tmp();
        let p = d.join("config.toml");
        std::fs::write(&p, text).unwrap();
        let r = load_from(&p, false);
        let _ = std::fs::remove_dir_all(&d);
        r
    }

    #[test]
    fn default_toml_equals_default() {
        let c: Config = toml::from_str(&default_toml()).expect("default_toml 은 유효한 TOML");
        assert_eq!(c, Config::default());
        // sanitize 도 경고 없이 통과
        let mut c2 = c.clone();
        assert!(c2.sanitize().is_empty());
        assert_eq!(c2, c);
    }

    #[test]
    fn partial_file() {
        let r = load_text("theme = \"dark\"\nresult_limit = 50\n[copy]\nquote = true\n");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(!r.created);
        assert_eq!(r.config.theme, "dark");
        assert_eq!(r.config.result_limit, 50);
        assert!(r.config.copy.quote);
        assert!(!r.config.copy.forward_slash);
        assert_eq!(r.config.hotkey, "Ctrl+Alt+Space");
        assert_eq!(r.config.deny_top, Config::default().deny_top);
    }

    #[test]
    fn empty_file() {
        let r = load_text("");
        assert_eq!(r.config, Config::default());
        assert!(r.warnings.is_empty());
        assert!(!r.created);
    }

    #[test]
    fn missing_file_creates() {
        let d = tmp();
        let p = d.join("sub").join("fdpick").join("config.toml");
        let r = load_from(&p, true);
        assert!(r.created);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.config, Config::default());
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, default_toml());
        let r2 = load_from(&p, true);
        assert!(!r2.created);
        assert_eq!(r2.config, Config::default());
        assert!(r2.warnings.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_no_create() {
        let d = tmp();
        let p = d.join("nope").join("config.toml");
        let r = load_from(&p, false);
        assert!(r.created);
        assert!(!p.exists());
        assert_eq!(r.config, Config::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn create_failure_is_warning_only() {
        let d = tmp();
        // 부모가 "파일"이라 디렉터리 생성이 실패한다
        let blocker = d.join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let r = load_from(&blocker.join("config.toml"), true);
        assert!(r.created);
        assert!(!r.warnings.is_empty());
        assert_eq!(r.config, Config::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn parse_error_has_location() {
        let r = load_text("hotkey = \"x\"\ntheme = = \"dark\"\n");
        assert_eq!(r.config, Config::default());
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("2줄"), "{}", r.warnings[0]);
        assert!(r.warnings[0].contains("열"), "{}", r.warnings[0]);
    }

    #[test]
    fn type_error_falls_back() {
        let r = load_text("cache_ttl_minutes = \"abc\"\n");
        assert_eq!(r.config, Config::default());
        assert_eq!(r.warnings.len(), 1);
        let r = load_text("result_limit = -5\n");
        assert_eq!(r.config, Config::default());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn invalid_utf8_falls_back() {
        let d = tmp();
        let p = d.join("config.toml");
        std::fs::write(&p, [b'a', b' ', 0xFF, 0xFE, b'\n']).unwrap();
        let r = load_from(&p, false);
        assert_eq!(r.config, Config::default());
        assert_eq!(r.warnings.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bom_is_ignored() {
        let r = load_text("\u{FEFF}theme = \"light\"\n");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.config.theme, "light");
    }

    #[test]
    fn sanitize_bad_values() {
        let mut c = Config {
            drive_types: vec!["fixed".into(), "CD".into(), "NETWORK".into(), "Fixed".into()],
            theme: "neon".into(),
            cache_ttl_minutes: 0,
            result_limit: 10_001,
            empty_limit: 100_001,
            hotkey: "  ".into(),
            ..Config::default()
        };
        let w = c.sanitize();
        assert_eq!(w.len(), 6, "{w:?}");
        assert_eq!(c.drive_types, vec!["Fixed", "Network"]);
        assert_eq!(c.theme, "system");
        assert_eq!(c.cache_ttl_minutes, 15);
        assert_eq!(c.result_limit, 200);
        assert_eq!(c.empty_limit, 100_000);
        assert_eq!(c.hotkey, "Ctrl+Alt+Space");

        let mut c = Config {
            drive_types: vec!["x".into()],
            result_limit: 0,
            empty_limit: 0,
            ..Config::default()
        };
        let w = c.sanitize();
        assert_eq!(w.len(), 4, "{w:?}"); // 알 수 없는 값 + 비어서 대체 + result + empty
        assert_eq!(c.drive_types, vec!["Fixed"]);
        assert_eq!(c.result_limit, 200);
        assert_eq!(c.empty_limit, 2000);
    }

    #[test]
    fn sanitize_via_file() {
        let r = load_text("theme = \"neon\"\ndrive_types = [\"Removable\", \"zip\"]\n");
        assert_eq!(r.config.theme, "system");
        assert_eq!(r.config.drive_types, vec!["Removable"]);
        assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    }

    #[test]
    fn unknown_keys_warn() {
        let r = load_text("thme = \"dark\"\nresult_limit = 30\n[copy]\nqoute = true\n[extra]\na = 1\n");
        assert_eq!(r.config.result_limit, 30);
        assert_eq!(r.config.theme, "system");
        assert!(r.warnings.iter().any(|w| w.contains("thme")), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("copy.qoute")), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("extra")), "{:?}", r.warnings);
        assert_eq!(r.warnings.len(), 3);
    }

    #[test]
    fn korean_values() {
        let r = load_text(
            "fd_path = \"C:\\\\도구\\\\fd.exe\"\n# 주석 한글\ndeny_top = [\"사용자 폴더\", \"임시\"]\ndefault_fd_args = [\"--exclude\", \"빌드결과\"]\n",
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.config.fd_path, "C:\\도구\\fd.exe");
        assert_eq!(r.config.deny_top, vec!["사용자 폴더", "임시"]);
        assert_eq!(r.config.default_fd_args, vec!["--exclude", "빌드결과"]);
    }

    #[test]
    fn korean_error_column_counts_chars() {
        let (l, c) = line_col("가나다 = =\n", "가나다 = ".len());
        assert_eq!((l, c), (1, 7));
    }

    #[test]
    fn copy_table() {
        let r = load_text("[copy]\nquote = true\nforward_slash = true\n");
        assert!(r.warnings.is_empty());
        assert!(r.config.copy.quote && r.config.copy.forward_slash);
        let r = load_text("[copy]\n");
        assert_eq!(r.config.copy, CopyConfig::default());
    }

    #[test]
    fn serialize_roundtrip() {
        let c = Config::default();
        let s = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn config_path_shape() {
        if let Some(p) = config_path() {
            assert!(p.ends_with(Path::new("fdpick").join("config.toml")));
        }
    }

    fn act(name: &str, key: &str) -> ActionConfig {
        ActionConfig {
            name: name.into(),
            key: key.into(),
            ext: vec![],
            target: "any".into(),
            run: vec!["x.exe".into(), "{path}".into()],
            mode: "each".into(),
            cwd: String::new(),
            window: false,
            close_popup: true,
        }
    }

    fn sanitized(actions: Vec<ActionConfig>) -> (Vec<ActionConfig>, Vec<String>) {
        let mut c = Config { actions, ..Config::default() };
        let w = c.sanitize();
        (c.actions, w)
    }

    #[test]
    fn default_has_no_actions_and_example_is_commented() {
        assert!(Config::default().actions.is_empty());
        assert!(default_toml().contains("# [[action]]"));
        assert!(!default_toml().lines().any(|l| l.trim_start().starts_with("[[action]]")));
    }

    #[test]
    fn action_parse_multiple_and_defaults() {
        let r = load_text(
            "[[action]]\nname = \"a\"\nrun = [\"x.exe\"]\n\n[[action]]\nname = \"b\"\nrun = [\"y.exe\", \"{paths}\"]\nmode = \"batch\"\nwindow = true\nclose_popup = false\ncwd = \"{dir}\"\next = [\"png\"]\ntarget = \"file\"\nkey = \"Ctrl+Shift+B\"\n",
        );
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.config.actions.len(), 2);
        let a = &r.config.actions[0];
        assert_eq!(a.target, "any");
        assert_eq!(a.mode, "each");
        assert!(a.close_popup);
        assert!(!a.window);
        assert!(a.key.is_empty() && a.ext.is_empty() && a.cwd.is_empty());
        let b = &r.config.actions[1];
        assert_eq!(b.mode, "batch");
        assert!(b.window && !b.close_popup);
        assert_eq!(b.cwd, "{dir}");
        assert_eq!(b.target, "file");
        assert_eq!(b.key, "Ctrl+Shift+B");
    }

    #[test]
    fn action_missing_required_field_falls_back_whole_config() {
        let r = load_text("theme = \"dark\"\n[[action]]\nname = \"a\"\n");
        assert_eq!(r.config, Config::default());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn action_unknown_key_warns() {
        let r = load_text("[[action]]\nname = \"a\"\nrun = [\"x\"]\nwindw = true\n");
        assert_eq!(r.config.actions.len(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("action[1].windw")), "{:?}", r.warnings);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn action_korean_name() {
        let r = load_text("[[action]]\nname = \"포토샵으로 열기\"\nrun = [\"ps.exe\", \"{path}\"]\n");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.config.actions[0].name, "포토샵으로 열기");
    }

    #[test]
    fn action_sanitize_removes_invalid() {
        let mut no_run = act("norun", "");
        no_run.run.clear();
        let (v, w) = sanitized(vec![act("  ", ""), no_run, act("ok", "")]);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "ok");
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn action_sanitize_target_mode() {
        let mut a = act("a", "");
        a.target = "folder".into();
        a.mode = "all".into();
        let mut b = act("b", "");
        b.target = " DIR ".into();
        b.mode = "Batch".into();
        b.run = vec!["x".into(), "{paths}".into()];
        let (v, w) = sanitized(vec![a, b]);
        assert_eq!((v[0].target.as_str(), v[0].mode.as_str()), ("any", "each"));
        assert_eq!((v[1].target.as_str(), v[1].mode.as_str()), ("dir", "batch"));
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn action_sanitize_ext() {
        let mut a = act("a", "");
        a.ext = vec![".PNG".into(), "jpg".into(), "".into(), ".".into(), "Png".into(), " .Gif ".into()];
        let (v, w) = sanitized(vec![a]);
        assert_eq!(v[0].ext, vec!["png", "jpg", "gif"]);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn key_normalization() {
        let ok = |s: &str| normalize_key(s).unwrap();
        assert_eq!(ok("ctrl+shift+v"), "Ctrl+Shift+V");
        assert_eq!(ok("Shift+Alt+Ctrl+x"), "Ctrl+Alt+Shift+X");
        assert_eq!(ok(" Ctrl + 1 "), "Ctrl+1");
        assert_eq!(ok("alt+f5"), "Alt+F5");
        assert_eq!(ok("Ctrl+F24"), "Ctrl+F24");
        assert_eq!(ok("ctrl+arrowup"), "Ctrl+ArrowUp");
        assert_eq!(ok("ctrl+up"), "Ctrl+ArrowUp");
        assert_eq!(ok("ctrl+pagedown"), "Ctrl+PageDown");
        assert_eq!(ok("CTRL+SPACE"), "Ctrl+Space");
        assert_eq!(ok("Ctrl++"), "Ctrl++");
        assert!(normalize_key("Ctrl+F25").is_err());
        assert!(normalize_key("Ctrl+F0").is_err());
        assert!(normalize_key("Ctrl+F05").is_err());
        assert!(normalize_key("Ctrl+Foo").is_err());
        assert!(normalize_key("Win+V").is_err());
        assert!(normalize_key("Ctrl+").is_err());
        assert!(normalize_key("V").is_err());
        assert!(normalize_key("Shift+V").is_err());
        assert!(normalize_key("F5").is_err());
        assert!(normalize_key("Shift+F5").is_err());
    }

    #[test]
    fn action_sanitize_keys() {
        let (v, w) = sanitized(vec![
            act("a", "ctrl+shift+v"),
            act("b", "V"),
            act("c", "Shift+V"),
            act("d", "Ctrl+P"),
            act("e", "alt+enter"),
            act("f", "Ctrl+Shift+V"),
            act("g", "Ctrl+Alt+G"),
            act("h", "ctrl+backspace"),
            act("i", "Ctrl+Foo"),
            act("j", "Ctrl+Shift+P"),
        ]);
        let keys: Vec<&str> = v.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, vec!["Ctrl+Shift+V", "", "", "", "", "", "Ctrl+Alt+G", "", "", "Ctrl+Shift+P"]);
        assert_eq!(w.len(), 7, "{w:?}");
        assert!(w.iter().any(|x| x.contains("내장")));
        assert!(w.iter().any(|x| x.contains("중복")));
    }

    #[test]
    fn action_batch_without_paths_warns() {
        let mut a = act("a", "");
        a.mode = "batch".into();
        let (v, w) = sanitized(vec![a.clone()]);
        assert_eq!(v.len(), 1);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("{paths}"));

        // {paths} 가 다른 문자열과 섞이면 확장되지 않음을 경고
        a.run = vec!["x".into(), "--list={paths}".into()];
        let (_, w) = sanitized(vec![a.clone()]);
        assert_eq!(w.len(), 1, "{w:?}");

        a.run = vec!["x".into(), "{paths}".into()];
        let (_, w) = sanitized(vec![a]);
        assert!(w.is_empty(), "{w:?}");
    }

    #[test]
    fn action_roundtrip_serialize() {
        let c = Config { actions: vec![act("한글", "Ctrl+Alt+H")], ..Config::default() };
        let s = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back, c);
    }
}
