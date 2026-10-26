//! Controller 시나리오 테스트. 엔진 결과는 ResultsUpdated 를 직접 만들어 on_results 에 넣는다.

use std::sync::Arc;

use fdpick_core::engine::{Mode, ResultItem, ResultsUpdated, RunState};
use fdpick_core::user_actions::ActionInfo;

use crate::controller::{Controller, ControllerInit};
use crate::view::{Command, KeyEvent, KeyOutcome, StatusKind};

fn act(id: usize, name: &str, key: &str, close: bool) -> ActionInfo {
    ActionInfo { id, name: name.into(), key: key.into(), batch: false, close_popup: close }
}

fn make(saved: Option<Mode>, actions: Vec<ActionInfo>, notes: Vec<&str>) -> Controller {
    let list = actions.clone();
    Controller::new(ControllerInit {
        saved_mode: saved,
        all_actions: actions,
        applicable: Arc::new(move |_paths: &[String]| list.clone()),
        startup_notes: notes.into_iter().map(String::from).collect(),
    })
}

fn plain(saved: Option<Mode>) -> Controller {
    make(saved, Vec::new(), Vec::new())
}

fn item(path: &str, is_dir: bool, recent: bool) -> ResultItem {
    let cs: Vec<char> = path.chars().collect();
    let ns = cs.iter().rposition(|&c| c == '\\').map(|i| i + 1).unwrap_or(0);
    ResultItem {
        path: path.into(),
        name_start: ns,
        indices: Vec::new(),
        is_dir,
        recent,
        missing: false,
    }
}

fn dir(path: &str) -> ResultItem {
    item(path, true, false)
}

fn res(seq: u64, mode: Mode, items: Vec<ResultItem>, state: RunState) -> ResultsUpdated {
    let n = items.len() as u32;
    ResultsUpdated {
        seq,
        mode,
        items,
        matched: n,
        scanned: n,
        state,
        elapsed_ms: 5,
        message: None,
        cache_note: None,
    }
}

fn k(key: &str) -> KeyEvent {
    KeyEvent {
        key: key.into(),
        ctrl: false,
        alt: false,
        shift: false,
        caret_at_end: true,
        composing: false,
    }
}

fn ck(key: &str) -> KeyEvent {
    KeyEvent { ctrl: true, ..k(key) }
}

/// 입력 텍스트를 바꾸고(on_input_changed) 결과를 넣어 경로 모드 상태를 만든다.
fn typed(c: &mut Controller, text: &str, seq: u64, items: Vec<ResultItem>) {
    c.on_input_changed(text, false);
    c.on_results(res(seq, c.view().mode, items, RunState::Done));
}

fn query(text: &str, mode: Mode, force: bool) -> Command {
    Command::Query { text: text.into(), mode, force }
}

fn has_hide(o: &KeyOutcome) -> bool {
    o.commands.contains(&Command::Hide)
}

fn copy_cmd(paths: &[&str]) -> Command {
    Command::RunAction { kind: "copy".into(), paths: paths.iter().map(|s| s.to_string()).collect() }
}

#[test]
fn initial_mode_and_labels() {
    let c = plain(None);
    let v = c.view();
    assert_eq!(v.mode, Mode::Path);
    assert_eq!(v.mode_label, "경로");
    assert_eq!(v.placeholder, "경로 입력 (예: d:\\work)");
    assert_eq!(
        v.help,
        "폴더 탐색 · Tab 또는 / 진입 · Shift+Tab 상위 · Enter 복사 · Ctrl+Enter 탐색기 · Shift+Enter 열린 탐색기로 · Ctrl+L 모드"
    );
    let v = plain(Some(Mode::Fuzzy)).view();
    assert_eq!(v.mode, Mode::Fuzzy);
    assert_eq!(v.mode_label, "검색");
    assert_eq!(v.placeholder, "[fd옵션] -- 검색어");
    assert!(v.help.starts_with("[fd옵션] -- 검색어 \u{00a0} 예) @D:\\work -e pdf"));
    // 저장값이 recent 로 들어와도 경로 모드
    assert_eq!(plain(Some(Mode::Recent)).view().mode, Mode::Path);
    assert_eq!(plain(None).view().status, "준비됨");
}

#[test]
fn help_has_actions_suffix() {
    let c = make(None, vec![act(0, "A", "", true)], vec![]);
    assert!(c.view().help.ends_with("Ctrl+L 모드 · Ctrl+K 액션"));
}

#[test]
fn ctrl_l_cycles_and_save_mode_rules() {
    let mut c = plain(Some(Mode::Fuzzy));
    // fuzzy -> path : 저장
    let o = c.on_key(&ck("l"), "abc");
    assert!(o.handled);
    assert_eq!(
        o.commands,
        vec![
            Command::SaveMode(Mode::Path),
            Command::SetInput(String::new()),
            query("", Mode::Path, false)
        ]
    );
    assert_eq!(c.view().mode_label, "경로");
    // path -> recent : 저장하지 않음
    let o = c.on_key(&ck("L"), "");
    assert_eq!(o.commands, vec![Command::SetInput(String::new()), query("", Mode::Recent, false)]);
    let v = c.view();
    assert_eq!(v.mode, Mode::Recent);
    assert_eq!(v.mode_label, "최근");
    assert_eq!(v.placeholder, "최근 사용 항목 검색");
    assert!(v.help.starts_with("최근 사용 · 퍼지 검색"));
    // recent -> fuzzy : 저장
    let o = c.on_key(&ck("l"), "");
    assert_eq!(o.commands[0], Command::SaveMode(Mode::Fuzzy));
    assert_eq!(c.view().mode, Mode::Fuzzy);
    // 팝업 재표시는 마지막으로 저장한 모드(fuzzy)
    let cmds = c.on_show();
    assert_eq!(cmds, vec![Command::SetInput(String::new()), query("", Mode::Fuzzy, false)]);
}

#[test]
fn recent_mode_is_not_restored_on_show() {
    let mut c = plain(Some(Mode::Path));
    c.on_key(&ck("l"), "");
    assert_eq!(c.view().mode, Mode::Recent);
    c.on_show();
    assert_eq!(c.view().mode, Mode::Path);
}

