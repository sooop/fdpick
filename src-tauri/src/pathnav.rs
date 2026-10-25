//! Windows 경로 탐색 모드의 순수 로직 모듈.
//! 외부 크레이트 없이 std 만 사용한다.

use std::fs;
use std::io;
use std::path::Path;

/// 입력창 문자열을 해석한 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct NavInput {
    /// 부모 폴더 경로. Drives 모드이면 None.
    pub parent: Option<String>,
    /// 마지막 구분자 뒤의 이름 필터.
    pub filter: String,
    /// 현재 탐색 종류.
    pub mode: NavKind,
}

/// 탐색 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavKind {
    /// 드라이브 목록.
    Drives,
    /// 드라이브 루트(`D:\`) 의 폴더 목록.
    DriveRoot,
    /// 드라이브 아래 임의 폴더.
    Dir,
}

/// 폴더 또는 드라이브 항목.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: String,
}

/// 구분자(`\` 또는 `/`) 판정.
fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

/// 드라이브 목록 모드 결과를 만든다. 필터는 입력 전체.
fn drives_input(input: &str) -> NavInput {
    NavInput {
        parent: None,
        filter: input.to_string(),
        mode: NavKind::Drives,
    }
}

/// 드라이브 루트 모드 결과를 만든다.
fn drive_root(letter: char) -> NavInput {
    NavInput {
        parent: Some(format!("{}:\\", letter.to_ascii_uppercase())),
        filter: String::new(),
        mode: NavKind::DriveRoot,
    }
}

/// 입력 문자열을 탐색 상태로 해석한다.
pub fn parse_input(input: &str) -> NavInput {
    if input.is_empty() {
        return NavInput {
            parent: None,
            filter: String::new(),
            mode: NavKind::Drives,
        };
    }
    // 빈 문자열은 위에서 걸렀으므로 첫 글자는 반드시 있다.
    let first = input.chars().next().unwrap();
    if !first.is_ascii_alphabetic() {
        return drives_input(input);
    }
    // 드라이브 문자는 ASCII 한 바이트이므로 잘라내도 안전하다.
    let rest = &input[first.len_utf8()..];
    if rest.is_empty() {
        return drive_root(first);
    }
    let after = match rest.strip_prefix(':') {
        Some(a) => a,
        None => return drives_input(input),
    };
    if after.is_empty() || after == "\\" || after == "/" {
        return drive_root(first);
    }
    if !after.starts_with(is_sep) {
        return drives_input(input);
    }
    // 여기서 after 는 구분자로 시작한다. 마지막 구분자 기준으로 부모와 필터를 나눈다.
    let idx = after.rfind(is_sep).unwrap();
    let body = &after[..idx];
    let filter = &after[idx + 1..];
    // 연속 구분자와 선행 구분자는 빈 조각으로 보고 버린다.
    let parts: Vec<&str> = body.split(is_sep).filter(|s| !s.is_empty()).collect();
    let root = format!("{}:\\", first.to_ascii_uppercase());
    let parent = if parts.is_empty() {
        root
    } else {
        format!("{}{}", root, parts.join("\\"))
    };
    let mode = if parts.is_empty() && filter.is_empty() {
        NavKind::DriveRoot
    } else {
        NavKind::Dir
    };
    NavInput {
        parent: Some(parent),
        filter: filter.to_string(),
        mode,
    }
}

