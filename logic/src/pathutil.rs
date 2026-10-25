//! 경로 문자열 유틸(src/pathutil.ts 와 main.ts 의 작은 순수 함수 포팅).
//! 인덱스는 모두 char(코드포인트) 단위다.

/// 하이라이트 구간. 같은 hit 값이 이어진 글자를 하나로 묶는다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub hit: bool,
}

/// `[A-Za-z]` 한 글자인가
fn is_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
}

/// 정규식 `^[A-Za-z]:?$` (드라이브 문자만, 콜론은 선택)
pub fn is_drive_letter(t: &str) -> bool {
    let mut it = t.chars();
    match (it.next(), it.next(), it.next()) {
        (Some(a), None, _) => is_letter(a),
        (Some(a), Some(':'), None) => is_letter(a),
        _ => false,
    }
}

/// `D:` 또는 `D:\` 형태의 드라이브 루트인가.
pub fn is_drive_root(p: &str) -> bool {
    let mut it = p.chars();
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(a), Some(':'), None, _) => is_letter(a),
        (Some(a), Some(':'), Some('\\'), None) => is_letter(a),
        _ => false,
    }
}

/// 끝에 `\` 가 없으면 붙인다(`D:\` 는 중복하지 않는다).
pub fn ensure_trailing_sep(p: &str) -> String {
    if p.ends_with('\\') {
        p.to_string()
    } else {
        format!("{p}\\")
    }
}

/// 복사 대상 경로 정규화: `/` -> `\`, 끝 구분자 제거, 드라이브 루트는 `C:\` 유지.
/// 빈 입력이면 빈 문자열.
pub fn normalize_copy_path(text: &str) -> String {
    let mut t: String = text.trim().replace('/', "\\");
    if t.is_empty() {
        return String::new();
    }
    // 드라이브 문자만(`d`, `d:`, `d:\\`)이면 루트로 취급: ^[A-Za-z]:?\\*$
    {
        let mut it = t.chars();
        let first = it.next().unwrap_or(' ');
        if is_letter(first) {
            let rest: String = it.collect();
            let rest = rest.strip_prefix(':').unwrap_or(&rest);
            if rest.chars().all(|c| c == '\\') {
                return format!("{}:\\", first.to_ascii_uppercase());
            }
        }
    }
    // UNC 선두 `\\` 는 보존하고 끝 구분자만 제거한다(길이 1 이하가 되면 멈춘다).
    while t.chars().count() > 1 && t.ends_with('\\') {
        t.pop();
    }
    t
}

/// Shift+Tab 부모 폴더 계산. 드라이브 루트이거나 조각이 하나뿐이면 빈 문자열(드라이브 목록).
pub fn parent_input(text: &str) -> String {
    let t = text.replace('/', "\\");
    let t = t.trim_end_matches('\\');
    if t.is_empty() || is_drive_letter(t) {
        return String::new();
    }
    match t.rfind('\\') {
        Some(idx) => t[..idx + 1].to_string(),
        None => String::new(),
    }
}

/// 항목이 속한 폴더 경로(끝 구분자 없음, 단 드라이브 루트는 `D:\`).
pub fn folder_of(path: &str, is_dir: bool) -> String {
    if is_dir {
        return path.to_string();
    }
    let chars: Vec<char> = path.chars().collect();
    let Some(idx) = chars.iter().rposition(|&c| c == '\\') else {
        return path.to_string();
    };
    // `D:\file.txt` 처럼 드라이브 루트 바로 아래이면 `D:\` 를 유지한다.
    if idx == 2 && chars[1] == ':' {
        return chars[..=idx].iter().collect();
    }
    chars[..idx].iter().collect()
}

/// 입력 끝의 이름 필터(마지막 구분자 뒤). 구분자가 없으면 입력 전체.
pub fn typed_filter(v: &str) -> String {
    match v.rfind(['\\', '/']) {
        Some(i) => v[i + 1..].to_string(),
        None => v.to_string(),
    }
}

/// 끝이 `\` 또는 `/` 인가(정규식 `[\\/]$`)
pub fn ends_with_sep(v: &str) -> bool {
    v.ends_with(['\\', '/'])
}

/// 경로 모드에서 입력이 폴더 자체를 가리키는가: trim 한 입력이 끝 구분자로 끝나거나
/// 드라이브 문자만(`d`, `d:`)이다. 빈 입력은 false.
pub fn points_at_folder(input: &str) -> bool {
    let t = input.trim();
    !t.is_empty() && (ends_with_sep(t) || is_drive_letter(t))
}

fn runs(chars: &[char], from: usize, to: usize, hits: &[u32]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut buf = String::new();
    let mut cur = false;
    for (i, &c) in chars.iter().enumerate().take(to).skip(from) {
        let h = hits.contains(&(i as u32));
        if !buf.is_empty() && h != cur {
            out.push(Seg { text: std::mem::take(&mut buf), hit: cur });
        }
        cur = h;
        buf.push(c);
    }
    if !buf.is_empty() {
        out.push(Seg { text: buf, hit: cur });
    }
    out
}

