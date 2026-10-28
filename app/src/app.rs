//! 창 생성, 윈도 프로시저, 팝업 수명(표시/숨김/배치), 컨트롤러 연동과 명령 실행기.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::Arc;

use fdpick_core::actions;
use fdpick_core::config::Config;
use fdpick_core::engine::{Engine, Mode, ResultsUpdated};
use fdpick_core::history;
use fdpick_core::user_actions;
use fdpick_logic::view::{Command, KeyEvent};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Controls::{EM_SETMARGINS, EM_SETSEL};
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::{self, metrics_of, palette, sc};
use crate::explorer;
use crate::hotkey;
use crate::input::{edit_proc, edit_text};
use crate::paint::{self, chip_rect, layout, L_WIN_H, L_WIN_W};
use crate::state::*;
use crate::tray;
use crate::util::{hi_word, lo_word, point_of, w};

/// 모니터 작업 영역 높이 대비 팝업 상단 위치
const TOP_RATIO: i32 = 15;
/// 포커스 상실로 숨긴 직후 트레이 클릭이 다시 열지 않도록 하는 시간(ms)
const BLUR_CLICK_GUARD_MS: u64 = 300;

// ---------------------------------------------------------------- 창 생성

/// 메인 창과 입력창(EDIT 자식)을 만든다. 숨겨진 상태로 둔다.
pub unsafe fn create_window(hinst: HINSTANCE, class: &str) -> Result<(), String> {
    let a = st();
    a.hinst = hinst;
    let cls = w(class);
    let mut wc: WNDCLASSEXW = zeroed();
    wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
    wc.lpfnWndProc = Some(wnd_proc);
    wc.hInstance = hinst;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.hIcon = LoadIconW(hinst, tray::ICON_RES_ID as usize as *const u16);
    wc.lpszClassName = cls.as_ptr();
    if RegisterClassExW(&wc) == 0 {
        return Err("창 클래스 등록 실패".into());
    }
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        cls.as_ptr(),
        w("fdpick").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN,
        0,
        0,
        100,
        100,
        null_mut(),
        null_mut(),
        hinst,
        null(),
    );
    if hwnd.is_null() {
        return Err("창 생성 실패".into());
    }
    a.hwnd = hwnd;

    // Windows 11 둥근 모서리. 실패(Windows 10 등)하면 SetWindowRgn 폴백
    let pref: i32 = DWMWCP_ROUND;
    let hr = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE as u32, &pref as *const i32 as *const _, 4);
    a.dwm_round_ok = hr >= 0;

    // 입력창: 시스템 EDIT 자식(IME 는 시스템이 처리). WM_SETFONT 는 DPI 적용 후에 보낸다.
    let edit = CreateWindowExW(
        0,
        w("EDIT").as_ptr(),
        null(),
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
        0,
        0,
        100,
        20,
        hwnd,
        null_mut(),
        hinst,
        null(),
    );
    if edit.is_null() {
        return Err("입력창 생성 실패".into());
    }
    a.edit = edit;
    SendMessageW(edit, EM_SETMARGINS, 3, 0); // EC_LEFTMARGIN | EC_RIGHTMARGIN = 0
    let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
    a.orig_edit_proc = std::mem::transmute::<isize, WNDPROC>(old);

    a.taskbar_created = RegisterWindowMessageW(w("TaskbarCreated").as_ptr());
    gfx::refresh_theme();
    place_window();
    Ok(())
}

fn st() -> &'static mut App {
    crate::state::app()
}

// ---------------------------------------------------------------- 배치/DPI

/// DPI 적용 후 입력창을 칩 오른쪽에 맞춰 다시 배치한다.
unsafe fn layout_edit() {
    let a = st();
    if a.edit.is_null() {
        return;
    }
    SendMessageW(a.edit, WM_SETFONT, a.f_input as WPARAM, 1);
    let hdc = GetDC(a.hwnd);
    let label = a.view.as_ref().map(|v| v.mode_label.as_str()).unwrap_or("");
    let left = match chip_rect(hdc, label) {
        Some(r) => r.right + sc(9.5),
        None => paint::pad(),
    };
    let th = metrics_of(hdc, a.f_input).tmHeight;
    ReleaseDC(a.hwnd, hdc);
    let l = layout();
    MoveWindow(a.edit, left, (l.bar_h - th) / 2, l.w - left - paint::edit_right_pad(), th, 1);
}

