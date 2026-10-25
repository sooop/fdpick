#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(dead_code)]
mod actions;
#[allow(dead_code)]
mod config;
mod engine;
// fd.rs 와 pathnav.rs 에는 아직 쓰지 않는 공개 함수(테스트용 포함)가 있다
#[allow(dead_code)]
mod fd;
#[allow(dead_code)]
mod matcher;
#[allow(dead_code)]
mod recents;
#[allow(dead_code)]
mod pathnav;
mod query;
mod user_actions;

use std::time::Instant;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

// 기본 크기의 80%
const POPUP_WIDTH: f64 = 608.0;
const POPUP_HEIGHT: f64 = 416.0;
// 모니터 높이 대비 팝업 상단 위치
const TOP_RATIO: f64 = 0.15;

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

/// 커서가 있는 모니터의 중앙 상단에 배치한다. DPI 배율은 그 모니터 기준.
fn place_window(app: &AppHandle, win: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };
    let scale = m.scale_factor();
    let w = (POPUP_WIDTH * scale).round();
    let h = (POPUP_HEIGHT * scale).round();
    let mp = m.position();
    let ms = m.size();
    let x = mp.x as f64 + (ms.width as f64 - w) / 2.0;
    let y = mp.y as f64 + ms.height as f64 * TOP_RATIO;
    let _ = win.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

fn show_popup(app: &AppHandle) {
    let t0 = Instant::now();
    let Some(win) = main_window(app) else { return };
    place_window(app, &win);
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("popup-shown", ());
    #[cfg(debug_assertions)]
    eprintln!("show_popup: {:?}", t0.elapsed());
    let _ = t0;
}

fn hide_popup_inner(app: &AppHandle) {
    if let Some(win) = main_window(app) {
        let _ = win.hide();
    }
}

fn toggle_popup(app: &AppHandle) {
    let visible = main_window(app)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    #[cfg(debug_assertions)]
    eprintln!("toggle_popup: visible={visible}");
    if visible {
        hide_popup_inner(app);
    } else {
        show_popup(app);
    }
}

#[tauri::command]
fn hide_popup(app: AppHandle) {
    hide_popup_inner(&app);
}

// 동기 명령은 메인 스레드에서 실행되므로, fd 취소·파일 시스템 접근이 UI 를 막지 않게 async 로 둔다
#[tauri::command(async)]
fn query_changed(engine: tauri::State<engine::Engine>, text: String, mode: String, force: Option<bool>) {
    engine.query_changed(&text, &mode, force.unwrap_or(false));
}

