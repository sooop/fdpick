//! 사용자 정의 액션(`[[action]]`) 적용 판정과 실행
//!
//! - 셸을 쓰지 않는다. `run` 배열을 `Command::args` 로 직접 넘긴다.
//! - 프로세스는 기다리지 않고(detached) 시작한다.
//! - 주의(Windows): `.bat`/`.cmd` 는 cmd.exe 가 인자를 따로 해석하므로 인자에 `"`, `%`, `&`, `^` 등이
//!   있으면 인용 문제가 생길 수 있다(Rust 표준 라이브러리는 안전하게 인용할 수 없으면 오류를 돌려준다).
//!   가능하면 `.exe` 나 `powershell -File` 을 쓰고, 배치 파일을 쓸 때는 경로에 특수문자가 없는지 확인한다.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::ActionConfig;

/// 콘솔 창을 만들지 않는 프로세스 생성 플래그
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 프런트엔드에 넘기는 액션 요약
#[derive(Clone, Debug, serde::Serialize)]
pub struct ActionInfo {
    /// `actions` 슬라이스에서의 위치
    pub id: usize,
    pub name: String,
    pub key: String,
    pub batch: bool,
    pub close_popup: bool,
}

fn info(id: usize, a: &ActionConfig) -> ActionInfo {
    ActionInfo {
        id,
        name: a.name.clone(),
        key: a.key.clone(),
        batch: a.mode == "batch",
        close_popup: a.close_popup,
    }
}

/// 단축키 바인딩용: 전체 액션
pub fn all(actions: &[ActionConfig]) -> Vec<ActionInfo> {
    actions.iter().enumerate().map(|(i, a)| info(i, a)).collect()
}

/// 이 선택에 적용 가능한 액션만(메뉴용)
pub fn applicable(actions: &[ActionConfig], paths: &[String]) -> Vec<ActionInfo> {
    actions
        .iter()
        .enumerate()
        .filter(|(_, a)| is_applicable(a, paths))
        .map(|(i, a)| info(i, a))
        .collect()
}

/// 경로 문자열 정규화: `/` -> `\`, `\\?\` 제거, 끝 구분자 제거(드라이브 루트 유지)
fn normalize(raw: &str) -> String {
    let mut s = raw.trim().replace('/', "\\");
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        s = format!("\\\\{rest}");
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        s = rest.to_string();
    }
    let is_drive_root = |t: &str| t.len() == 3 && t.as_bytes()[1] == b':' && t.ends_with('\\');
    while s.len() > 1 && s.ends_with('\\') && !is_drive_root(&s) {
        s.pop();
    }
    let b = s.as_bytes();
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        s.push('\\');
    }
    s
}

/// 정규화된 경로가 드라이브 루트(`D:\`)이거나 UNC 공유 루트(`\\서버\공유`)인가
fn is_root(p: &str) -> bool {
    let b = p.as_bytes();
    if b.len() == 3 && b[1] == b':' && b[2] == b'\\' {
        return true;
    }
    match p.strip_prefix("\\\\") {
        Some(rest) => rest.matches('\\').count() <= 1,
        None => false,
    }
}

/// 부모 폴더. 루트 바로 아래 항목은 `D:\`, 루트 자신은 자기 자신.
fn parent_of(p: &str) -> String {
    if is_root(p) {
        return p.to_string();
    }
    match p.rfind('\\') {
        None => String::new(),
        Some(i) => {
            let b = p.as_bytes();
            if i == 2 && b[1] == b':' {
                p[..3].to_string()
            } else {
                p[..i].to_string()
            }
        }
    }
}

/// 마지막 요소(루트는 경로 자체)
fn name_of(p: &str) -> String {
    if is_root(p) {
        return p.to_string();
    }
    match p.rfind('\\') {
        Some(i) => p[i + 1..].to_string(),
        None => p.to_string(),
    }
}

/// 이름을 (확장자 뺀 이름, 확장자)로 나눈다. `.gitignore` 처럼 점으로 시작하면 확장자 없음.
fn split_ext(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

/// 한 경로가 액션의 target/ext 조건을 만족하는가(`raw` 는 정규화 전 경로)
fn path_matches(a: &ActionConfig, raw: &str) -> bool {
    let p = normalize(raw);
    if p.is_empty() {
        return false;
    }
    let meta = std::fs::metadata(&p).ok();
    match a.target.as_str() {
        "file" => {
            if !meta.as_ref().is_some_and(|m| m.is_file()) {
                return false;
            }
        }
        "dir" => {
            if !meta.as_ref().is_some_and(|m| m.is_dir()) {
                return false;
            }
        }
        _ => {}
    }
    if !a.ext.is_empty() {
        // 디렉터리에는 확장자 조건을 적용할 수 없다
        if meta.as_ref().is_some_and(|m| m.is_dir()) {
            return false;
        }
        let name = name_of(&p);
        let (_, ext) = split_ext(&name);
        let ext = ext.to_lowercase();
        if ext.is_empty() || !a.ext.iter().any(|e| e.to_lowercase() == ext) {
            return false;
        }
    }
    true
}

fn is_applicable(a: &ActionConfig, paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| path_matches(a, p))
}