fn notify_page_size() {
    let n = paint::page_size();
    if let Some(c) = st().ctl.as_mut() {
        c.set_page_size(n);
    }
}

/// DPI 변경: 글꼴 재생성 + 입력창 재배치 + 스크롤 보정
unsafe fn apply_dpi(dpi: u32) {
    if !gfx::apply_dpi(dpi) {
        return;
    }
    layout_edit();
    let rows = st().view.as_ref().map(|v| v.rows.len()).unwrap_or(0);
    st().scroll = st().scroll.clamp(0, paint::max_scroll(rows));
    notify_page_size();
}

/// 커서가 있는 모니터의 중앙 상단에 배치(그 모니터 DPI 기준)
unsafe fn place_window() {
    let a = st();
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut mi);
    let (mut dx, mut dy) = (96u32, 96u32);
    GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    apply_dpi(dx);
    let (ww, wh) = (sc(L_WIN_W), sc(L_WIN_H));
    let work = mi.rcWork;
    let x = work.left + ((work.right - work.left) - ww) / 2;
    let y = work.top + (work.bottom - work.top) * TOP_RATIO / 100;
    SetWindowPos(a.hwnd, HWND_TOPMOST, x, y, ww, wh, SWP_NOACTIVATE);
    // 이동 중 WM_DPICHANGED 로 DPI 가 바뀌었을 수 있어 한 번 더 확정
    apply_dpi(dx);
    SetWindowPos(a.hwnd, HWND_TOPMOST, x, y, ww, wh, SWP_NOACTIVATE);
    if !a.dwm_round_ok {
        let r = 2 * sc(12.0);
        SetWindowRgn(a.hwnd, CreateRoundRectRgn(0, 0, ww + 1, wh + 1, r, r), 1);
    }
}

// ---------------------------------------------------------------- 표시/숨김

/// 팝업을 연다. `mode` 는 단축키가 정한 시작 모드. 이미 떠 있으면 상태를 유지하고 앞으로 가져온다.
pub unsafe fn show_popup(mode: Mode) {
    let a = st();
    if a.visible {
        // 이미 떠 있으면 상태는 유지하고 앞으로 가져온다
        SetForegroundWindow(a.hwnd);
        SetFocus(a.edit);
        return;
    }
    let fg = GetForegroundWindow();
    a.prev_fg = if fg == a.hwnd { 0 } else { fg as usize };
    if gfx::refresh_theme() {
        InvalidateRect(a.edit, null(), 1);
    }
    place_window();
    // 입력 비우기: 컨트롤러는 on_show 에서 스스로 초기화하므로 이 EN_CHANGE 는 넘기지 않는다
    a.suppress_change = true;
    SetWindowTextW(a.edit, w("").as_ptr());
    a.suppress_change = false;
    a.composing = false;
    a.visible = true;
    a.scroll = 0;
    let cmds = a.ctl.as_mut().map(|c| c.on_show(mode)).unwrap_or_default();
    exec(cmds);
    refresh_view(true);
    ShowWindow(a.hwnd, SW_SHOW);
    SetForegroundWindow(a.hwnd);
    SetFocus(a.edit);
}

/// 숨긴다. 엔진은 그대로 둔다. `by_blur` 는 포커스 상실로 숨긴 경우.
pub unsafe fn hide_popup(by_blur: bool) {
    let a = st();
    if !a.visible {
        return;
    }
    a.visible = false;
    if by_blur {
        a.blur_hidden_at = GetTickCount64();
    }
    ShowWindow(a.hwnd, SW_HIDE);
    if let Some(c) = a.ctl.as_mut() {
        c.on_blur();
    }
    // 숨겨진 동안 작업 집합을 줄인다
    SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
}

