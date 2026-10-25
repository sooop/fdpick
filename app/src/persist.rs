//! `%APPDATA%\fdpick\state.json` 읽기/쓰기. 형식: `{"mode":"fuzzy"}` 또는 `{"mode":"path"}`.
//! serde_json 의존성을 피하려고 이 한 가지 키만 직접 다룬다.

use std::path::PathBuf;

use fdpick_core::engine::Mode;

fn state_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    if appdata.is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join("fdpick").join("state.json"))
}

/// 저장된 모드. 파일이 없거나 깨졌거나 최근 모드면 None.
pub fn load_mode() -> Option<Mode> {
    let text = std::fs::read_to_string(state_path()?).ok()?;
    parse_mode(&text)
}

fn parse_mode(text: &str) -> Option<Mode> {
    let i = text.find("\"mode\"")?;
    let rest = text[i + 6..].trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    match &rest[..end] {
        "fuzzy" => Some(Mode::Fuzzy),
        "path" => Some(Mode::Path),
        _ => None,
    }
}

/// 마지막 사용 모드를 저장한다. 최근 모드는 저장하지 않는다. 실패는 조용히 무시한다.
pub fn save_mode(mode: Mode) {
    let name = match mode {
        Mode::Fuzzy => "fuzzy",
        Mode::Path => "path",
        Mode::Recent => return,
    };
    let Some(p) = state_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, format!("{{\"mode\":\"{name}\"}}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modes() {
        assert_eq!(parse_mode("{\"mode\":\"fuzzy\"}"), Some(Mode::Fuzzy));
        assert_eq!(parse_mode("{ \"mode\" : \"path\" }"), Some(Mode::Path));
        assert_eq!(parse_mode("{\"mode\":\"recent\"}"), None);
        assert_eq!(parse_mode("garbage"), None);
        assert_eq!(parse_mode("{\"mode\":"), None);
    }
}