/// 부모 폴더의 직계 폴더를 읽는다. 파일은 제외하고 숨김 폴더는 포함한다.
/// 부모가 드라이브 루트(`X:\`)일 때만 `deny_top` 이름을 대소문자 무시로 제외한다.
/// 결과는 이름의 대소문자 무시 사전순이다.
pub fn list_dirs(parent: &str, deny_top: &[&str]) -> io::Result<Vec<Entry>> {
    let base_norm = parent.replace('/', "\\");
    let base = base_norm.trim_end_matches('\\');
    // 드라이브 루트 판정: "X:" 형태(구분자 제거 후)일 때만 참이다.
    let at_root = base.len() == 2
        && base.as_bytes()[0].is_ascii_alphabetic()
        && base.as_bytes()[1] == b':';

    let mut out = Vec::new();
    for dent in fs::read_dir(parent)? {
        let dent = dent?;
        let name = dent.file_name().to_string_lossy().into_owned();
        // 심볼릭 링크와 정션은 대상을 따라가서 디렉터리인지 확인한다.
        let is_dir = match dent.file_type() {
            Ok(ft) if ft.is_dir() => true,
            _ => fs::metadata(dent.path()).map(|m| m.is_dir()).unwrap_or(false),
        };
        if !is_dir {
            continue;
        }
        if at_root {
            let lower = name.to_lowercase();
            if deny_top.iter().any(|d| d.to_lowercase() == lower) {
                continue;
            }
        }
        let path = format!("{}\\{}", base, name);
        out.push(Entry { name, path });
    }
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// A:~Z: 중 루트 경로가 존재하는 드라이브를 나열한다. name 은 `D:`, path 는 `D:\`.
pub fn list_drives() -> Vec<Entry> {
    (b'A'..=b'Z')
        .filter_map(|c| {
            let name = format!("{}:", c as char);
            let path = format!("{}\\", name);
            if Path::new(&path).exists() {
                Some(Entry { name, path })
            } else {
                None
            }
        })
        .collect()
}

/// 이름 길이 짧은 순, 같으면 소문자 사전순으로 정렬한다.
fn sort_group(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        a.name
            .chars()
            .count()
            .cmp(&b.name.chars().count())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 이름으로 항목을 거른다. 접두사 일치가 앞, 부분 문자열 일치가 뒤에 온다.
/// 필터가 비었으면 입력 순서를 그대로 돌려준다.
pub fn filter_entries(entries: Vec<Entry>, filter: &str) -> Vec<Entry> {
    if filter.is_empty() {
        return entries;
    }
    let f = filter.to_lowercase();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    for e in entries {
        let lower = e.name.to_lowercase();
        if lower.starts_with(&f) {
            prefix.push(e);
        } else if lower.contains(&f) {
            contains.push(e);
        }
    }
    sort_group(&mut prefix);
    sort_group(&mut contains);
    prefix.extend(contains);
    prefix
}

/// 선택한 항목으로 입력창 문자열을 완성한다. 항상 `\` 구분자이고 끝에 `\` 한 개를 붙인다.
/// 드라이브 루트(`D:\`)는 중복 없이 `D:\` 가 된다. `_input` 의 구분자 형태는 무시한다.
pub fn complete(_input: &str, picked: &Entry) -> String {
    let p = picked.path.replace('/', "\\");
    let base = p.trim_end_matches('\\');
    format!("{}\\", base)
}

/// Enter 시 강조 항목이 없을 때 복사할 현재 폴더 경로.
/// Dir 또는 DriveRoot 이고 필터가 비었을 때만 Some 이다.
/// 드라이브 루트는 `D:\`, 그 외는 끝 구분자 없는 `D:\projects` 형태다.
pub fn current_dir_of(input: &str) -> Option<String> {
    let p = parse_input(input);
    if !p.filter.is_empty() {
        return None;
    }
    match p.mode {
        NavKind::Drives => None,
        NavKind::DriveRoot | NavKind::Dir => p.parent.map(|s| {
            if s.ends_with(":\\") {
                s
            } else {
                s.trim_end_matches('\\').to_string()
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn nav(parent: Option<&str>, filter: &str, mode: NavKind) -> NavInput {
        NavInput {
            parent: parent.map(|s| s.to_string()),
            filter: filter.to_string(),
            mode,
        }
    }

    fn ent(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            path: format!("D:\\{}", name),
        }
    }

    fn names(v: &[Entry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    // ---------- parse_input ----------

    #[test]
    fn parse_empty_is_drives() {
        assert_eq!(parse_input(""), nav(None, "", NavKind::Drives));
    }

    #[test]
    fn parse_drive_letter_only() {
        let want = nav(Some("D:\\"), "", NavKind::DriveRoot);
        assert_eq!(parse_input("d"), want);
        assert_eq!(parse_input("D"), want);
    }

    #[test]
    fn parse_drive_with_colon() {
        assert_eq!(parse_input("d:"), nav(Some("D:\\"), "", NavKind::DriveRoot));
    }

    #[test]
    fn parse_drive_root_with_separator() {
        let want = nav(Some("D:\\"), "", NavKind::DriveRoot);
        assert_eq!(parse_input("d:/"), want);
        assert_eq!(parse_input("D:\\"), want);
    }

    #[test]
    fn parse_drive_then_name() {
        assert_eq!(parse_input("d:\\proj"), nav(Some("D:\\"), "proj", NavKind::Dir));
        assert_eq!(parse_input("d:/proj"), nav(Some("D:\\"), "proj", NavKind::Dir));
    }

    #[test]
    fn parse_trailing_separator_is_dir_with_empty_filter() {
        assert_eq!(
            parse_input("d:\\projects\\"),
            nav(Some("D:\\projects"), "", NavKind::Dir)
        );
    }

    #[test]
    fn parse_deep_path() {
        assert_eq!(parse_input("D:\\a\\b\\c"), nav(Some("D:\\a\\b"), "c", NavKind::Dir));
    }

    #[test]
    fn parse_no_drive_prefix_goes_to_drives_with_whole_filter() {
        assert_eq!(parse_input("proj"), nav(None, "proj", NavKind::Drives));
        assert_eq!(parse_input("dd"), nav(None, "dd", NavKind::Drives));
    }

    #[test]
    fn parse_mixed_separators() {
        assert_eq!(
            parse_input("d:/a\\b/"),
            nav(Some("D:\\a\\b"), "", NavKind::Dir)
        );
    }

    #[test]
    fn parse_collapses_repeated_separators() {
        assert_eq!(parse_input("d:\\\\"), nav(Some("D:\\"), "", NavKind::DriveRoot));
        assert_eq!(parse_input("d:\\\\a\\\\b"), nav(Some("D:\\a"), "b", NavKind::Dir));
    }

    // ---------- filter_entries ----------

    #[test]
    fn filter_empty_keeps_order() {
        let v = vec![ent("zeta"), ent("alpha")];
        assert_eq!(names(&filter_entries(v, "")), vec!["zeta", "alpha"]);
    }

    #[test]
    fn filter_prefix_before_contains_then_length_then_alpha() {
        let v = vec![
            ent("xa"),
            ent("abcd"),
            ent("xab"),
            ent("abc"),
            ent("ab"),
        ];
        assert_eq!(
            names(&filter_entries(v, "a")),
            vec!["ab", "abc", "abcd", "xa", "xab"]
        );
    }

    #[test]
    fn filter_is_case_insensitive() {
        let v = vec![ent("my proj"), ent("Projects"), ent("proj-x"), ent("other")];
        assert_eq!(
            names(&filter_entries(v, "PROJ")),
            vec!["proj-x", "Projects", "my proj"]
        );
    }

    #[test]
    fn filter_korean() {
        let v = vec![ent("가나다"), ent("한글"), ent("한글 폴더"), ent("abc")];
        assert_eq!(names(&filter_entries(v.clone(), "한")), vec!["한글", "한글 폴더"]);
        assert_eq!(names(&filter_entries(v, "나")), vec!["가나다"]);
    }

    #[test]
    fn filter_no_match_is_empty() {
        let v = vec![ent("abc")];
        assert!(filter_entries(v, "zzz").is_empty());
    }

    // ---------- complete ----------

    #[test]
    fn complete_normal_dir_adds_single_trailing_separator() {
        let e = Entry {
            name: "projects".into(),
            path: "D:\\projects".into(),
        };
        assert_eq!(complete("d:\\pro", &e), "D:\\projects\\");
    }

    #[test]
    fn complete_drive_root_no_duplicate_separator() {
        let e = Entry {
            name: "D:".into(),
            path: "D:\\".into(),
        };
        assert_eq!(complete("d", &e), "D:\\");
        assert_eq!(complete("d:/", &e), "D:\\");
    }

    #[test]
    fn complete_normalizes_slash_in_picked_path() {
        let e = Entry {
            name: "x".into(),
            path: "D:/a/x".into(),
        };
        // 입력의 구분자 형태와 무관하게 항상 `\` 이다.
        assert_eq!(complete("d:/a/", &e), "D:\\a\\x\\");
        assert_eq!(complete("d:\\a\\", &e), "D:\\a\\x\\");
    }

    // ---------- current_dir_of ----------

    #[test]
    fn current_dir_of_cases() {
        assert_eq!(current_dir_of(""), None);
        assert_eq!(current_dir_of("proj"), None);
        assert_eq!(current_dir_of("d"), Some("D:\\".to_string()));
        assert_eq!(current_dir_of("D:"), Some("D:\\".to_string()));
        assert_eq!(current_dir_of("d:/"), Some("D:\\".to_string()));
        assert_eq!(current_dir_of("d:\\proj"), None);
        assert_eq!(current_dir_of("d:\\projects\\"), Some("D:\\projects".to_string()));
        assert_eq!(current_dir_of("D:\\a\\b\\c"), None);
        assert_eq!(current_dir_of("d:/a\\b/"), Some("D:\\a\\b".to_string()));
    }

    // ---------- list_dirs / list_drives (실제 파일시스템) ----------

    struct TempGuard(std::path::PathBuf);

    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 고유 임시 디렉터리를 만든다. 한글·숨김·공백 폴더 3개와 파일 1개를 넣는다.
    /// 반환된 가드가 드롭되면 디렉터리를 지운다.
    fn make_fixture(tag: &str) -> (TempGuard, String) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "fdpick_pathnav_{}_{}_{}",
            tag,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&dir).unwrap();
        for d in ["한글 폴더", ".hidden", "with space"] {
            fs::create_dir(dir.join(d)).unwrap();
        }
        fs::write(dir.join("file.txt"), b"x").unwrap();
        let base = dir.to_string_lossy().trim_end_matches('\\').to_string();
        (TempGuard(dir), base)
    }

    #[test]
    fn list_dirs_folders_only_including_hidden_korean_space() {
        let (_g, base) = make_fixture("dirs");
        let v = list_dirs(&base, &[]).unwrap();
        assert_eq!(names(&v), vec![".hidden", "with space", "한글 폴더"]);
        let e = v.iter().find(|e| e.name == "with space").unwrap();
        assert_eq!(e.path, format!("{}\\with space", base));
    }

    #[test]
    fn list_dirs_non_root_ignores_deny_list() {
        let (_g, base) = make_fixture("deny");
        let v = list_dirs(&base, &["with space", "windows"]).unwrap();
        assert!(v.iter().any(|e| e.name == "with space"));
    }

    #[test]
    fn list_dirs_missing_parent_is_err() {
        let (_g, base) = make_fixture("missing");
        let missing = format!("{}\\no_such_dir_xyz", base);
        assert!(list_dirs(&missing, &[]).is_err());
    }

    #[test]
    fn list_dirs_root_denies_top_names_case_insensitive() {
        // C:\ 가 없는 환경에서는 검사를 건너뛴다.
        if !Path::new("C:\\").exists() {
            return;
        }
        let all = list_dirs("C:\\", &[]).unwrap();
        let v = list_dirs("C:\\", &["windows"]).unwrap();
        // 원본 목록에 Windows 가 있을 때만 제외 효과를 단언한다.
        if all.iter().any(|e| e.name.eq_ignore_ascii_case("windows")) {
            assert!(v.iter().all(|e| !e.name.eq_ignore_ascii_case("windows")));
        }
        for e in &v {
            assert_eq!(e.path, format!("C:\\{}", e.name));
        }
    }

    #[test]
    fn list_drives_entries_are_existing_roots() {
        let ds = list_drives();
        for d in &ds {
            assert_eq!(d.name.len(), 2);
            assert_eq!(d.path, format!("{}\\", d.name));
            assert!(Path::new(&d.path).exists());
        }
        if Path::new("C:\\").exists() {
            assert!(ds.iter().any(|d| d.name == "C:"));
        }
    }
}