#[tauri::command(async)]
fn do_action(
    config: tauri::State<config::Config>,
    engine: tauri::State<engine::Engine>,
    kind: String,
    paths: Vec<String>,
) -> Result<(), String> {
    let opts = actions::CopyOptions {
        quote: config.copy.quote,
        forward_slash: config.copy.forward_slash,
    };
    actions::run_with(&kind, &paths, &opts)?;
    // 성공한 동작의 대상만 최근 목록에 올린다
    let norm: Vec<String> = paths
        .iter()
        .map(|p| actions::normalize_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    let recorded: Vec<String> = match kind.as_str() {
        "copy_dir" => norm.iter().map(|p| actions::to_dir_path(p)).collect(),
        "reveal" | "open" => norm.into_iter().take(1).collect(),
        _ => norm,
    };
    engine.record_recent(&recorded);
    Ok(())
}

#[tauri::command]
fn list_actions(config: tauri::State<config::Config>) -> Vec<user_actions::ActionInfo> {
    user_actions::all(&config.actions)
}

#[tauri::command(async)]
fn applicable_actions(config: tauri::State<config::Config>, paths: Vec<String>) -> Vec<user_actions::ActionInfo> {
    user_actions::applicable(&config.actions, &paths)
}

#[tauri::command(async)]
fn run_user_action(
    config: tauri::State<config::Config>,
    engine: tauri::State<engine::Engine>,
    id: usize,
    paths: Vec<String>,
) -> Result<(), String> {
    user_actions::run(&config.actions, id, &paths)?;
    let norm: Vec<String> = paths
        .iter()
        .map(|p| actions::normalize_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    engine.record_recent(&norm);
    Ok(())
}

/// 시작 시 알릴 메시지(설정 파일 경고, 단축키 등록 실패 등). 프런트가 상태줄에 보여준다.
struct StartupNotes(Vec<String>);

#[tauri::command]
fn startup_notes(notes: tauri::State<StartupNotes>) -> Vec<String> {
    notes.0.clone()
}

/// 로그인 시 자동 시작을 HKCU Run 키에 맞춘다. 설정과 같으면 아무것도 바꾸지 않는다.
/// 개발 빌드(디버그)에서는 임시 경로가 등록되지 않도록 건너뛴다.
fn sync_autostart(enabled: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let reg = |args: &[&str]| {
        Command::new("reg.exe")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("reg.exe 실행 실패: {e}"))
    };
    let exists = reg(&["query", RUN_KEY, "/v", "fdpick"])?.status.success();
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let value = format!("\"{}\"", exe.display());
        let out = reg(&["add", RUN_KEY, "/v", "fdpick", "/t", "REG_SZ", "/d", &value, "/f"])?;
        if !out.status.success() {
            return Err("자동 시작 등록에 실패했습니다".into());
        }
    } else if exists {
        reg(&["delete", RUN_KEY, "/v", "fdpick", "/f"])?;
    }
    Ok(())
}

fn main() {
    let loaded = config::load_or_create();
    let config = loaded.config.clone();
    let mut notes: Vec<String> = loaded.warnings.clone();
    for w in &loaded.warnings {
        eprintln!("설정 경고: {w}");
    }
    if let Err(e) = sync_autostart(config.autostart) {
        eprintln!("자동 시작 설정 실패: {e}");
        notes.push(format!("자동 시작 설정 실패: {e}"));
    }

    tauri::Builder::default()
        // 두 번째 실행은 첫 번째 인스턴스의 팝업을 띄우고 종료된다
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_popup(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            hide_popup,
            query_changed,
            do_action,
            startup_notes,
            list_actions,
            applicable_actions,
            run_user_action
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            app.manage(engine::Engine::new(handle.clone(), &config));
            app.manage(config.clone());

            // 전역 단축키: 누름 시점에만 토글. 등록에 실패해도(다른 앱과 충돌) 앱은 계속 뜨고
            // 트레이로 열 수 있다. 환경변수 FDPICK_HOTKEY 가 있으면 설정 파일보다 우선한다.
            let hotkey = std::env::var("FDPICK_HOTKEY").unwrap_or_else(|_| config.hotkey.clone());
            let hotkey_error = app
                .global_shortcut()
                .on_shortcut(hotkey.as_str(), |app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_popup(app);
                    }
                })
                .err();
            if let Some(e) = &hotkey_error {
                eprintln!("단축키 {hotkey} 등록 실패(다른 앱이 사용 중일 수 있음): {e}");
                notes.push(format!("단축키 {hotkey} 등록 실패(다른 앱이 사용 중일 수 있음)"));
            }
            app.manage(StartupNotes(std::mem::take(&mut notes)));
            let tooltip = match &hotkey_error {
                None => format!("fdpick ({hotkey})"),
                Some(_) => format!("fdpick (단축키 {hotkey} 등록 실패 - 트레이로 열기)"),
            };

            // 트레이: 좌클릭 = 열기, 메뉴 = 열기/종료
            let open = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip(tooltip)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_popup(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_popup(tray.app_handle());
                    }
                });
            if let Some(icon) = handle.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(debug_assertions)]
            if let WindowEvent::Focused(f) = event {
                eprintln!("focused={f}");
            }
            if let WindowEvent::Focused(false) = event {
                let hide = window.app_handle().state::<config::Config>().hide_on_blur;
                if hide && window.label() == "main" {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("fdpick 실행 실패");
}