/// 전역 단축키. 닫혀 있으면 그 모드로 열고, 같은 모드로 열려 있으면 닫고, 다른 모드로 열려 있으면 모드만 바꾼다.
unsafe fn on_hotkey(mode: Mode) {
    let a = st();
    if !a.visible {
        show_popup(mode);
        return;
    }
    let cur = a.ctl.as_ref().map(|c| c.view().mode);
    if cur == Some(mode) {
        hide_popup(false);
        return;
    }
    let cmds = a.ctl.as_mut().map(|c| c.switch_to(mode)).unwrap_or_default();
    exec(cmds);
    refresh_view(true);
    SetForegroundWindow(a.hwnd);
    SetFocus(a.edit);
}

/// 검색 이력을 모두 지운다(확인 후). 저장 파일도 비운다. 예 = 사용 이력(최근 목록)도 함께, 아니요 = 검색 이력만.
unsafe fn clear_search_history() {
    let a = st();
    let msg = w("검색 이력을 모두 지울까요?

[예] 사용 이력(최근 목록)도 함께 지움
[아니요] 검색 이력만 지움");
    let r = MessageBoxW(a.hwnd, msg.as_ptr(), w("fdpick").as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION);
    if r != IDYES && r != IDNO {
        return;
    }
    let cmds = a.ctl.as_mut().map(|c| c.clear_history()).unwrap_or_default();
    exec(cmds);
    if r == IDYES {
        if let Some(engine) = a.engine.as_ref() {
            engine.clear_recents();
        }
    }
}

// ---------------------------------------------------------------- 뷰 갱신

/// 컨트롤러의 `view()` 를 가져와 바뀐 경우에만 다시 그린다. 숨겨진 동안은 건너뛴다(force 제외).
pub unsafe fn refresh_view(force: bool) {
    let a = st();
    if !a.visible && !force {
        return;
    }
    let Some(ctl) = a.ctl.as_ref() else { return };
    let nv = ctl.view();
    let old = a.view.take();
    if old.as_ref() == Some(&nv) {
        a.view = old;
        return;
    }
    let (label_changed, chrome_changed, rows_changed, cursor_changed) = match &old {
        Some(o) => (
            o.mode_label != nv.mode_label,
            o.input_invalid != nv.input_invalid || o.placeholder != nv.placeholder,
            o.rows != nv.rows,
            o.cursor != nv.cursor,
        ),
        None => (true, true, true, true),
    };
    let (cursor, rows) = (nv.cursor, nv.rows.len());
    a.view = Some(nv);
    if label_changed {
        layout_edit();
    }
    if chrome_changed || label_changed {
        InvalidateRect(a.edit, null(), 1);
    }
    // 스크롤: 커서가 바뀌면 보이게, 결과가 바뀌어 커서가 맨 위/없음이면 맨 위로
    if rows_changed || cursor_changed {
        match cursor {
            Some(c) if c < rows && (cursor_changed || c > 0) => paint::ensure_visible(c, rows),
            _ => a.scroll = 0,
        }
        a.scroll = a.scroll.clamp(0, paint::max_scroll(rows));
    }
    InvalidateRect(a.hwnd, null(), 0);
}

// ---------------------------------------------------------------- 컨트롤러 연동

/// EN_CHANGE(및 조합 종료) -> 컨트롤러
pub unsafe fn on_edit_changed() {
    let a = st();
    if a.suppress_change {
        return;
    }
    let text = edit_text(a.edit);
    let composing = a.composing;
    let cmds = a.ctl.as_mut().map(|c| c.on_input_changed(&text, composing)).unwrap_or_default();
    exec(cmds);
    refresh_view(false);
}

/// 키 입력 -> 컨트롤러. 처리됐으면 true(EDIT 에 전달하지 않는다).
pub unsafe fn on_key(ev: &KeyEvent, text: &str) -> bool {
    let a = st();
    let Some(ctl) = a.ctl.as_mut() else { return false };
    let out = ctl.on_key(ev, text);
    exec(out.commands);
    refresh_view(false);
    out.handled
}

fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::Fuzzy => "fuzzy",
        Mode::Path => "path",
        Mode::Recent => "recent",
    }
}