#[test]
fn input_change_always_queries_even_when_composing() {
    let mut c = plain(None);
    assert_eq!(c.on_input_changed("한", true), vec![query("한", Mode::Path, false)]);
    assert_eq!(c.on_input_changed("한글", false), vec![query("한글", Mode::Path, false)]);
}

#[test]
fn ctrl_r_forces_query() {
    let mut c = plain(None);
    c.on_input_changed("abc", false);
    let o = c.on_key(&ck("r"), "abc");
    assert!(o.handled);
    assert_eq!(o.commands, vec![query("abc", Mode::Path, true)]);
}

#[test]
fn drive_letter_means_no_cursor_and_slash_only_adds_separator() {
    let mut c = plain(None);
    typed(&mut c, "c:", 1, vec![dir("C:\\apps"), dir("C:\\temp")]);
    let v = c.view();
    assert_eq!(v.cursor, None);
    assert_eq!(v.rows.len(), 2);
    // '/' 는 항목으로 완성하지 않고 구분자만 붙인다
    let o = c.on_key(&k("/"), "c:");
    assert!(o.handled);
    assert_eq!(o.commands, vec![Command::SetInput("c:\\".into()), query("c:\\", Mode::Path, false)]);
    // 입력이 폴더를 가리키므로 여전히 선택 없음
    assert_eq!(c.view().cursor, None);
    // 끝 구분자가 있으면 '\' 는 아무것도 붙이지 않는다
    let o = c.on_key(&k("\\"), "c:\\");
    assert!(o.handled);
    assert!(o.commands.is_empty());
}

#[test]
fn separator_not_at_end_is_left_to_edit() {
    let mut c = plain(None);
    typed(&mut c, "c:\\a", 1, vec![dir("C:\\apps")]);
    let mut ev = k("/");
    ev.caret_at_end = false;
    let o = c.on_key(&ev, "c:\\a");
    assert!(!o.handled);
    assert!(o.commands.is_empty());
}

#[test]
fn separator_on_empty_input_does_nothing() {
    let mut c = plain(None);
    c.on_results(res(1, Mode::Path, vec![dir("C:\\"), dir("D:\\")], RunState::Done));
    let o = c.on_key(&k("\\"), "");
    assert!(o.handled);
    assert!(o.commands.is_empty());
}

#[test]
fn separator_enters_highlighted_dir() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\b", 1, vec![dir("C:\\apps\\bin"), dir("C:\\apps\\bak")]);
    assert_eq!(c.view().cursor, Some(0));
    let o = c.on_key(&k("\\"), "C:\\apps\\b");
    assert_eq!(
        o.commands,
        vec![Command::SetInput("C:\\apps\\bin\\".into()), query("C:\\apps\\bin\\", Mode::Path, false)]
    );
}

#[test]
fn enter_completes_first_then_copies() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\b", 1, vec![dir("C:\\apps\\bin")]);
    // 첫 Enter: 복사하지 않고 완성만
    let o = c.on_key(&k("Enter"), "C:\\apps\\b");
    assert!(o.handled);
    assert_eq!(
        o.commands,
        vec![Command::SetInput("C:\\apps\\bin\\".into()), query("C:\\apps\\bin\\", Mode::Path, false)]
    );
    // SetInput 에 대한 EN_CHANGE 에코는 무시된다(중복 질의 없음)
    assert!(c.on_input_changed("C:\\apps\\bin\\", false).is_empty());
    // 결과가 오면 폴더를 가리키므로 선택 없음
    c.on_results(res(2, Mode::Path, vec![dir("C:\\apps\\bin\\sub")], RunState::Done));
    assert_eq!(c.view().cursor, None);
    // 두 번째 Enter: 입력 경로(끝 구분자 제거)를 복사
    let o = c.on_key(&k("Enter"), "C:\\apps\\bin\\");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\apps\\bin"])]);
}

#[test]
fn enter_copies_when_input_equals_name_case_insensitive() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\BIN", 1, vec![dir("C:\\apps\\bin")]);
    let o = c.on_key(&k("Enter"), "C:\\apps\\BIN");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\apps\\bin"])]);
}

#[test]
fn enter_copies_arrow_selected_item() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\b", 1, vec![dir("C:\\apps\\bin"), dir("C:\\apps\\bak")]);
    c.on_key(&k("ArrowDown"), "C:\\apps\\b");
    assert_eq!(c.view().cursor, Some(1));
    let o = c.on_key(&k("Enter"), "C:\\apps\\b");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\apps\\bak"])]);
}

#[test]
fn enter_with_multi_pick_copies_picked() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\b", 1, vec![dir("C:\\apps\\bin"), dir("C:\\apps\\bak")]);
    c.on_key(&ck("Space"), "C:\\apps\\b");
    let o = c.on_key(&k("Enter"), "C:\\apps\\b");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\apps\\bin"])]);
}

#[test]
fn enter_on_empty_input_without_cursor_does_nothing() {
    let mut c = plain(None);
    let o = c.on_key(&k("Enter"), "");
    assert!(o.handled);
    assert!(o.commands.is_empty());
}

#[test]
fn enter_variants() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\bin", 1, vec![dir("C:\\apps\\bin")]);
    let mut alt = k("Enter");
    alt.alt = true;
    let o = c.on_key(&alt, "C:\\apps\\bin");
    assert_eq!(
        o.commands,
        vec![Command::RunAction { kind: "open".into(), paths: vec!["C:\\apps\\bin".into()] }]
    );
    let o = c.on_key(&ck("Enter"), "C:\\apps\\bin");
    assert_eq!(
        o.commands,
        vec![Command::RunAction { kind: "reveal".into(), paths: vec!["C:\\apps\\bin".into()] }]
    );
    let o = c.on_key(&ck("o"), "C:\\apps\\bin");
    assert_eq!(
        o.commands,
        vec![Command::RunAction { kind: "reveal".into(), paths: vec!["C:\\apps\\bin".into()] }]
    );
    let mut sh = k("Enter");
    sh.shift = true;
    let o = c.on_key(&sh, "C:\\apps\\bin");
    assert_eq!(
        o.commands,
        vec![Command::RunAction { kind: "to_explorer".into(), paths: vec!["C:\\apps\\bin".into()] }]
    );
    let o = c.on_key(&ck("d"), "C:\\apps\\bin");
    assert_eq!(
        o.commands,
        vec![Command::RunAction { kind: "copy_dir".into(), paths: vec!["C:\\apps\\bin".into()] }]
    );
    // Ctrl+Shift+Enter 는 아무 일도 하지 않지만 handled
    let mut cs = ck("Enter");
    cs.shift = true;
    let o = c.on_key(&cs, "C:\\apps\\bin");
    assert!(o.handled && o.commands.is_empty());
}

