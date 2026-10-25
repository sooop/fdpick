//! 경로 항목 동작: 클립보드 복사, 탐색기에서 보기, 열기.
//! 모든 경로는 먼저 `normalize_path` 로 정규화한다.

use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// CF_UNICODETEXT 클립보드 형식 번호
const CF_UNICODETEXT: u32 = 13;

/// 복사 서식 옵션 (1차 구현에서는 기본값만 사용)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CopyOptions {
    /// 각 경로를 큰따옴표로 감싼다
    pub quote: bool,
    /// `\` 를 `/` 로 바꾼다
    pub forward_slash: bool,
}

/// 연속 `\` 를 하나로 줄인다.
fn collapse_seps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in s.chars() {
        if ch == '\\' {
            if prev_sep {
                continue;
            }
            prev_sep = true;
        } else {
            prev_sep = false;
        }
        out.push(ch);
    }
    out
}

/// 경로 문자열 정규화.
pub fn normalize_path(raw: &str) -> String {
    let mut s = raw.trim().replace('/', "\\");
    // 확장 길이 접두사 제거
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        s = format!("\\\\{}", rest);
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        s = rest.to_string();
    }
    let s = s.trim();
    // 연속 구분자 축약 (UNC 접두 `\\` 는 보존)
    let mut out = match s.strip_prefix("\\\\") {
        Some(rest) => format!("\\\\{}", collapse_seps(rest.trim_start_matches('\\'))),
        None => collapse_seps(s),
    };
    // 끝 구분자 제거 (드라이브 루트 유지)
    let is_drive_root = |t: &str| t.len() == 3 && t.as_bytes()[1] == b':' && t.ends_with('\\');
    while out.len() > 1 && out.ends_with('\\') && !is_drive_root(&out) {
        out.pop();
    }
    // `C:` 단독이면 `C:\`
    let b = out.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        out.push('\\');
    }
    out
}

fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 클립보드에 유니코드 문자열을 쓴다.
fn set_clipboard_text(text: &str) -> Result<(), String> {
    let wide = to_wide(text);
    let bytes = wide.len() * 2;
    unsafe {
        // 다른 앱이 쥐고 있을 수 있어 10ms 간격으로 최대 10회 재시도
        let mut opened = false;
        for i in 0..10 {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
            if i < 9 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        if !opened {
            return Err(
                "클립보드를 열 수 없습니다. 다른 프로그램이 사용 중일 수 있으니 잠시 후 다시 시도하세요."
                    .into(),
            );
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return Err("클립보드를 비우지 못했습니다.".to_string());
            }
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if hmem.is_null() {
                return Err("클립보드용 메모리를 할당하지 못했습니다.".to_string());
            }
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                GlobalFree(hmem);
                return Err("클립보드용 메모리를 잠그지 못했습니다.".to_string());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(hmem);
            let h: HANDLE = SetClipboardData(CF_UNICODETEXT, hmem as HANDLE);
            if h.is_null() {
                // 소유권이 넘어가지 않았으므로 직접 해제
                GlobalFree(hmem);
                return Err("클립보드에 데이터를 쓰지 못했습니다.".to_string());
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}

/// explorer.exe 에 넘길 원본 인자 문자열 생성 (순수 함수)
pub fn explorer_args(path: &str, is_dir: bool) -> String {
    if is_dir {
        format!("\"{}\"", path)
    } else {
        format!("/select,\"{}\"", path)
    }
}

fn spawn_explorer(path: &str, is_dir: bool) -> Result<(), String> {
    // 종료 코드가 0 이 아니어도 정상일 수 있으므로 spawn 성공만 확인하고 기다리지 않는다
    Command::new("explorer.exe")
        .raw_arg(explorer_args(path, is_dir))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("탐색기를 실행하지 못했습니다: {}", e))
}

fn shell_open(path: &str) -> Result<(), String> {
    let verb = to_wide("open");
    let file = to_wide(path);
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if r <= 32 {
        Err(format!(
            "파일을 열지 못했습니다 (ShellExecute 오류 코드 {}): {}",
            r, path
        ))
    } else {
        Ok(())
    }
}

/// copy_dir 용 경로 변환: 디렉터리면 그대로, 그 외(파일/미존재)는 부모 폴더, 부모가 없으면 자기 자신
pub fn to_dir_path(p: &str) -> String {
    let path = Path::new(p);
    if path.is_dir() {
        return p.to_string();
    }
    match path.parent().and_then(|x| x.to_str()) {
        Some(parent) if !parent.is_empty() => normalize_path(parent),
        _ => p.to_string(),
    }
}

fn format_text(items: &[String], opts: &CopyOptions) -> String {
    items
        .iter()
        .map(|p| {
            let mut s = p.clone();
            if opts.forward_slash {
                s = s.replace('\\', "/");
            }
            if opts.quote {
                s = format!("\"{}\"", s);
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

pub fn run(kind: &str, paths: &[String]) -> Result<(), String> {
    run_with(kind, paths, &CopyOptions::default())
}

pub fn run_with(kind: &str, paths: &[String], opts: &CopyOptions) -> Result<(), String> {
    let norm: Vec<String> = paths
        .iter()
        .map(|p| normalize_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    if norm.is_empty() {
        return Err("대상 경로가 없습니다.".into());
    }
    match kind {
        "copy" => set_clipboard_text(&format_text(&norm, opts)),
        "copy_dir" => {
            let mut dirs: Vec<String> = Vec::new();
            for p in &norm {
                let d = to_dir_path(p);
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
            set_clipboard_text(&format_text(&dirs, opts))
        }
        "reveal" | "open" => {
            let p = &norm[0];
            let path = Path::new(p);
            if !path.exists() {
                return Err(format!("경로가 존재하지 않습니다: {}", p));
            }
            let is_dir = path.is_dir();
            if kind == "open" && !is_dir {
                shell_open(p)
            } else {
                spawn_explorer(p, is_dir)
            }
        }
        other => Err(format!("지원하지 않는 동작입니다: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::DataExchange::GetClipboardData;
    use windows_sys::Win32::System::Memory::GlobalSize;

    fn read_clipboard() -> Result<String, String> {
        unsafe {
            let mut opened = false;
            for _ in 0..20 {
                if OpenClipboard(std::ptr::null_mut()) != 0 {
                    opened = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            if !opened {
                return Err("클립보드 열기 실패".into());
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                CloseClipboard();
                return Err("클립보드 데이터 없음".into());
            }
            let p = GlobalLock(h) as *const u16;
            let n = GlobalSize(h) / 2;
            let sl = std::slice::from_raw_parts(p, n);
            let end = sl.iter().position(|&c| c == 0).unwrap_or(n);
            let s = String::from_utf16_lossy(&sl[..end]);
            GlobalUnlock(h);
            CloseClipboard();
            Ok(s)
        }
    }

    #[test]
    fn normalize_trailing_and_root() {
        assert_eq!(normalize_path("C:\\a\\b\\"), "C:\\a\\b");
        assert_eq!(normalize_path("C:\\"), "C:\\");
        assert_eq!(normalize_path("C:"), "C:\\");
        assert_eq!(normalize_path("c:/"), "c:\\");
        assert_eq!(normalize_path("C:\\\\"), "C:\\");
    }

    #[test]
    fn normalize_prefix_unc() {
        assert_eq!(normalize_path("\\\\?\\C:\\a\\"), "C:\\a");
        assert_eq!(
            normalize_path("\\\\?\\UNC\\서버\\공유\\x\\"),
            "\\\\서버\\공유\\x"
        );
        assert_eq!(normalize_path("\\\\?\\UNC\\서버\\공유"), "\\\\서버\\공유");
        assert_eq!(normalize_path("\\\\srv\\share\\"), "\\\\srv\\share");
        assert_eq!(normalize_path("\\\\srv\\\\share\\\\a"), "\\\\srv\\share\\a");
        assert_eq!(normalize_path("//srv/share/a/"), "\\\\srv\\share\\a");
    }

    #[test]
    fn normalize_mixed_and_spaces() {
        assert_eq!(normalize_path("C:/a/b\\c"), "C:\\a\\b\\c");
        assert_eq!(normalize_path("C:\\a\\\\b\\\\\\c"), "C:\\a\\b\\c");
        assert_eq!(normalize_path("  C:\\a  "), "C:\\a");
        assert_eq!(normalize_path("   "), "");
        assert_eq!(
            normalize_path("D:/자료/내 문서/파일 [1].txt/"),
            "D:\\자료\\내 문서\\파일 [1].txt"
        );
    }

    #[test]
    fn copy_dir_conversion() {
        let base = std::env::temp_dir().join(format!("fdpick_actions_{}", std::process::id()));
        let sub = base.join("하위 폴더");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("파일 [1].txt");
        std::fs::write(&file, b"x").unwrap();
        let sub_s = normalize_path(sub.to_str().unwrap());
        let file_s = normalize_path(file.to_str().unwrap());
        assert_eq!(to_dir_path(&sub_s), sub_s);
        assert_eq!(to_dir_path(&file_s), sub_s);
        let missing = format!("{}\\없는\\x.bin", sub_s);
        assert_eq!(to_dir_path(&missing), format!("{}\\없는", sub_s));
        let missing_noext = format!("{}\\없는폴더", sub_s);
        assert_eq!(to_dir_path(&missing_noext), sub_s);
        assert_eq!(to_dir_path("C:\\"), "C:\\");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn explorer_args_format() {
        assert_eq!(
            explorer_args("C:\\a b\\c [1].txt", false),
            "/select,\"C:\\a b\\c [1].txt\""
        );
        assert_eq!(explorer_args("C:\\a b", true), "\"C:\\a b\"");
    }

    #[test]
    fn errors() {
        assert!(run("copy", &[]).is_err());
        assert!(run("copy", &["  ".into()]).is_err());
        assert!(run("bogus", &["C:\\".into()]).is_err());
        let e = run("reveal", &["C:\\__no_such_path_fdpick__".into()]).unwrap_err();
        assert!(e.contains("경로가 존재하지 않습니다"));
        assert!(run("open", &["C:\\__no_such_path_fdpick__".into()]).is_err());
    }

    #[test]
    fn format_options() {
        let v = vec!["C:\\a b\\c".to_string(), "D:\\d".to_string()];
        let q = CopyOptions {
            quote: true,
            forward_slash: false,
        };
        assert_eq!(format_text(&v, &q), "\"C:\\a b\\c\"\r\n\"D:\\d\"");
        let f = CopyOptions {
            quote: false,
            forward_slash: true,
        };
        assert_eq!(format_text(&v, &f), "C:/a b/c\r\nD:/d");
    }

    // 클립보드는 전역 자원이므로 한 테스트 안에서 순차 실행
    #[test]
    fn clipboard_roundtrip() {
        let multi = vec![
            "C:\\한글 폴더\\파일 [1].txt".to_string(),
            "D:/다른 경로/".to_string(),
            "\\\\?\\E:\\x y\\z".to_string(),
        ];
        run("copy", &multi).expect("복사 실패");
        assert_eq!(
            read_clipboard().unwrap(),
            "C:\\한글 폴더\\파일 [1].txt\r\nD:\\다른 경로\r\nE:\\x y\\z"
        );

        run("copy", &["C:\\단일 항목\\a b.txt".to_string()]).unwrap();
        assert_eq!(read_clipboard().unwrap(), "C:\\단일 항목\\a b.txt");

        run_with(
            "copy",
            &["C:\\a b".to_string()],
            &CopyOptions {
                quote: true,
                forward_slash: true,
            },
        )
        .unwrap();
        assert_eq!(read_clipboard().unwrap(), "\"C:/a b\"");

        // copy_dir 왕복: 같은 폴더의 파일 둘은 중복 제거
        let base = std::env::temp_dir().join(format!("fdpick_cb_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let f1 = base.join("a.txt");
        let f2 = base.join("b.txt");
        std::fs::write(&f1, b"1").unwrap();
        std::fs::write(&f2, b"2").unwrap();
        run(
            "copy_dir",
            &[f1.to_str().unwrap().into(), f2.to_str().unwrap().into()],
        )
        .unwrap();
        assert_eq!(
            read_clipboard().unwrap(),
            normalize_path(base.to_str().unwrap())
        );
        std::fs::remove_dir_all(&base).ok();
    }

    // 수동 확인용: 임시 디렉터리 대상으로 explorer spawn 성공 여부만 확인 (창이 뜰 수 있음)
    #[test]
    #[ignore]
    fn manual_explorer_spawn() {
        let base = std::env::temp_dir().join(format!("fdpick_ex_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let f = base.join("a b [1].txt");
        std::fs::write(&f, b"x").unwrap();
        run("reveal", &[f.to_str().unwrap().into()]).expect("spawn 실패");
    }
}