unsafe fn set_input(text: &str) {
    let a = st();
    if edit_text(a.edit) == text {
        return;
    }
    SendMessageW(a.edit, EM_SETSEL, 0, -1);
    SetWindowTextW(a.edit, w(text).as_ptr());
    let n = GetWindowTextLengthW(a.edit).max(0) as isize;
    SendMessageW(a.edit, EM_SETSEL, n as WPARAM, n);
}

/// 질의는 마지막 값만 남겨 합쳤다가, 현재 큐에 쌓인 입력 메시지를 처리한 뒤 한 번 보낸다.
unsafe fn queue_query(text: String, mode: Mode, force: bool) {
    let a = st();
    let force = force || a.pending_query.as_ref().map(|q| q.2).unwrap_or(false);
    a.pending_query = Some((text, mode, force));
    if !a.flush_posted {
        a.flush_posted = true;
        PostMessageW(a.hwnd, WM_APP_FLUSH, 0, 0);
    }
}

unsafe fn flush_query() {
    let a = st();
    a.flush_posted = false;
    if let (Some((text, mode, force)), Some(engine)) = (a.pending_query.take(), a.engine.as_ref()) {
        engine.query_changed(&text, mode_str(mode), force);
    }
}

/// 컨트롤러가 요청한 명령을 실행한다.
pub unsafe fn exec(cmds: Vec<Command>) {
    for c in cmds {
        match c {
            Command::SetInput(t) => set_input(&t),
            Command::Query { text, mode, force } => queue_query(text, mode, force),
            Command::Hide => hide_popup(false),
            Command::RunAction { kind, paths } => run_action(kind, paths),
            Command::RunUserAction { id, paths, close_popup } => run_user_action(id, paths, close_popup),
            Command::SaveHistory(items) => {
                if let Some(p) = history::default_path() {
                    history::save(&p, &items);
                }
            }
        }
    }
}

/// 성공한 동작의 대상만 최근 목록에 올린다(이전 Tauri 구현과 같은 규칙).
fn recorded_paths(kind: &str, paths: &[String]) -> Vec<String> {
    let norm: Vec<String> = paths.iter().map(|p| actions::normalize_path(p)).filter(|p| !p.is_empty()).collect();
    match kind {
        "copy_dir" => norm.iter().map(|p| actions::to_dir_path(p)).collect(),
        "reveal" | "open" | "to_explorer" => norm.into_iter().take(1).collect(),
        _ => norm,
    }
}

fn post_done(hwnd: usize, done: ActionDone) {
    let ptr = Box::into_raw(Box::new(done));
    unsafe {
        if PostMessageW(hwnd as HWND, WM_APP_ACTION, 0, ptr as LPARAM) == 0 {
            drop(Box::from_raw(ptr));
        }
    }
}

/// 작업 스레드에서 실행해 UI 를 막지 않는다. 스레드는 작업이 끝나면 종료된다.
unsafe fn run_action(kind: String, paths: Vec<String>) {
    let a = st();
    let (Some(cfg), Some(engine)) = (a.config.clone(), a.engine.clone()) else { return };
    let hwnd = a.hwnd as usize;
    let prev_fg = a.prev_fg;
    let spawned = std::thread::Builder::new().name("fdpick-action".into()).spawn(move || {
        let opts = actions::CopyOptions { quote: cfg.copy.quote, forward_slash: cfg.copy.forward_slash };
        // to_explorer 는 직전 포커스가 탐색기이면 그 창을 이동하고, 아니면 reveal 처럼 새 창을 연다.
        let result = if kind == "to_explorer" {
            paths.first().map(|p| explorer::goto(prev_fg, p)).unwrap_or(Ok(false))
        } else {
            actions::run_with(&kind, &paths, &opts).map(|_| true)
        };
        let result = result.map(|acted| {
            if acted {
                engine.record_recent(&recorded_paths(&kind, &paths));
            }
        });
        // 내장 동작은 성공하면 팝업을 닫는 것이 기본 흐름
        post_done(hwnd, ActionDone { result, close_popup: true });
    });
    if spawned.is_err() {
        post_done(hwnd, ActionDone { result: Err("작업 스레드를 만들 수 없습니다".into()), close_popup: false });
    }
}