#[test]
fn recent_items_skipped_by_default_selection() {
    let mut c = plain(None);
    let items = vec![
        item("C:\\old\\a", true, true),
        item("C:\\old\\b.txt", false, true),
        dir("C:\\apps\\bin"),
        dir("C:\\apps\\bak"),
    ];
    typed(&mut c, "C:\\apps\\b", 1, items);
    let v = c.view();
    assert_eq!(v.cursor, Some(2));
    assert!(v.rows[0].recent_badge && !v.rows[0].recent_last);
    assert!(v.rows[1].recent_badge && v.rows[1].recent_last);
    assert!(!v.rows[2].recent_badge && !v.rows[2].recent_last);
    // 최근 항목이 모두이면 첫 항목
    let mut c = plain(None);
    typed(&mut c, "xy", 1, vec![item("C:\\o\\a", true, true)]);
    assert_eq!(c.view().cursor, Some(0));
}

#[test]
fn recent_mode_rows_have_no_badge() {
    let mut c = plain(None);
    c.on_key(&ck("l"), "");
    c.on_results(res(1, Mode::Recent, vec![item("C:\\o\\a", true, true), item("C:\\o\\b", true, true)], RunState::Done));
    let v = c.view();
    assert!(v.rows.iter().all(|r| !r.recent_badge && !r.recent_last));
    assert_eq!(v.cursor, Some(0));
    assert_eq!(v.status, "2개 일치");
}

#[test]
fn recent_item_separator_and_tab_rules() {
    let mut c = plain(None);
    let items = vec![item("C:\\old\\a.txt", false, true), dir("C:\\apps\\bin")];
    typed(&mut c, "C:\\a", 1, items.clone());
    // 기본 선택은 폴더 항목이므로 직접 올려 최근 항목을 고른다
    c.on_key(&k("ArrowUp"), "C:\\a");
    assert_eq!(c.view().cursor, Some(0));
    // 직접 고른 최근 항목은 '/' 로 부모 폴더 진입(파일이면 부모)
    let o = c.on_key(&k("/"), "C:\\a");
    assert_eq!(o.commands, vec![Command::SetInput("C:\\old\\".into()), query("C:\\old\\", Mode::Path, false)]);
    // 직접 고르지 않은 최근 항목(Row 클릭 없이 결과만 recent)에서는 구분자만
    let mut c = plain(None);
    typed(&mut c, "C:\\a", 1, vec![item("C:\\old\\a.txt", false, true)]);
    assert_eq!(c.view().cursor, Some(0));
    let o = c.on_key(&k("\\"), "C:\\a");
    assert_eq!(o.commands, vec![Command::SetInput("C:\\a\\".into()), query("C:\\a\\", Mode::Path, false)]);
    // Tab 은 최근 항목이면 폴더 진입(경로 모드 유지, SaveMode 없음)
    let mut c = plain(None);
    typed(&mut c, "C:\\a", 1, vec![item("C:\\old\\a.txt", false, true)]);
    let o = c.on_key(&k("Tab"), "C:\\a");
    assert_eq!(o.commands, vec![Command::SetInput("C:\\old\\".into()), query("C:\\old\\", Mode::Path, false)]);
}

#[test]
fn shift_tab_goes_to_parent() {
    let mut c = plain(None);
    typed(&mut c, "C:\\apps\\bin\\", 1, vec![]);
    let mut st = k("Tab");
    st.shift = true;
    let o = c.on_key(&st, "C:\\apps\\bin\\");
    assert!(o.handled);
    assert_eq!(o.commands, vec![Command::SetInput("C:\\apps\\".into()), query("C:\\apps\\", Mode::Path, false)]);
    let o = c.on_key(&st, "C:\\apps\\");
    assert_eq!(o.commands[0], Command::SetInput("C:\\".into()));
    // 드라이브 루트 -> 빈 문자열(드라이브 목록)
    let o = c.on_key(&st, "C:\\");
    assert_eq!(o.commands[0], Command::SetInput(String::new()));
    // 빈 입력은 무시하되 handled
    let o = c.on_key(&st, "");
    assert!(o.handled && o.commands.is_empty());
}

#[test]
fn shift_tab_ignored_in_other_modes() {
    let mut c = plain(Some(Mode::Fuzzy));
    let mut st = k("Tab");
    st.shift = true;
    let o = c.on_key(&st, "abc");
    assert!(o.handled && o.commands.is_empty());
}

#[test]
fn tab_in_path_mode_enters_highlighted_dir() {
    let mut c = plain(None);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    let o = c.on_key(&k("Tab"), "C:\\ap");
    assert_eq!(o.commands, vec![Command::SetInput("C:\\apps\\".into()), query("C:\\apps\\", Mode::Path, false)]);
    // 항목이 없으면 아무 일도 없다
    let mut c = plain(None);
    let o = c.on_key(&k("Tab"), "");
    assert!(o.handled && o.commands.is_empty());
    // Ctrl+Tab 도 handled, 무동작
    let o = c.on_key(&ck("Tab"), "");
    assert!(o.handled && o.commands.is_empty());
}