/// 코드포인트 단위로 경로를 상위 경로(dir)/파일명(name) 구간으로 나누고 매치 인덱스를 분배한다.
pub fn split_highlight(path: &str, name_start: usize, indices: &[u32]) -> (Vec<Seg>, Vec<Seg>) {
    let chars: Vec<char> = path.chars().collect();
    let ns = name_start.min(chars.len());
    (runs(&chars, 0, ns, indices), runs(&chars, ns, chars.len(), indices))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_root() {
        assert!(is_drive_root("D:"));
        assert!(is_drive_root("d:\\"));
        assert!(!is_drive_root("D:\\x"));
        assert!(!is_drive_root("D"));
        assert!(!is_drive_root("D:/"));
        assert!(!is_drive_root("가:"));
        assert!(!is_drive_root(""));
    }

    #[test]
    fn trailing_sep() {
        assert_eq!(ensure_trailing_sep("D:\\"), "D:\\");
        assert_eq!(ensure_trailing_sep("D:\\work"), "D:\\work\\");
        assert_eq!(ensure_trailing_sep("D:/work"), "D:/work\\");
        assert_eq!(ensure_trailing_sep("C:\\문서"), "C:\\문서\\");
    }

    #[test]
    fn copy_path() {
        assert_eq!(normalize_copy_path(""), "");
        assert_eq!(normalize_copy_path("   "), "");
        assert_eq!(normalize_copy_path("d"), "D:\\");
        assert_eq!(normalize_copy_path("d:"), "D:\\");
        assert_eq!(normalize_copy_path("d:\\"), "D:\\");
        assert_eq!(normalize_copy_path("d:/"), "D:\\");
        assert_eq!(normalize_copy_path("d:\\\\"), "D:\\");
        assert_eq!(normalize_copy_path("C:/apps/bin/"), "C:\\apps\\bin");
        assert_eq!(normalize_copy_path(" C:\\apps\\bin\\\\ "), "C:\\apps\\bin");
        assert_eq!(normalize_copy_path("C:/문서/보고서/"), "C:\\문서\\보고서");
        assert_eq!(normalize_copy_path("\\\\srv\\share\\"), "\\\\srv\\share");
        assert_eq!(normalize_copy_path("\\\\"), "\\");
        assert_eq!(normalize_copy_path("ab"), "ab");
    }

    #[test]
    fn parent() {
        assert_eq!(parent_input("C:\\apps\\bin"), "C:\\apps\\");
        assert_eq!(parent_input("C:\\apps\\bin\\"), "C:\\apps\\");
        assert_eq!(parent_input("C:/apps/bin/"), "C:\\apps\\");
        assert_eq!(parent_input("C:\\apps"), "C:\\");
        assert_eq!(parent_input("C:\\apps\\"), "C:\\");
        assert_eq!(parent_input("C:\\"), "");
        assert_eq!(parent_input("C:"), "");
        assert_eq!(parent_input("c"), "");
        assert_eq!(parent_input(""), "");
        assert_eq!(parent_input("\\\\"), "");
        assert_eq!(parent_input("D:\\문서\\한글폴더"), "D:\\문서\\");
        assert_eq!(parent_input("abc"), "");
    }

    #[test]
    fn folder() {
        assert_eq!(folder_of("C:\\apps\\bin", true), "C:\\apps\\bin");
        assert_eq!(folder_of("C:\\apps\\a.txt", false), "C:\\apps");
        assert_eq!(folder_of("D:\\a.txt", false), "D:\\");
        assert_eq!(folder_of("a.txt", false), "a.txt");
        assert_eq!(folder_of("D:\\문서\\보고서.pdf", false), "D:\\문서");
        assert_eq!(folder_of("가나다\\라.txt", false), "가나다");
        // idx==2 이지만 드라이브가 아닌 경우는 일반 규칙
        assert_eq!(folder_of("ab\\c.txt", false), "ab");
        assert_eq!(folder_of("가나\\c", false), "가나");
    }

    #[test]
    fn typed() {
        assert_eq!(typed_filter("C:\\apps\\b"), "b");
        assert_eq!(typed_filter("C:/apps/b"), "b");
        assert_eq!(typed_filter("C:\\apps\\"), "");
        assert_eq!(typed_filter("abc"), "abc");
        assert_eq!(typed_filter("C:\\문서\\한글"), "한글");
        assert_eq!(typed_filter("a/b\\c"), "c");
    }

    #[test]
    fn folder_pointing() {
        assert!(points_at_folder("c"));
        assert!(points_at_folder("c:"));
        assert!(points_at_folder(" C:\\apps\\ "));
        assert!(points_at_folder("C:/apps/"));
        assert!(!points_at_folder("C:\\apps\\b"));
        assert!(!points_at_folder("ab"));
        assert!(!points_at_folder(""));
        assert!(!points_at_folder("  "));
    }

    fn seg(t: &str, h: bool) -> Seg {
        Seg { text: t.to_string(), hit: h }
    }

    #[test]
    fn highlight() {
        let (d, n) = split_highlight("C:\\apps\\bin", 8, &[0, 8, 9]);
        assert_eq!(d, vec![seg("C", true), seg(":\\apps\\", false)]);
        assert_eq!(n, vec![seg("bi", true), seg("n", false)]);
        // 한글: char 인덱스 기준
        let (d, n) = split_highlight("D:\\문서\\보고서", 6, &[3, 7]);
        assert_eq!(d, vec![seg("D:\\", false), seg("문", true), seg("서\\", false)]);
        assert_eq!(n, vec![seg("보", false), seg("고", true), seg("서", false)]);
        // nameStart 범위 초과는 길이로 제한
        let (d, n) = split_highlight("ab", 10, &[]);
        assert_eq!(d, vec![seg("ab", false)]);
        assert!(n.is_empty());
        let (d, n) = split_highlight("", 0, &[]);
        assert!(d.is_empty() && n.is_empty());
    }
}