unsafe fn run_user_action(id: usize, paths: Vec<String>, close_popup: bool) {
    let a = st();
    let (Some(cfg), Some(engine)) = (a.config.clone(), a.engine.clone()) else { return };
    let hwnd = a.hwnd as usize;
    let spawned = std::thread::Builder::new().name("fdpick-action".into()).spawn(move || {
        let result = user_actions::run(&cfg.actions, id, &paths);
        if result.is_ok() {
            let norm: Vec<String> = paths.iter().map(|p| actions::normalize_path(p)).filter(|p| !p.is_empty()).collect();
            engine.record_recent(&norm);
        }
        post_done(hwnd, ActionDone { result, close_popup });
    });
    if spawned.is_err() {
        post_done(hwnd, ActionDone { result: Err("작업 스레드를 만들 수 없습니다".into()), close_popup: false });
    }
}

/// 엔진 sink 가 쓰는 전송 함수: 결과를 Box 로 만들어 UI 스레드로 넘긴다(어느 스레드에서든 호출 가능).
pub fn post_results(hwnd: usize, p: ResultsUpdated) {
    let ptr = Box::into_raw(Box::new(p));
    unsafe {
        if PostMessageW(hwnd as HWND, WM_APP_RESULTS, 0, ptr as LPARAM) == 0 {
            drop(Box::from_raw(ptr));
        }
    }
}

/// 설정/엔진을 상태에 넣는다.
pub fn install(config: Arc<Config>, engine: Arc<Engine>, ctl: fdpick_logic::controller::Controller) {
    let a = st();
    a.config = Some(config);
    a.engine = Some(engine);
    a.ctl = Some(ctl);
    notify_page_size();
}

// ---------------------------------------------------------------- 윈도 프로시저

unsafe fn on_activate(wp: WPARAM, lp: LPARAM) {
    let a = st();
    if lo_word(wp) != 0 || !a.visible || a.in_menu {
        return;
    }
    // 같은 프로세스의 다른 창(IME 후보 창 등)으로 넘어간 것은 포커스 상실이 아니다
    let other = lp as HWND;
    if !other.is_null() {
        let mut pid = 0u32;
        GetWindowThreadProcessId(other, &mut pid);
        if pid == GetCurrentProcessId() {
            return;
        }
    }
    let hide = a.config.as_ref().map(|c| c.hide_on_blur).unwrap_or(true);
    if hide {
        hide_popup(true); // 내부에서 on_blur 호출
    } else if let Some(c) = a.ctl.as_mut() {
        c.on_blur();
    }
}

/// 같은 실행 파일을 `--restart` 로 새로 띄우고 이 인스턴스를 종료한다.
/// 새 프로세스는 단일 인스턴스 뮤텍스가 풀릴 때까지 기다린 뒤 설정을 다시 읽으며 시작한다.
/// 새 프로세스를 만들지 못하면 종료하지 않고 상태를 유지한다.
unsafe fn restart() {
    let a = st();
    let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--restart").spawn());
    match spawned {
        Ok(_) => {
            DestroyWindow(a.hwnd);
        }
        Err(e) => {
            let msg = w(&format!("다시 시작하지 못했습니다: {e}"));
            MessageBoxW(a.hwnd, msg.as_ptr(), w("fdpick").as_ptr(), MB_OK | MB_ICONERROR);
        }
    }
}