#[test]
fn tab_completes_at_path_token() {
    let base = std::env::temp_dir().join(format!("fdpick_ac_{}", std::process::id()));
    let dir = base.join("with space");
    std::fs::create_dir_all(&dir).unwrap();
    let base_s = base.to_string_lossy().replace('/', "\\");

    let mut c = plain(Some(Mode::Fuzzy));
    let typed = format!("-e pdf @{}/wi", base_s);
    let o = c.on_key(&k("Tab"), &typed);
    let want = format!("-e pdf @\"{}\\with space\\\"", base_s);
    assert!(o.handled);
    assert_eq!(o.commands, vec![Command::SetInput(want.clone()), query(&want, Mode::Fuzzy, false)]);
    assert_eq!(c.view().mode, Mode::Fuzzy);

    // 후보가 없으면 입력은 그대로이고 모드도 바뀌지 않는다
    let mut c = plain(Some(Mode::Fuzzy));
    let o = c.on_key(&k("Tab"), &format!("@{}\\zzz", base_s));
    assert!(o.handled);
    assert!(o.commands.is_empty());
    assert_eq!(c.view().mode, Mode::Fuzzy);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn tab_fuzzy_to_path_switches_and_saves() {
    let mut c = plain(Some(Mode::Fuzzy));
    c.on_input_changed("rep", false);
    c.on_results(res(
        1,
        Mode::Fuzzy,
        vec![item("D:\\docs\\보고서.pdf", false, false), item("D:\\x\\sub", true, false)],
        RunState::Done,
    ));
    let o = c.on_key(&k("Tab"), "rep");
    assert!(o.handled);
    assert_eq!(
        o.commands,
        vec![
            Command::SaveMode(Mode::Path),
            Command::SetInput("D:\\docs\\".into()),
            query("D:\\docs\\", Mode::Path, false)
        ]
    );
    assert_eq!(c.view().mode, Mode::Path);
    // 폴더 항목이면 자기 자신
    let mut c = plain(Some(Mode::Fuzzy));
    c.on_results(res(1, Mode::Fuzzy, vec![item("D:\\x\\sub", true, false)], RunState::Done));
    let o = c.on_key(&k("Tab"), "");
    assert_eq!(o.commands[1], Command::SetInput("D:\\x\\sub\\".into()));
    // 파일이 드라이브 루트 바로 아래
    let mut c = plain(Some(Mode::Fuzzy));
    c.on_results(res(1, Mode::Fuzzy, vec![item("D:\\a.txt", false, false)], RunState::Done));
    let o = c.on_key(&k("Tab"), "");
    assert_eq!(o.commands[1], Command::SetInput("D:\\".into()));
    // 항목이 없으면 모드 전환도 없다
    let mut c = plain(Some(Mode::Fuzzy));
    let o = c.on_key(&k("Tab"), "");
    assert!(o.handled && o.commands.is_empty());
    assert_eq!(c.view().mode, Mode::Fuzzy);
}

#[test]
fn tab_from_recent_mode_saves_path() {
    let mut c = plain(Some(Mode::Path));
    c.on_key(&ck("l"), "");
    c.on_results(res(1, Mode::Recent, vec![item("D:\\r\\f.txt", false, true)], RunState::Done));
    let o = c.on_key(&k("Tab"), "");
    assert_eq!(o.commands[0], Command::SaveMode(Mode::Path));
    assert_eq!(o.commands[1], Command::SetInput("D:\\r\\".into()));
    assert_eq!(c.view().mode, Mode::Path);
}

#[test]
fn escape_hides_and_menu_escape_only_closes_menu() {
    let mut c = make(None, vec![act(0, "열기", "Ctrl+Shift+V", true)], vec![]);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    // 메뉴 열기
    let o = c.on_key(&ck("k"), "C:\\ap");
    assert!(o.handled);
    let v = c.view();
    assert!(v.menu_open);
    assert_eq!(v.help, "액션 메뉴 · 위/아래 선택 · Enter 실행 · Esc 닫기");
    assert_eq!(v.rows.len(), 1);
    assert_eq!(v.rows[0].path, "열기");
    assert_eq!(v.rows[0].name_start, 0);
    assert!(v.rows[0].indices.is_empty());
    assert_eq!(v.rows[0].hint.as_deref(), Some("Ctrl+Shift+V"));
    assert_eq!(v.cursor, Some(0));
    assert_eq!(v.status, "액션 메뉴 · 완료 · 1개 일치 / 1개 스캔 · 5ms");
    // Esc: 메뉴만 닫고 이전 목록 복원
    let o = c.on_key(&k("Escape"), "C:\\ap");
    assert!(o.handled && !has_hide(&o));
    let v = c.view();
    assert!(!v.menu_open);
    assert_eq!(v.rows[0].path, "C:\\apps");
    assert_eq!(v.cursor, Some(0));
    assert!(v.rows[0].hint.is_none());
    // 한 번 더 Esc 는 Hide
    let o = c.on_key(&k("Escape"), "C:\\ap");
    assert!(has_hide(&o));
}

#[test]
fn menu_run_and_ignored_keys() {
    let acts = vec![act(3, "첫째", "", true), act(7, "둘째", "Ctrl+Alt+X", false)];
    let mut c = make(None, acts, vec![]);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    c.on_key(&ck("k"), "C:\\ap");
    // 무시 키: handled 지만 명령 없음
    for ev in [k("Tab"), k("/"), k("\\"), ck("d"), ck("o"), ck("l"), ck("r"), ck("k"), ck("Space")] {
        let o = c.on_key(&ev, "C:\\ap");
        assert!(o.handled, "{:?}", ev);
        assert!(o.commands.is_empty());
        assert!(c.view().menu_open);
    }
    // 일반 문자 키는 EDIT 로 넘긴다
    let o = c.on_key(&k("x"), "C:\\ap");
    assert!(!o.handled && o.commands.is_empty());
    // 입력 변경은 메뉴 중 무시
    assert!(c.on_input_changed("C:\\apx", false).is_empty());
    // 이동 후 실행
    c.on_key(&k("ArrowDown"), "C:\\ap");
    assert_eq!(c.view().cursor, Some(1));
    c.on_key(&k("ArrowDown"), "C:\\ap");
    assert_eq!(c.view().cursor, Some(1));
    c.on_key(&ck("p"), "C:\\ap");
    assert_eq!(c.view().cursor, Some(0));
    // Ctrl+Enter 는 실행하지 않는다
    let o = c.on_key(&ck("Enter"), "C:\\ap");
    assert!(o.handled && o.commands.is_empty());
    let o = c.on_key(&k("Enter"), "C:\\ap");
    assert_eq!(
        o.commands,
        vec![Command::RunUserAction { id: 3, paths: vec!["C:\\apps".into()], close_popup: true }]
    );
    c.on_key(&k("ArrowDown"), "C:\\ap");
    let o = c.on_key(&k("Enter"), "C:\\ap");
    assert_eq!(
        o.commands,
        vec![Command::RunUserAction { id: 7, paths: vec!["C:\\apps".into()], close_popup: false }]
    );
    // 메뉴는 실행 후에도 열려 있다
    assert!(c.view().menu_open);
}

#[test]
fn menu_holds_results_until_closed() {
    let mut c = make(None, vec![act(0, "A", "", true)], vec![]);
    typed(&mut c, "C:\\a", 1, vec![dir("C:\\apps")]);
    c.on_key(&ck("k"), "C:\\a");
    // 메뉴 중 결과: 목록은 그대로
    c.on_results(res(2, Mode::Path, vec![dir("C:\\alpha"), dir("C:\\apps")], RunState::Done));
    let v = c.view();
    assert_eq!(v.rows.len(), 1);
    assert_eq!(v.rows[0].path, "A");
    // 닫으면 마지막 payload 가 반영된다
    c.on_key(&k("Escape"), "C:\\a");
    let v = c.view();
    assert_eq!(v.rows.len(), 2);
    assert_eq!(v.rows[0].path, "C:\\alpha");
    // 메뉴 열기 직전 커서(0, 이동 안 함)로 복원
    assert_eq!(v.cursor, Some(0));
}

#[test]
fn menu_restores_user_moved_cursor() {
    let mut c = make(None, vec![act(0, "A", "", true)], vec![]);
    typed(&mut c, "C:\\a", 1, vec![dir("C:\\a1"), dir("C:\\a2"), dir("C:\\a3")]);
    c.on_key(&k("ArrowDown"), "C:\\a");
    c.on_key(&k("ArrowDown"), "C:\\a");
    c.on_key(&ck("k"), "C:\\a");
    c.on_results(res(2, Mode::Path, vec![dir("C:\\a0"), dir("C:\\a1"), dir("C:\\a2"), dir("C:\\a3")], RunState::Done));
    c.on_key(&k("Escape"), "C:\\a");
    assert_eq!(c.view().cursor, Some(2));
}

#[test]
fn menu_without_applicable_actions() {
    let mut c = Controller::new(ControllerInit {
        saved_mode: None,
        all_actions: vec![act(0, "A", "", true)],
        applicable: Arc::new(|_: &[String]| Vec::new()),
        startup_notes: vec![],
    });
    typed(&mut c, "C:\\a", 1, vec![dir("C:\\apps")]);
    let o = c.on_key(&ck("k"), "C:\\a");
    assert!(o.handled);
    let v = c.view();
    assert!(!v.menu_open);
    assert_eq!(v.status, "적용할 수 있는 액션이 없습니다");
    assert_eq!(v.status_kind, StatusKind::Error);
    // 다음 입력에서 사라진다
    c.on_input_changed("C:\\ap", false);
    assert_ne!(c.view().status, "적용할 수 있는 액션이 없습니다");
}

#[test]
fn menu_not_opened_without_targets() {
    let mut c = make(Some(Mode::Fuzzy), vec![act(0, "A", "", true)], vec![]);
    let o = c.on_key(&ck("k"), "");
    assert!(o.handled);
    assert!(!c.view().menu_open);
}

#[test]
fn user_hotkeys_match() {
    let acts = vec![act(1, "뷰어", "Ctrl+Shift+V", true), act(2, "스페이스", "Ctrl+Alt+Space", false)];
    let mut c = make(None, acts, vec![]);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    let mut ev = ck("v");
    ev.shift = true;
    let o = c.on_key(&ev, "C:\\ap");
    assert!(o.handled);
    assert_eq!(
        o.commands,
        vec![Command::RunUserAction { id: 1, paths: vec!["C:\\apps".into()], close_popup: true }]
    );
    // 소문자 키 이름도 대문자로 정규화
    let mut ev = ck("V");
    ev.shift = true;
    assert_eq!(c.on_key(&ev, "C:\\ap").commands.len(), 1);
    // Space (Ctrl+Alt 이므로 내장 Ctrl+Space 와 구분)
    let mut ev = ck("Space");
    ev.alt = true;
    let o = c.on_key(&ev, "C:\\ap");
    assert_eq!(
        o.commands,
        vec![Command::RunUserAction { id: 2, paths: vec!["C:\\apps".into()], close_popup: false }]
    );
    // 매치 안 되는 조합은 EDIT 로
    let o = c.on_key(&ck("v"), "C:\\ap");
    assert!(!o.handled && o.commands.is_empty());
}

#[test]
fn user_hotkey_without_targets_is_handled_noop() {
    let mut c = make(Some(Mode::Fuzzy), vec![act(1, "뷰어", "Ctrl+Shift+V", true)], vec![]);
    let mut ev = ck("v");
    ev.shift = true;
    let o = c.on_key(&ev, "");
    assert!(o.handled && o.commands.is_empty());
}

#[test]
fn multi_select_and_action_targets() {
    let mut c = plain(Some(Mode::Fuzzy));
    c.on_input_changed("f", false);
    c.on_results(res(
        1,
        Mode::Fuzzy,
        vec![item("C:\\a.txt", false, false), item("C:\\b.txt", false, false), item("C:\\c.txt", false, false)],
        RunState::Done,
    ));
    // 커서 항목만
    let o = c.on_key(&k("Enter"), "f");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\a.txt"])]);
    // Ctrl+Space: 선택 후 한 칸 아래
    c.on_key(&ck("Space"), "f");
    assert_eq!(c.view().cursor, Some(1));
    c.on_key(&ck(" "), "f");
    assert_eq!(c.view().cursor, Some(2));
    let v = c.view();
    assert!(v.rows[0].picked && v.rows[1].picked && !v.rows[2].picked);
    assert_eq!(v.status, "2개 선택 · 완료 · 3개 일치 / 3개 스캔 · 5ms");
    // 다중 선택 > 커서 항목
    let o = c.on_key(&k("Enter"), "f");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\a.txt", "C:\\b.txt"])]);
    // 같은 항목 다시 토글하면 해제
    c.on_key(&k("ArrowUp"), "f");
    c.on_key(&k("ArrowUp"), "f");
    c.on_key(&ck("Space"), "f");
    let o = c.on_key(&k("Enter"), "f");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\b.txt"])]);
    // 모드 전환 시 선택 초기화
    c.on_key(&ck("l"), "f");
    assert!(c.view().rows.iter().all(|r| !r.picked));
    assert!(!c.view().status.contains("선택"));
}

#[test]
fn path_mode_target_falls_back_to_input_path() {
    let mut c = plain(None);
    typed(&mut c, "c:/apps/", 1, vec![dir("C:\\apps\\bin")]);
    assert_eq!(c.view().cursor, None);
    let o = c.on_key(&k("Enter"), "c:/apps/");
    assert_eq!(o.commands, vec![copy_cmd(&["c:\\apps"])]);
    // 드라이브 문자만
    let mut c = plain(None);
    typed(&mut c, "d", 1, vec![dir("D:\\x")]);
    let o = c.on_key(&k("Enter"), "d");
    assert_eq!(o.commands, vec![copy_cmd(&["D:\\"])]);
    // fuzzy 모드는 입력 경로 폴백 없음
    let mut c = plain(Some(Mode::Fuzzy));
    let o = c.on_key(&k("Enter"), "C:\\apps");
    assert!(o.commands.is_empty());
}

#[test]
fn drive_list_default_cursor_is_first() {
    let mut c = plain(None);
    c.on_results(res(1, Mode::Path, vec![dir("C:\\"), dir("D:\\")], RunState::Done));
    assert_eq!(c.view().cursor, Some(0));
    // 빈 입력 Enter 는 첫 드라이브 복사
    let o = c.on_key(&k("Enter"), "");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\"])]);
}

#[test]
fn stale_seq_is_dropped() {
    let mut c = plain(None);
    c.on_results(res(5, Mode::Path, vec![dir("C:\\new")], RunState::Done));
    c.on_results(res(4, Mode::Path, vec![dir("C:\\old")], RunState::Done));
    c.on_results(res(5, Mode::Path, vec![dir("C:\\dup")], RunState::Done));
    assert_eq!(c.view().rows[0].path, "C:\\new");
    c.on_results(res(6, Mode::Path, vec![dir("C:\\newer")], RunState::Done));
    assert_eq!(c.view().rows[0].path, "C:\\newer");
}

#[test]
fn mode_mismatch_payload_is_dropped_and_does_not_advance_seq() {
    let mut c = plain(None);
    c.on_results(res(9, Mode::Fuzzy, vec![dir("C:\\fz")], RunState::Done));
    assert!(c.view().rows.is_empty());
    // 폐기된 payload 는 lastSeq 를 올리지 않으므로 더 낮은 seq 의 올바른 결과도 받는다
    c.on_results(res(3, Mode::Path, vec![dir("C:\\ok")], RunState::Done));
    assert_eq!(c.view().rows[0].path, "C:\\ok");
}

#[test]
fn last_seq_survives_show_and_mode_switch() {
    let mut c = plain(None);
    c.on_results(res(10, Mode::Path, vec![dir("C:\\a")], RunState::Done));
    c.on_show();
    assert!(c.view().rows.is_empty());
    // TS 와 동일: lastSeq 는 초기화하지 않는다
    c.on_results(res(10, Mode::Path, vec![dir("C:\\a")], RunState::Done));
    assert!(c.view().rows.is_empty());
    c.on_results(res(11, Mode::Path, vec![dir("C:\\b")], RunState::Done));
    assert_eq!(c.view().rows.len(), 1);
}

#[test]
fn user_moved_keeps_same_path_across_results() {
    let mut c = plain(None);
    typed(&mut c, "C:\\a", 1, vec![dir("C:\\a1"), dir("C:\\a2"), dir("C:\\a3")]);
    c.on_key(&k("ArrowDown"), "C:\\a");
    c.on_key(&k("ArrowDown"), "C:\\a");
    assert_eq!(c.view().cursor, Some(2));
    // 새 결과(입력 변경 없음): 같은 path 유지
    c.on_results(res(2, Mode::Path, vec![dir("C:\\a0"), dir("C:\\a1"), dir("C:\\a3")], RunState::Running));
    assert_eq!(c.view().cursor, Some(2));
    // 사라진 path 면 기본 선택
    c.on_results(res(3, Mode::Path, vec![dir("C:\\z1"), dir("C:\\z2")], RunState::Done));
    assert_eq!(c.view().cursor, Some(0));
    // 입력이 바뀌면 이동 여부 초기화: 첫 항목
    c.on_key(&k("ArrowDown"), "C:\\a");
    c.on_input_changed("C:\\z", false);
    c.on_results(res(4, Mode::Path, vec![dir("C:\\z2"), dir("C:\\z1")], RunState::Done));
    assert_eq!(c.view().cursor, Some(0));
}

#[test]
fn no_cursor_arrow_keys_pick_ends() {
    let mut c = plain(None);
    typed(&mut c, "c:\\", 1, vec![dir("C:\\a"), dir("C:\\b"), dir("C:\\c")]);
    assert_eq!(c.view().cursor, None);
    c.on_key(&k("ArrowUp"), "c:\\");
    assert_eq!(c.view().cursor, Some(2));
    // 다시 입력하면 선택 없음으로, 아래 화살표는 첫 항목
    let mut c = plain(None);
    typed(&mut c, "c:\\", 1, vec![dir("C:\\a"), dir("C:\\b")]);
    c.on_key(&k("ArrowDown"), "c:\\");
    assert_eq!(c.view().cursor, Some(0));
    // 화살표로 직접 고른 항목은 Enter 가 바로 복사
    let o = c.on_key(&k("Enter"), "c:\\");
    assert_eq!(o.commands, vec![copy_cmd(&["C:\\a"])]);
}

#[test]
fn no_cursor_persists_for_following_results() {
    let mut c = plain(None);
    typed(&mut c, "c:\\", 1, vec![dir("C:\\a")]);
    c.on_results(res(2, Mode::Path, vec![dir("C:\\a"), dir("C:\\b")], RunState::Running));
    assert_eq!(c.view().cursor, None);
    // 폴더를 가리키지 않는 입력으로 바뀌면 선택이 생긴다
    typed(&mut c, "c:\\a", 3, vec![dir("C:\\a")]);
    assert_eq!(c.view().cursor, Some(0));
}

#[test]
fn paging_and_clamping() {
    let mut c = plain(Some(Mode::Fuzzy));
    c.set_page_size(3);
    let items: Vec<ResultItem> = (0..10).map(|i| item(&format!("C:\\f{i}"), false, false)).collect();
    c.on_results(res(1, Mode::Fuzzy, items, RunState::Done));
    c.on_key(&k("PageDown"), "");
    assert_eq!(c.view().cursor, Some(3));
    c.on_key(&k("PageDown"), "");
    c.on_key(&k("PageDown"), "");
    c.on_key(&k("PageDown"), "");
    assert_eq!(c.view().cursor, Some(9));
    c.on_key(&k("PageUp"), "");
    assert_eq!(c.view().cursor, Some(6));
    c.on_key(&ck("p"), "");
    assert_eq!(c.view().cursor, Some(5));
    c.on_key(&ck("n"), "");
    assert_eq!(c.view().cursor, Some(6));
    c.on_row_click(1);
    assert_eq!(c.view().cursor, Some(1));
}

#[test]
fn status_texts() {
    let mut c = plain(Some(Mode::Fuzzy));
    assert_eq!(c.view().status, "준비됨");
    let mut p = res(1, Mode::Fuzzy, vec![item("C:\\a", false, false)], RunState::Running);
    p.scanned = 1234;
    p.matched = 56;
    p.elapsed_ms = 1200;
    c.on_results(p);
    let v = c.view();
    assert_eq!(v.status, "검색 중… 1,234개 스캔 · 56개 일치 · 1.2s");
    assert_eq!(v.status_kind, StatusKind::Running);

    let mut p = res(2, Mode::Fuzzy, vec![item("C:\\a", false, false)], RunState::Done);
    p.scanned = 1_234_567;
    p.matched = 1000;
    p.elapsed_ms = 42;
    c.on_results(p);
    let v = c.view();
    assert_eq!(v.status, "완료 · 1,000개 일치 / 1,234,567개 스캔 · 42ms");
    assert_eq!(v.status_kind, StatusKind::Normal);

    let mut p = res(3, Mode::Fuzzy, vec![], RunState::Done);
    p.message = Some("힌트".into());
    p.cache_note = Some("캐시".into());
    p.elapsed_ms = 100;
    c.on_results(p);
    assert_eq!(c.view().status, "완료 · 0개 일치 / 0개 스캔 · 0.1s · 힌트 · 캐시");

    let mut p = res(4, Mode::Fuzzy, vec![], RunState::Error);
    p.message = Some("fd 실패".into());
    p.cache_note = Some("캐시".into());
    c.on_results(p);
    let v = c.view();
    assert_eq!(v.status, "fd 실패 · 캐시");
    assert_eq!(v.status_kind, StatusKind::Error);

    c.on_results(res(5, Mode::Fuzzy, vec![], RunState::Error));
    assert_eq!(c.view().status, "오류");

    c.on_results(res(6, Mode::Fuzzy, vec![], RunState::Idle));
    assert_eq!(c.view().status, "준비됨");
}

#[test]
fn elapsed_formatting() {
    let mut c = plain(Some(Mode::Fuzzy));
    for (n, (ms, want)) in [(0u64, "0ms"), (99, "99ms"), (100, "0.1s"), (1250, "1.3s"), (1249, "1.2s"), (12_340, "12.3s"), (1750, "1.8s")].into_iter().enumerate() {
        let mut p = res(n as u64 + 1, Mode::Fuzzy, vec![], RunState::Done);
        p.elapsed_ms = ms;
        c.on_results(p);
        assert!(c.view().status.ends_with(want), "{ms} -> {}", c.view().status);
    }
}

#[test]
fn recent_mode_status_and_empty_hints() {
    let mut c = plain(Some(Mode::Path));
    c.on_key(&ck("l"), "");
    let mut p = res(1, Mode::Recent, vec![], RunState::Done);
    p.matched = 0;
    c.on_results(p);
    let v = c.view();
    assert_eq!(v.status, "0개 일치");
    assert_eq!(v.empty_hint.as_deref(), Some("최근 사용 기록이 없습니다"));
    // 입력이 있으면 일반 문구
    c.on_input_changed("zz", false);
    c.on_results(res(2, Mode::Recent, vec![], RunState::Done));
    assert_eq!(c.view().empty_hint.as_deref(), Some("일치하는 항목이 없습니다"));
}

#[test]
fn empty_hint_conditions() {
    let mut c = plain(Some(Mode::Fuzzy));
    c.on_results(res(1, Mode::Fuzzy, vec![], RunState::Running));
    assert_eq!(c.view().empty_hint, None);
    c.on_results(res(2, Mode::Fuzzy, vec![], RunState::Done));
    assert_eq!(c.view().empty_hint.as_deref(), Some("일치하는 항목이 없습니다"));
    c.on_results(res(3, Mode::Fuzzy, vec![], RunState::Idle));
    assert_eq!(c.view().empty_hint.as_deref(), Some("일치하는 항목이 없습니다"));
    c.on_results(res(4, Mode::Fuzzy, vec![], RunState::Error));
    assert_eq!(c.view().empty_hint, None);
    c.on_results(res(5, Mode::Fuzzy, vec![item("C:\\a", false, false)], RunState::Done));
    assert_eq!(c.view().empty_hint, None);
}

#[test]
fn startup_notes_until_first_input() {
    let mut c = make(None, vec![], vec!["경고1", "경고2"]);
    let v = c.view();
    assert_eq!(v.status, "경고1 · 경고2 · 준비됨");
    assert_eq!(v.status_kind, StatusKind::Error);
    // 팝업 재표시로는 사라지지 않는다
    c.on_show();
    assert!(c.view().status.starts_with("경고1 · 경고2"));
    // 첫 입력 후 사라진다
    c.on_input_changed("a", false);
    assert_eq!(c.view().status, "준비됨");
    assert_eq!(c.view().status_kind, StatusKind::Normal);
}

#[test]
fn input_invalid_rules() {
    let mut c = plain(None);
    // 이름 필터가 있는데 폴더 항목이 하나도 없음
    typed(&mut c, "C:\\apps\\zzz", 1, vec![]);
    assert!(c.view().input_invalid);
    // 최근 항목만 있어도 invalid
    typed(&mut c, "C:\\apps\\zzz", 2, vec![item("C:\\old\\z", true, true)]);
    assert!(c.view().input_invalid);
    // 폴더 항목이 있으면 valid
    typed(&mut c, "C:\\apps\\b", 3, vec![dir("C:\\apps\\bin")]);
    assert!(!c.view().input_invalid);
    // 끝 구분자(필터 없음)면 항목이 없어도 valid
    typed(&mut c, "C:\\apps\\", 4, vec![]);
    assert!(!c.view().input_invalid);
    // 오류 상태는 invalid
    c.on_input_changed("C:\\nope\\", false);
    c.on_results(res(5, Mode::Path, vec![], RunState::Error));
    assert!(c.view().input_invalid);
    // 빈 입력은 valid
    c.on_input_changed("", false);
    c.on_results(res(6, Mode::Path, vec![], RunState::Error));
    assert!(!c.view().input_invalid);
    // fuzzy 모드는 항상 valid
    let mut c = plain(Some(Mode::Fuzzy));
    typed(&mut c, "abc", 1, vec![]);
    assert!(!c.view().input_invalid);
    // 모드 전환 시 해제
    let mut c = plain(None);
    typed(&mut c, "C:\\x\\q", 1, vec![]);
    assert!(c.view().input_invalid);
    c.on_key(&ck("l"), "C:\\x\\q");
    assert!(!c.view().input_invalid);
}

#[test]
fn composing_ignores_command_keys() {
    let mut c = plain(None);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    for name in ["Enter", "Tab", "Escape", "ArrowDown", "/"] {
        let mut ev = k(name);
        ev.composing = true;
        let o = c.on_key(&ev, "C:\\ap");
        assert!(!o.handled && o.commands.is_empty(), "{name}");
    }
    let mut ev = ck("l");
    ev.composing = true;
    let o = c.on_key(&ev, "C:\\ap");
    assert!(!o.handled && o.commands.is_empty());
    assert_eq!(c.view().mode, Mode::Path);
    // 조합 중 입력 변경은 질의한다
    assert_eq!(c.on_input_changed("C:\\apㅂ", true), vec![query("C:\\apㅂ", Mode::Path, false)]);
}

#[test]
fn on_show_resets_everything() {
    let mut c = make(None, vec![act(0, "A", "", true)], vec![]);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps"), dir("C:\\apx")]);
    c.on_key(&ck("Space"), "C:\\ap");
    c.on_key(&ck("k"), "C:\\ap");
    assert!(c.view().menu_open);
    c.set_page_size(5);
    let cmds = c.on_show();
    assert_eq!(cmds, vec![Command::SetInput(String::new()), query("", Mode::Path, false)]);
    let v = c.view();
    assert!(!v.menu_open);
    assert!(v.rows.is_empty());
    assert_eq!(v.cursor, None);
    assert_eq!(v.status, "준비됨");
    assert!(!v.input_invalid);
    assert_eq!(v.empty_hint, None);
    // 선택 집합도 비워졌다
    c.on_results(res(9, Mode::Path, vec![dir("C:\\apps")], RunState::Done));
    assert!(!c.view().rows[0].picked);
}

#[test]
fn action_result_handling() {
    let mut c = plain(None);
    assert_eq!(c.on_action_result(Ok(()), true), vec![Command::Hide]);
    assert!(c.on_action_result(Ok(()), false).is_empty());
    assert!(c.on_action_result(Err("실패했다".into()), true).is_empty());
    let v = c.view();
    assert_eq!(v.status, "실패했다");
    assert_eq!(v.status_kind, StatusKind::Error);
    // 다음 결과 도착 시 오류 표시 해제
    c.on_results(res(1, Mode::Path, vec![dir("C:\\a")], RunState::Done));
    assert_ne!(c.view().status, "실패했다");
}

#[test]
fn echo_of_set_input_is_ignored_once() {
    let mut c = plain(None);
    typed(&mut c, "C:\\ap", 1, vec![dir("C:\\apps")]);
    let o = c.on_key(&k("Tab"), "C:\\ap");
    assert_eq!(o.commands.len(), 2);
    assert!(c.on_input_changed("C:\\apps\\", false).is_empty());
    // 두 번째는 정상 처리(중복 안전)
    assert_eq!(c.on_input_changed("C:\\apps\\", false), vec![query("C:\\apps\\", Mode::Path, false)]);
}

#[test]
fn echo_does_not_swallow_later_real_edit() {
    let mut c = plain(None);
    c.on_show();
    // 에코가 오지 않은 채로 사용자가 입력
    let o = c.on_key(&k("a"), "");
    assert!(!o.handled);
    assert_eq!(c.on_input_changed("a", false), vec![query("a", Mode::Path, false)]);
}

#[test]
fn mode_switch_does_not_leak_no_cursor() {
    // TS 와 다른 점: 선택 없음 상태에서 Ctrl+L 해도 새 모드의 첫 결과에는 기본 선택이 있다
    let mut c = plain(None);
    typed(&mut c, "c:", 1, vec![dir("C:\\a")]);
    assert_eq!(c.view().cursor, None);
    c.on_key(&ck("l"), "c:");
    c.on_results(res(2, Mode::Recent, vec![item("C:\\r", true, true)], RunState::Done));
    assert_eq!(c.view().cursor, Some(0));
}

#[test]
fn unhandled_character_keys() {
    let mut c = plain(None);
    for name in ["a", "A", "1", "Backspace", "Delete", "Home", "End", "ArrowLeft"] {
        let o = c.on_key(&k(name), "x");
        assert!(!o.handled && o.commands.is_empty(), "{name}");
    }
    // 경로 모드가 아니면 '/' 는 EDIT 로
    let mut c = plain(Some(Mode::Fuzzy));
    let o = c.on_key(&k("/"), "x");
    assert!(!o.handled);
}
