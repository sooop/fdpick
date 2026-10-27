//! fdpick Win32 셸: 단일 인스턴스, 설정/엔진/컨트롤러 조립, 메시지 루프.
//! 숨겨진 동안은 GetMessageW 에서 블록되어 CPU 를 쓰지 않는다.
#![windows_subsystem = "windows"]

mod app;
mod autostart;
mod explorer;
mod gfx;
mod hotkey;
mod input;
mod paint;
mod state;
mod tray;
mod util;

use std::mem::zeroed;
use std::ptr::null;
use std::sync::Arc;

use fdpick_core::config;
use fdpick_core::engine::{Engine, Mode, Sink};
use fdpick_core::history;
use fdpick_core::user_actions;
use fdpick_logic::controller::{Controller, ControllerInit};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::state::{app as st, WM_APP_SHOW};
use crate::util::{instance_suffix, w};

/// 이미 실행 중이면 기존 창을 띄우고 true. 뮤텍스 핸들은 프로세스 수명 동안 유지한다.
unsafe fn already_running(class: &str, mutex: &str) -> bool {
    // `--restart`: 이전 인스턴스가 끝나 뮤텍스가 풀릴 때까지 최대 5초 기다린다
    let restarting = std::env::args().any(|a| a == "--restart");
    let mut waits = if restarting { 50 } else { 0 };
    loop {
        let h = CreateMutexW(null(), 0, w(mutex).as_ptr());
        if h.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        if waits == 0 {
            break;
        }
        waits -= 1;
        CloseHandle(h);
        windows_sys::Win32::System::Threading::Sleep(100);
    }
    // 첫 인스턴스가 아직 창을 만드는 중일 수 있어 잠시 재시도한다
    let cls = w(class);
    for _ in 0..20 {
        let hwnd = FindWindowW(cls.as_ptr(), null());
        if !hwnd.is_null() {
            AllowSetForegroundWindow(u32::MAX);
            PostMessageW(hwnd, WM_APP_SHOW, 0, 0);
            return true;
        }
        windows_sys::Win32::System::Threading::Sleep(100);
    }
    true
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        // 테스트 사본(fdpick_test.exe 등)은 이름 접미사로 인스턴스를 분리한다
        let suffix = instance_suffix();
        let class = format!("FdPickWin32{suffix}");
        let mutex = format!("Local\\fdpick-single-instance{suffix}");
        if already_running(&class, &mutex) {
            return;
        }
        let show_now = std::env::args().any(|a| a == "--show");

        // 설정(경고는 시작 알림으로)
        let loaded = config::load_or_create();
        let config = Arc::new(loaded.config.clone());
        let mut notes: Vec<String> = loaded.warnings.clone();
        if let Err(e) = autostart::sync(config.autostart) {
            notes.push(format!("자동 시작 설정 실패: {e}"));
        }

        // 창
        let hinst = GetModuleHandleW(null());
        st().face = gfx::choose_face();
        if let Err(e) = app::create_window(hinst, &class) {
            fatal(&e);
            return;
        }
        let hwnd = st().hwnd;

        // 전역 단축키: 환경변수가 1차 설정보다 우선. 1차·2차 중 하나라도 등록되면 정상으로 본다.
        // 실패해도 앱은 계속 뜨고 트레이로 열 수 있다.
        let spec = std::env::var("FDPICK_HOTKEY").unwrap_or_else(|_| config.hotkey.clone());
        let mut specs = vec![(hotkey::ID_HOTKEY, spec)];
        if !config.hotkey2.trim().is_empty() && !config.hotkey2.trim().eq_ignore_ascii_case(specs[0].1.trim()) {
            specs.push((hotkey::ID_HOTKEY2, config.hotkey2.clone()));
        }
        let mut active = Vec::new();
        let mut failed = Vec::new();
        for (id, spec) in &specs {
            match hotkey::register(hwnd, *id, spec) {
                Ok(()) => active.push(spec.clone()),
                Err(e) => {
                    failed.push(spec.clone());
                    notes.push(e);
                }
            }
        }
        st().hotkey_ok = !active.is_empty();
        st().hotkey_text = if active.is_empty() { failed.join(", ") } else { active.join(" / ") };
        tray::add();

        // 엔진: sink 는 엔진 스레드에서 불리므로 결과를 UI 스레드로 보낸다
        let hwnd_id = hwnd as usize;
        let sink: Sink = Arc::new(move |p| app::post_results(hwnd_id, p));
        let engine = Arc::new(Engine::new(sink, &config));

        // 컨트롤러
        let cfg_for_applicable = config.clone();
        let history_path = history::default_path();
        let ctl = Controller::new(ControllerInit {
            start_mode: Some(Mode::Fuzzy),
            history: history_path
                .as_deref()
                .map(|p| history::load(p, config.history_limit))
                .unwrap_or_default(),
            history_limit: config.history_limit,
            all_actions: user_actions::all(&config.actions),
            applicable: Arc::new(move |paths: &[String]| user_actions::applicable(&cfg_for_applicable.actions, paths)),
            startup_notes: notes,
            enter_action: config.enter_action.clone(),
            shift_enter_action: config.shift_enter_action.clone(),
            home_dir: std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()),
        });
        app::install(config, engine, ctl);

        if show_now {
            app::show_popup(Mode::Fuzzy);
        }

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 창을 만들 수 없을 때의 최소 알림
unsafe fn fatal(msg: &str) {
    MessageBoxW(std::ptr::null_mut(), w(msg).as_ptr(), w("fdpick").as_ptr(), MB_OK | MB_ICONERROR);
}