unsafe fn tray_message(lp: LPARAM) {
    let a = st();
    match (lp & 0xffff) as u32 {
        WM_LBUTTONUP => {
            if a.visible {
                hide_popup(false);
            } else if GetTickCount64().saturating_sub(a.blur_hidden_at) > BLUR_CLICK_GUARD_MS {
                show_popup(Mode::Fuzzy);
            }
        }
        WM_RBUTTONUP => match tray::show_menu() {
            tray::ID_MENU_OPEN => show_popup(Mode::Fuzzy),
            tray::ID_MENU_CLEAR_HISTORY => clear_search_history(),
            tray::ID_MENU_RESTART => restart(),
            tray::ID_MENU_QUIT => {
                DestroyWindow(a.hwnd);
            }
            _ => {}
        },
        _ => {}
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let a = st();
    if a.taskbar_created != 0 && msg == a.taskbar_created {
        tray::add();
        return 0;
    }
    match msg {
        WM_PAINT => {
            paint::on_paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let p = palette();
            let invalid = a.view.as_ref().map(|v| v.input_invalid).unwrap_or(false);
            SetTextColor(wp as HDC, if invalid { p.invalid } else { p.text });
            SetBkColor(wp as HDC, p.panel_bg);
            a.brush_bg as LRESULT
        }
        WM_COMMAND => {
            if hi_word(wp) == EN_CHANGE && lp as HWND == a.edit {
                on_edit_changed();
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            let l = layout();
            let rows = a.view.as_ref().map(|v| v.rows.len()).unwrap_or(0);
            let ns = (a.scroll - delta * l.row_h * 3 / 120).clamp(0, paint::max_scroll(rows));
            if ns != a.scroll {
                a.scroll = ns;
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let (_, y) = point_of(lp);
            let rows = a.view.as_ref().map(|v| v.rows.len()).unwrap_or(0);
            if let Some(i) = paint::row_at(y, rows) {
                if let Some(c) = a.ctl.as_mut() {
                    c.on_row_click(i);
                }
                refresh_view(false);
            }
            SetFocus(a.edit);
            0
        }
        WM_SETFOCUS => {
            SetFocus(a.edit);
            0
        }
        WM_ACTIVATE => {
            on_activate(wp, lp);
            0
        }
        WM_HOTKEY => {
            // 1차 단축키는 검색 모드, 2차 단축키는 경로 모드로 시작한다
            match wp as i32 {
                hotkey::ID_HOTKEY => on_hotkey(Mode::Fuzzy),
                hotkey::ID_HOTKEY2 => on_hotkey(Mode::Path),
                _ => {}
            }
            0
        }
        WM_APP_SHOW => {
            show_popup(Mode::Fuzzy);
            0
        }
        WM_APP_TRAY => {
            tray_message(lp);
            0
        }
        WM_APP_FLUSH => {
            flush_query();
            0
        }
        WM_APP_RESULTS => {
            let p = Box::from_raw(lp as *mut ResultsUpdated);
            if let Some(c) = a.ctl.as_mut() {
                c.on_results(*p);
            }
            refresh_view(false);
            0
        }
        WM_APP_ACTION => {
            let d = Box::from_raw(lp as *mut ActionDone);
            let cmds = a.ctl.as_mut().map(|c| c.on_action_result(d.result, d.close_popup)).unwrap_or_default();
            exec(cmds);
            refresh_view(false);
            0
        }
        WM_DPICHANGED => {
            apply_dpi(lo_word(wp));
            0
        }
        WM_SETTINGCHANGE => {
            if gfx::refresh_theme() {
                InvalidateRect(hwnd, null(), 0);
                InvalidateRect(a.edit, null(), 1);
            }
            0
        }
        // Alt+F4 등은 종료가 아니라 숨김(종료는 트레이 메뉴)
        WM_CLOSE => {
            hide_popup(false);
            0
        }
        WM_DESTROY => {
            hotkey::unregister(hwnd);
            tray::remove();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
