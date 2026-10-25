//! UI 스레드 전역 상태. 창/컨트롤러/엔진/GDI 객체를 한곳에 둔다.
//! 모든 접근은 UI 스레드에서만 일어난다(엔진/작업 스레드는 PostMessage 로만 UI 에 알린다).

use std::ptr::null_mut;
use std::sync::Arc;

use fdpick_core::config::Config;
use fdpick_core::engine::{Engine, Mode};
use fdpick_logic::controller::Controller;
use fdpick_logic::view::ViewState;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{HBRUSH, HFONT};
use windows_sys::Win32::UI::WindowsAndMessaging::WNDPROC;

// 사용자 정의 창 메시지
pub const WM_APP_RESULTS: u32 = 0x8000 + 1; // lParam: Box<ResultsUpdated> 소유권
pub const WM_APP_ACTION: u32 = 0x8000 + 2; // lParam: Box<ActionDone> 소유권
pub const WM_APP_SHOW: u32 = 0x8000 + 3; // 두 번째 실행이 보냄
pub const WM_APP_FLUSH: u32 = 0x8000 + 4; // 합쳐 둔 질의 전송
pub const WM_APP_TRAY: u32 = 0x8000 + 5; // 트레이 콜백

/// 작업 스레드 -> UI 스레드: 액션 완료 통지
pub struct ActionDone {
    pub result: Result<(), String>,
    pub close_popup: bool,
}

pub struct App {
    pub hwnd: HWND,
    pub edit: HWND,
    pub hinst: HINSTANCE,
    pub dpi: u32,
    pub dark: bool,
    pub dwm_round_ok: bool,
    pub face: String,
    // 그리기용 GDI 객체: DPI 가 바뀔 때만 다시 만든다
    pub f_input: HFONT,
    pub f_name: HFONT,
    pub f_dir: HFONT,
    pub f_small: HFONT,
    pub f_chip: HFONT,
    pub f_badge: HFONT,
    pub brush_bg: HBRUSH,

    pub config: Option<Arc<Config>>,
    pub engine: Option<Arc<Engine>>,
    pub ctl: Option<Controller>,
    /// 마지막으로 그린 뷰(변경 감지용)
    pub view: Option<ViewState>,

    pub visible: bool,
    pub scroll: i32,
    pub composing: bool,
    /// 프로그램이 입력창을 비우는 동안 EN_CHANGE 를 컨트롤러에 넘기지 않는다
    pub suppress_change: bool,
    /// 컨텍스트 메뉴/트레이 메뉴가 떠 있는 동안 포커스 상실로 숨기지 않는다
    pub in_menu: bool,
    pub orig_edit_proc: WNDPROC,
    pub pending_query: Option<(String, Mode, bool)>,
    pub flush_posted: bool,

    pub hotkey_text: String,
    pub hotkey_ok: bool,
    pub taskbar_created: u32,
    /// 포커스 상실로 숨긴 시각(GetTickCount64). 트레이 클릭과 겹칠 때 다시 열리는 것을 막는다.
    pub blur_hidden_at: u64,
}

impl App {
    const fn new() -> Self {
        App {
            hwnd: null_mut(),
            edit: null_mut(),
            hinst: null_mut(),
            dpi: 0,
            dark: false,
            dwm_round_ok: false,
            face: String::new(),
            f_input: null_mut(),
            f_name: null_mut(),
            f_dir: null_mut(),
            f_small: null_mut(),
            f_chip: null_mut(),
            f_badge: null_mut(),
            brush_bg: null_mut(),
            config: None,
            engine: None,
            ctl: None,
            view: None,
            visible: false,
            scroll: 0,
            composing: false,
            suppress_change: false,
            in_menu: false,
            orig_edit_proc: None,
            pending_query: None,
            flush_posted: false,
            hotkey_text: String::new(),
            hotkey_ok: false,
            taskbar_created: 0,
            blur_hidden_at: 0,
        }
    }
}

static mut APP: App = App::new();

/// UI 스레드 전용 전역 상태. 메시지 처리 중 재진입(SetWindowTextW -> EN_CHANGE 등)이 있으므로
/// 호출 쪽은 참조를 오래 쥐지 않고 필요할 때마다 다시 얻는다.
#[allow(static_mut_refs)]
pub fn app() -> &'static mut App {
    unsafe { &mut APP }
}