/// 템플릿 치환에 쓰는 값
struct Vars {
    path: String,
    dir: String,
    name: String,
    stem: String,
    ext: String,
}

fn vars_for(path: &str) -> Vars {
    let name = name_of(path);
    let (stem, ext) = split_ext(&name);
    Vars {
        path: path.to_string(),
        dir: parent_of(path),
        stem: stem.to_string(),
        ext: ext.to_string(),
        name,
    }
}

/// `{path} {dir} {name} {stem} {ext}` 를 치환한다. 알 수 없는 `{foo}`(와 `{paths}`)는 그대로 둔다.
/// 한 번만 훑으므로 치환된 값 안의 `{...}` 는 다시 치환되지 않는다.
fn expand(tpl: &str, v: &Vars) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let replaced = after.find('}').and_then(|j| {
            let val = match &after[..j] {
                "path" => &v.path,
                "dir" => &v.dir,
                "name" => &v.name,
                "stem" => &v.stem,
                "ext" => &v.ext,
                _ => return None,
            };
            Some((val, j))
        });
        match replaced {
            Some((val, j)) => {
                out.push_str(val);
                rest = &after[j + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 인자 목록을 전개한다. 정확히 `{paths}` 인 인자는 `group` 의 모든 경로로 확장되고,
/// 나머지는 `group[0]` 기준으로 치환한다. `group` 은 비어 있으면 안 된다.
fn expand_args(args: &[String], group: &[String]) -> Vec<String> {
    let v = vars_for(&group[0]);
    let mut out = Vec::new();
    for a in args {
        if a == "{paths}" {
            out.extend(group.iter().cloned());
        } else {
            out.push(expand(a, &v));
        }
    }
    out
}

/// 한 번의 실행이 다루는 경로 묶음: each 는 경로마다, batch 는 전체 1묶음
fn groups(a: &ActionConfig, paths: &[String]) -> Vec<Vec<String>> {
    if paths.is_empty() {
        Vec::new()
    } else if a.mode == "batch" {
        vec![paths.to_vec()]
    } else {
        paths.iter().map(|p| vec![p.clone()]).collect()
    }
}

/// 실행할 명령 전체(`[실행파일, 인자...]`)를 만든다. `paths` 는 정규화된 경로여야 한다.
/// 실행 파일(`run[0]`)은 고정 문자열이라 치환하지 않는다.
fn build_commands(a: &ActionConfig, paths: &[String]) -> Vec<Vec<String>> {
    if a.run.is_empty() {
        return Vec::new();
    }
    groups(a, paths)
        .iter()
        .map(|g| {
            let mut argv = vec![a.run[0].clone()];
            argv.extend(expand_args(&a.run[1..], g));
            argv
        })
        .collect()
}

/// 작업 폴더: cwd 템플릿 치환 결과, 비면 첫 대상의 폴더. 존재하지 않으면 None.
fn cwd_for(a: &ActionConfig, group: &[String]) -> Option<String> {
    let v = vars_for(&group[0]);
    let mut c = if a.cwd.trim().is_empty() { String::new() } else { expand(a.cwd.trim(), &v) };
    if c.is_empty() {
        c = v.dir;
    }
    if !c.is_empty() && Path::new(&c).is_dir() {
        Some(c)
    } else {
        None
    }
}

fn spawn(a: &ActionConfig, argv: &[String], cwd: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    #[cfg(windows)]
    if !a.window {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = a.window;
    match cmd.spawn() {
        Ok(_) => Ok(()), // 기다리지 않는다
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("실행 파일을 찾을 수 없습니다: {} ({})", argv[0], a.name))
        }
        Err(e) => Err(format!("실행하지 못했습니다 ({}): {e}", a.name)),
    }
}

/// 액션을 실행한다. 하나라도 시작하지 못하면 즉시 Err.
pub fn run(actions: &[ActionConfig], id: usize, paths: &[String]) -> Result<(), String> {
    let a = actions.get(id).ok_or_else(|| format!("알 수 없는 액션입니다: {id}"))?;
    if a.run.is_empty() {
        return Err(format!("실행할 명령이 없습니다: {}", a.name));
    }
    if !is_applicable(a, paths) {
        return Err(format!("이 항목에는 적용할 수 없습니다: {}", a.name));
    }
    let norm: Vec<String> = paths.iter().map(|p| normalize(p)).collect();
    let gs = groups(a, &norm);
    let cmds = build_commands(a, &norm);
    for (g, argv) in gs.iter().zip(cmds.iter()) {
        let cwd = cwd_for(a, g);
        spawn(a, argv, cwd.as_deref())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fdpick_ua_test_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn act(target: &str, ext: &[&str], mode: &str, run: &[&str]) -> ActionConfig {
        ActionConfig {
            name: "테스트".into(),
            key: String::new(),
            ext: ext.iter().map(|s| s.to_string()).collect(),
            target: target.into(),
            run: run.iter().map(|s| s.to_string()).collect(),
            mode: mode.into(),
            cwd: String::new(),
            window: false,
            close_popup: true,
        }
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().to_string()
    }

    fn wait_for(p: &Path) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if p.exists() {
                // 쓰기 완료를 잠깐 기다린다
                std::thread::sleep(Duration::from_millis(150));
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn normalize_cases() {
        assert_eq!(normalize("C:/a/b/"), "C:\\a\\b");
        assert_eq!(normalize("D:\\"), "D:\\");
        assert_eq!(normalize("D:"), "D:\\");
        assert_eq!(normalize("\\\\?\\C:\\x\\"), "C:\\x");
        assert_eq!(normalize("\\\\?\\UNC\\srv\\sh\\f"), "\\\\srv\\sh\\f");
    }

    #[test]
    fn parent_and_name() {
        assert_eq!(parent_of("D:\\a.txt"), "D:\\");
        assert_eq!(parent_of("D:\\x\\a.txt"), "D:\\x");
        assert_eq!(parent_of("D:\\"), "D:\\");
        assert_eq!(parent_of("\\\\srv\\sh\\f.txt"), "\\\\srv\\sh");
        assert_eq!(parent_of("\\\\srv\\sh"), "\\\\srv\\sh");
        assert_eq!(name_of("D:\\x\\a.txt"), "a.txt");
        assert_eq!(split_ext("a.tar.GZ"), ("a.tar", "GZ"));
        assert_eq!(split_ext(".gitignore"), (".gitignore", ""));
        assert_eq!(split_ext("noext"), ("noext", ""));
        assert_eq!(split_ext("trail."), ("trail.", ""));
    }

    #[test]
    fn expand_each_key() {
        let v = vars_for("D:\\work\\사진.2024\\Photo.final.PSD");
        assert_eq!(expand("{path}", &v), "D:\\work\\사진.2024\\Photo.final.PSD");
        assert_eq!(expand("{dir}", &v), "D:\\work\\사진.2024");
        assert_eq!(expand("{name}", &v), "Photo.final.PSD");
        assert_eq!(expand("{stem}", &v), "Photo.final");
        assert_eq!(expand("{ext}", &v), "PSD");
        assert_eq!(expand("--out={dir}\\{stem}.png", &v), "--out=D:\\work\\사진.2024\\Photo.final.png");
    }

    #[test]
    fn expand_root_child_dir() {
        let v = vars_for("D:\\a.txt");
        assert_eq!(expand("{dir}", &v), "D:\\");
        // 디렉터리 대상이면 그 디렉터리의 부모
        let v = vars_for("D:\\data");
        assert_eq!(expand("{dir}|{name}|{stem}|{ext}", &v), "D:\\|data|data|");
    }

    #[test]
    fn expand_unknown_and_edge() {
        let v = vars_for("D:\\a\\f.txt");
        assert_eq!(expand("{foo}", &v), "{foo}");
        assert_eq!(expand("{paths}", &v), "{paths}");
        assert_eq!(expand("{{path}}", &v), "{D:\\a\\f.txt}");
        assert_eq!(expand("{path", &v), "{path");
        assert_eq!(expand("a}{", &v), "a}{");
        assert_eq!(expand("{}", &v), "{}");
        assert_eq!(expand("한글{name}끝", &v), "한글f.txt끝");
        // 치환된 값 안의 템플릿은 다시 치환되지 않는다
        let v = vars_for("D:\\a\\{name}.txt");
        assert_eq!(expand("{path}", &v), "D:\\a\\{name}.txt");
    }

    #[test]
    fn paths_expansion_position() {
        let g = vec!["D:\\a\\1.txt".to_string(), "D:\\b\\2.txt".to_string()];
        let args: Vec<String> = ["-f", "{paths}", "--last", "{name}", "x{paths}"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            expand_args(&args, &g),
            vec!["-f", "D:\\a\\1.txt", "D:\\b\\2.txt", "--last", "1.txt", "x{paths}"]
        );
    }

    #[test]
    fn build_commands_each_and_batch() {
        let paths = vec!["D:\\a\\1.psd".to_string(), "D:\\b\\2.psd".to_string()];
        let each = act("any", &[], "each", &["ed.exe", "{path}", "{paths}", "{foo}"]);
        assert_eq!(
            build_commands(&each, &paths),
            vec![
                vec!["ed.exe", "D:\\a\\1.psd", "D:\\a\\1.psd", "{foo}"],
                vec!["ed.exe", "D:\\b\\2.psd", "D:\\b\\2.psd", "{foo}"],
            ]
        );
        let batch = act("any", &[], "batch", &["ed.exe", "--base", "{dir}", "{paths}"]);
        assert_eq!(
            build_commands(&batch, &paths),
            vec![vec!["ed.exe", "--base", "D:\\a", "D:\\a\\1.psd", "D:\\b\\2.psd"]]
        );
        // paths 없는 batch: 인자 목록에 그대로 둠(없으면 확장 없음)
        let nopaths = act("any", &[], "batch", &["ed.exe", "{name}"]);
        assert_eq!(build_commands(&nopaths, &paths), vec![vec!["ed.exe", "1.psd"]]);
        // 실행 파일은 치환하지 않는다
        let exe = act("any", &[], "each", &["{path}.exe", "{name}"]);
        assert_eq!(build_commands(&exe, &paths[..1])[0][0], "{path}.exe");
        assert!(build_commands(&each, &[]).is_empty());
    }

    #[test]
    fn applicable_target_ext_combinations() {
        let d = tmp();
        let f_psd = d.join("A.PSD");
        let f_txt = d.join("b.txt");
        let f_noext = d.join("noext");
        let sub = d.join("folder.psd");
        std::fs::write(&f_psd, "x").unwrap();
        std::fs::write(&f_txt, "x").unwrap();
        std::fs::write(&f_noext, "x").unwrap();
        std::fs::create_dir_all(&sub).unwrap();
        let missing = d.join("missing.psd");
        let (psd, txt, noext, dir, miss) = (s(&f_psd), s(&f_txt), s(&f_noext), s(&sub), s(&missing));

        let acts = vec![
            act("any", &[], "each", &["x"]),            // 0: 제한 없음
            act("file", &[], "each", &["x"]),           // 1: 파일만
            act("dir", &[], "each", &["x"]),            // 2: 폴더만
            act("file", &["psd"], "each", &["x"]),      // 3: 파일 + psd
            act("any", &["psd", "txt"], "each", &["x"]), // 4: any + ext
            act("dir", &["psd"], "each", &["x"]),       // 5: dir + ext (항상 불충족)
        ];
        let ids = |paths: &[&str]| -> Vec<usize> {
            let p: Vec<String> = paths.iter().map(|x| x.to_string()).collect();
            applicable(&acts, &p).iter().map(|i| i.id).collect()
        };
        assert_eq!(ids(&[&psd]), vec![0, 1, 3, 4]);
        assert_eq!(ids(&[&txt]), vec![0, 1, 4]);
        assert_eq!(ids(&[&noext]), vec![0, 1]);
        assert_eq!(ids(&[&dir]), vec![0, 2]);
        // 존재하지 않는 경로: file/dir 불충족, any 는 통과, ext 는 이름으로 판정
        assert_eq!(ids(&[&miss]), vec![0, 4]);
        // 다중 경로 혼합: 모두 만족해야 한다
        assert_eq!(ids(&[&psd, &txt]), vec![0, 1, 4]);
        assert_eq!(ids(&[&psd, &dir]), vec![0]);
        assert_eq!(ids(&[&psd, &miss]), vec![0, 4]);
        assert_eq!(ids(&[&txt, &noext]), vec![0, 1]);
        // 선택이 비면 없음
        assert_eq!(ids(&[]), Vec::<usize>::new());
        // 정규화 전 표기(슬래시, 끝 구분자)도 같은 판정
        let alt = format!("{}/", dir.replace('\\', "/"));
        assert_eq!(ids(&[&alt]), vec![0, 2]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn all_lists_everything() {
        let mut b = act("file", &["psd"], "batch", &["x", "{paths}"]);
        b.key = "Ctrl+Alt+B".into();
        b.close_popup = false;
        let acts = vec![act("dir", &[], "each", &["x"]), b];
        let v = all(&acts);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].id, v[0].batch, v[0].close_popup), (0, false, true));
        assert_eq!((v[1].id, v[1].batch, v[1].close_popup), (1, true, false));
        assert_eq!(v[1].key, "Ctrl+Alt+B");
        assert_eq!(v[1].name, "테스트");
    }

    #[test]
    fn info_serializes() {
        let v = all(&[act("any", &[], "each", &["x"])]);
        let j = serde_json_like(&v[0]);
        assert!(j.contains("close_popup"));
    }

    // serde_json 의존성을 피하기 위해 toml 로 직렬화만 확인한다
    fn serde_json_like(i: &ActionInfo) -> String {
        toml::to_string(i).unwrap()
    }

    #[test]
    fn run_errors() {
        let d = tmp();
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let p = vec![s(&f)];
        let acts = vec![act("dir", &[], "each", &["x.exe"])];
        // id 범위
        assert!(run(&acts, 5, &p).is_err());
        // 적용 불가
        let e = run(&acts, 0, &p).unwrap_err();
        assert_eq!(e, "이 항목에는 적용할 수 없습니다: 테스트");
        // 선택 없음
        assert!(run(&acts, 0, &[]).is_err());
        // 존재하지 않는 실행 파일
        let acts = vec![act("any", &[], "each", &["fdpick_no_such_program_zz.exe", "{path}"])];
        let e = run(&acts, 0, &p).unwrap_err();
        assert!(e.contains("실행 파일을 찾을 수 없습니다"), "{e}");
        assert!(e.contains("fdpick_no_such_program_zz.exe") && e.contains("테스트"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn run_spawns_simple_program() {
        let d = tmp();
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let acts = vec![act("file", &["txt"], "each", &["hostname"])];
        assert!(run(&acts, 0, &[s(&f)]).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn run_each_spawns_per_file() {
        let d = tmp();
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        std::fs::write(&a, "x").unwrap();
        std::fs::write(&b, "x").unwrap();
        let acts = vec![act(
            "file",
            &[],
            "each",
            &["powershell.exe", "-NoProfile", "-Command", "Set-Content -LiteralPath '{path}.out' -Value x"],
        )];
        run(&acts, 0, &[s(&a), s(&b)]).unwrap();
        assert!(wait_for(&d.join("a.txt.out")), "a.txt.out 이 생기지 않음");
        assert!(wait_for(&d.join("b.txt.out")), "b.txt.out 이 생기지 않음");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn run_batch_spawns_once_with_all_paths_and_cwd() {
        let d = tmp();
        let a = d.join("a.txt");
        let b = d.join("b.txt");
        let c = d.join("c.txt");
        for f in [&a, &b, &c] {
            std::fs::write(f, "x").unwrap();
        }
        // 호출마다 고유한 파일을 만들고, 인자 개수와 작업 폴더를 기록한다
        let script = d.join("batch.ps1");
        std::fs::write(
            &script,
            "$n = [guid]::NewGuid().ToString()\r\nSet-Content -LiteralPath \"$PSScriptRoot\\call_$n.log\" -Value \"$($args.Count)|$((Get-Location).Path)\"\r\n",
        )
        .unwrap();
        let acts = vec![act(
            "file",
            &[],
            "batch",
            &["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &s(&script), "{paths}"],
        )];
        run(&acts, 0, &[s(&a), s(&b), s(&c)]).unwrap();

        let find_logs = || -> Vec<PathBuf> {
            std::fs::read_dir(&d)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "log"))
                .collect()
        };
        let end = Instant::now() + Duration::from_secs(5);
        while find_logs().is_empty() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(1000)); // 추가 호출이 없는지 확인할 여유
        let logs = find_logs();
        assert_eq!(logs.len(), 1, "batch 는 1회만 호출되어야 함: {logs:?}");
        let text = std::fs::read_to_string(&logs[0]).unwrap();
        let text = text.trim_start_matches('\u{FEFF}').trim();
        let (cnt, cwd) = text.split_once('|').unwrap();
        assert_eq!(cnt, "3", "{text}");
        assert!(cwd.eq_ignore_ascii_case(&s(&d)), "cwd={cwd} dir={}", s(&d));
        let _ = std::fs::remove_dir_all(&d);
    }
}
