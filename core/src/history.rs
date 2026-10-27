//! 검색 패턴 이력 저장소(`%APPDATA%\fdpick\search_history.json`).
//!
//! 형식: `{"version":1,"items":["최신", ..., "오래됨"]}`. 목록은 컨트롤러가 관리하고 여기서는 읽고 쓰기만 한다.
//! 읽기 실패나 형식 오류는 빈 목록으로, 쓰기 실패는 조용히 무시한다.

use std::path::{Path, PathBuf};

const FILE_VERSION: u64 = 1;

/// `%APPDATA%\fdpick\search_history.json`. APPDATA 가 없거나 비어 있으면 None.
pub fn default_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    if appdata.is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join("fdpick").join("search_history.json"))
}

/// 최신이 앞인 이력을 읽는다. 빈 항목은 버리고 `limit` 개까지만 돌려준다.
pub fn load(path: &Path, limit: usize) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(&bytes);
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { return Vec::new() };
    v.get("items")
        .and_then(|i| i.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str())
                .filter(|s| !s.trim().is_empty())
                .take(limit)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 이력을 저장한다(임시 파일에 쓰고 rename). 실패는 무시한다.
pub fn save(path: &Path, items: &[String]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::json!({ "version": FILE_VERSION, "items": items }).to_string();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("fdpick_hist_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_limit_and_garbage() {
        let d = tmp();
        let p = d.join("sub").join("h.json");
        assert!(load(&p, 10).is_empty());
        let items: Vec<String> = ["c 한글", "b", "a"].iter().map(|s| s.to_string()).collect();
        save(&p, &items);
        assert_eq!(load(&p, 10), items);
        assert_eq!(load(&p, 2), items[..2].to_vec());
        assert!(load(&p, 0).is_empty());
        save(&p, &[]);
        assert!(load(&p, 10).is_empty());
        std::fs::write(&p, "not json").unwrap();
        assert!(load(&p, 10).is_empty());
        std::fs::write(&p, r#"{"items":["x"," ",1,"y"]}"#).unwrap();
        assert_eq!(load(&p, 10), vec!["x", "y"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
