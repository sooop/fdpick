// fdpick Slint 프로토타입: 무테/투명/항상 위 팝업, 전역 단축키 토글, --hide-after 자동 숨김.
#![windows_subsystem = "windows"]

use std::time::Duration;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use slint::winit_030::{winit, WinitWindowAccessor};
use slint::{ComponentHandle, LogicalPosition, ModelRc, PhysicalPosition, SharedString, VecModel};

slint::include_modules!();

// user32 직접 선언(별도 크레이트 없이 주 모니터 크기와 시스템 DPI 만 필요)
#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDpiForSystem() -> u32;
}

const LOGICAL_W: f32 = 608.0;

// 가로 중앙, 세로 상단 15% 로 이동한다(주 모니터 기준, 물리 픽셀).
fn place(win: &MainWindow) {
    let (sw, sh, dpi) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1), GetDpiForSystem()) };
    let sf = dpi as f32 / 96.0;
    let x = ((sw as f32 - LOGICAL_W * sf) / 2.0).round() as i32;
    let y = (sh as f32 * 0.15).round() as i32;
    win.window().set_position(PhysicalPosition::new(x, y));
}

fn show_popup(win: &MainWindow) {
    place(win);
    let _ = win.show();
    // 다른 앱이 전면일 때 포커스를 가져온다.
    win.window().with_winit_window(|w: &winit::window::Window| w.focus_window());
    win.invoke_focus_text();
}

// 가짜 경로 생성(한글+영문 혼합)
fn make_rows(n: usize) -> Vec<Row> {
    let dirs = [
        r"D:\projects\보고서\2026",
        r"C:\Users\yepben\Documents\계약서",
        r"D:\projects\fdpick\src-tauri\src",
        r"E:\백업\사진\2025\여행",
        r"C:\Windows\System32\drivers\etc",
        r"D:\work\tracx\logistics\export\invoices",
    ];
    let names = [
        "report_final_v2.xlsx",
        "견적서_최종.docx",
        "main.rs",
        "발주내역_2026_09.csv",
        "README.md",
        "package-lock.json",
        "회의록(수정본).pdf",
        "IMG_2041.jpg",
    ];
    (0..n)
        .map(|i| Row {
            name: SharedString::from(format!("{}_{:04}", names[i % names.len()], i)),
            dir: SharedString::from(dirs[(i / 3) % dirs.len()]),
        })
        .collect()
}

// 글꼴 검사: 시스템 컬렉션(DirectWrite)에서 Jetendard 를 찾는지 확인, 못 찾으면 파일로 등록
fn ensure_font() -> &'static str {
    use slint::fontique_011::{fontique, shared_collection};
    let mut col = shared_collection();
    if col.family_id("Jetendard").is_some() {
        return "system";
    }
    let base = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut ok = false;
    for f in ["Jetendard-Regular.ttf", "Jetendard-Bold.ttf"] {
        let p = format!(r"{}\Microsoft\Windows\Fonts\{}", base, f);
        if let Ok(data) = std::fs::read(p) {
            let blob = fontique::Blob::new(std::sync::Arc::new(data));
            ok |= !col.register_fonts(blob, None).is_empty();
        }
    }
    if ok { "registered" } else { "missing" }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let arg_val = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let hide_after: Option<f64> = arg_val("--hide-after").and_then(|v| v.parse().ok());
    let row_count: usize = arg_val("--rows").and_then(|v| v.parse().ok()).unwrap_or(100);
    // 숨김 시 윈도(와 렌더러 자원)를 파괴하는 변형 시험용
    if args.iter().any(|a| a == "--destroy-on-hide") {
        std::env::set_var("SLINT_DESTROY_WINDOW_ON_HIDE", "1");
    }

    // 렌더러 선택(빌드 feature 로 하나만 포함)
    #[cfg(feature = "sw")]
    let renderer = "software";
    #[cfg(all(feature = "gl", not(feature = "sw")))]
    let renderer = "femtovg";

    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .with_winit_window_attributes_hook(|attrs| {
            use winit::platform::windows::WindowAttributesExtWindows;
            // 작업표시줄 숨김
            attrs.with_skip_taskbar(true)
        })
        .select()?;

    let font_state = ensure_font();
    if std::env::var_os("PROTO_LOG").is_some() {
        eprintln!("renderer={renderer} font={font_state}");
    }

    let win = MainWindow::new()?;
    let model = ModelRc::new(VecModel::from(make_rows(row_count)));
    win.set_rows(model);
    win.set_status(SharedString::from(format!(
        "{} 건  |  ↑↓ 이동  Esc 닫기  Ctrl+Alt+F 토글  [{} / 글꼴 {}]",
        row_count, renderer, font_state
    )));

    {
        let weak = win.as_weak();
        win.on_hide_requested(move || {
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
    }

    // 전역 단축키 Ctrl+Alt+F (메인 스레드에서 등록: 윈도 메시지는 winit 루프가 처리)
    let manager = GlobalHotKeyManager::new()?;
    // 기본 F. 다른 프로토타입과 충돌하면 --hotkey G 처럼 글자를 바꿔 시험한다.
    let key_name = arg_val("--hotkey").unwrap_or_else(|| "F".into()).to_uppercase();
    let code: Code = format!("Key{key_name}").parse().unwrap_or(Code::KeyF);
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), code);
    if let Err(e) = manager.register(hotkey) {
        eprintln!("hotkey register failed: {e}");
    }
    {
        let weak = win.as_weak();
        GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
            if ev.state != HotKeyState::Pressed {
                return;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                if w.window().is_visible() {
                    let _ = w.hide();
                } else {
                    show_popup(&w);
                }
            });
        }));
    }

    show_popup(&win);

    if let Some(secs) = hide_after {
        let weak = win.as_weak();
        slint::Timer::single_shot(Duration::from_secs_f64(secs), move || {
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
    }

    // 측정 자동화용: 지정 시간 뒤 프로세스 스스로 종료
    if let Some(secs) = arg_val("--exit-after").and_then(|v| v.parse::<f64>().ok()) {
        slint::Timer::single_shot(Duration::from_secs_f64(secs), || {
            let _ = slint::quit_event_loop();
        });
    }

    // 창이 숨겨져도 종료하지 않는 이벤트 루프
    slint::run_event_loop_until_quit()?;
    drop(manager);
    let _ = LogicalPosition::new(0.0, 0.0);
    Ok(())
}
