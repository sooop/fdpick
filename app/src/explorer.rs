//! 열려 있는 탐색기 창으로 경로를 보낸다(Shift+Enter).
//!
//! 팝업을 띄우기 직전에 포커스가 있던 창이 탐색기이면 그 창(탭이면 활성 탭)을 `Navigate2` 로 이동한다.
//! 파일이면 부모 폴더로 이동한 뒤 그 파일을 선택한다. 탐색기 창이 하나도 없으면 새 창을 연다.
//! 다른 앱에 포커스가 있었고 탐색기 창이 따로 열려 있으면 아무 일도 하지 않는다.

use std::path::Path;
use std::time::{Duration, Instant};

use fdpick_core::actions;
use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IServiceProvider, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    ILCreateFromPathW, ILFindLastID, ILFree, IShellBrowser, IShellWindows, IWebBrowser2, PathCreateFromUrlW,
    ShellWindows, SVSI_DESELECTOTHERS, SVSI_ENSUREVISIBLE, SVSI_FOCUSED, SVSI_SELECT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, IsWindowVisible, SetForegroundWindow};

/// SID_STopLevelBrowser
const SID_STOP_LEVEL_BROWSER: GUID = GUID::from_u128(0x4c96be40_915c_11cf_99d3_00aa004ae837);

struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        // 이미 다른 모드로 초기화된 스레드면 실패하지만 그대로 쓸 수 있다
        Self(unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct Win {
    browser: IWebBrowser2,
    top: HWND,
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// 탐색기 창(탭 포함) 목록. IE·바탕화면 등은 제외한다.
unsafe fn explorer_windows(sw: &IShellWindows) -> Vec<Win> {
    let mut out = Vec::new();
    let n = sw.Count().unwrap_or(0);
    for i in 0..n {
        let Ok(disp) = sw.Item(&VARIANT::from(i)) else { continue };
        let Ok(browser) = disp.cast::<IWebBrowser2>() else { continue };
        let Ok(top) = browser.HWND() else { continue };
        let top = HWND(top.0 as *mut _);
        if class_of(top) == "CabinetWClass" {
            out.push(Win { browser, top });
        }
    }
    out
}

/// 같은 최상위 창의 탭이 여럿이면 화면에 보이는(활성) 탭을 고른다.
unsafe fn active_tab<'a>(cands: &[&'a Win]) -> Option<&'a Win> {
    for w in cands {
        let visible = w
            .browser
            .cast::<IServiceProvider>()
            .and_then(|sp| sp.QueryService::<IShellBrowser>(&SID_STOP_LEVEL_BROWSER))
            .and_then(|sb| sb.GetWindow())
            .map(|h| IsWindowVisible(h).as_bool())
            .unwrap_or(false);
        if visible {
            return Some(*w);
        }
    }
    cands.first().copied()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn same_path(a: &str, b: &str) -> bool {
    actions::normalize_path(a).eq_ignore_ascii_case(&actions::normalize_path(b))
}

/// 브라우저의 현재 위치를 파일 시스템 경로로 읽는다(가상 폴더면 None).
unsafe fn location_path(wb: &IWebBrowser2) -> Option<String> {
    let url = wb.LocationURL().ok()?.to_string();
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    PathCreateFromUrlW(PCWSTR(wide(&url).as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len, 0).ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

unsafe fn navigate(wb: &IWebBrowser2, dir: &str) -> Result<(), String> {
    let url = VARIANT::from(dir);
    let none = VARIANT::default();
    wb.Navigate2(&url, Some(&none), Some(&none), Some(&none), Some(&none)).map_err(|e| format!("탐색기 이동 실패: {e}"))
}

/// 이동이 끝날 때까지 기다렸다가 파일을 선택한다. 시간 안에 끝나지 않으면 이동만 하고 끝낸다.
unsafe fn select_file(wb: &IWebBrowser2, dir: &str, file: &str) {
    let full = wide(file);
    let pidl = ILCreateFromPathW(PCWSTR(full.as_ptr()));
    if pidl.is_null() {
        return;
    }
    let child: *const ITEMIDLIST = ILFindLastID(pidl);
    let deadline = Instant::now() + Duration::from_millis(3000);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        let ready = !wb.Busy().map(|b| b.as_bool()).unwrap_or(true)
            && location_path(wb).map(|p| same_path(&p, dir)).unwrap_or(false);
        if !ready {
            continue;
        }
        let view = wb
            .cast::<IServiceProvider>()
            .and_then(|sp| sp.QueryService::<IShellBrowser>(&SID_STOP_LEVEL_BROWSER))
            .and_then(|sb| sb.QueryActiveShellView());
        if let Ok(view) = view {
            let flags = SVSI_DESELECTOTHERS.0 | SVSI_SELECT.0 | SVSI_ENSUREVISIBLE.0 | SVSI_FOCUSED.0;
            if view.SelectItem(child, flags as u32).is_ok() {
                break;
            }
        }
    }
    ILFree(Some(pidl));
}

/// `prev_fg`(팝업 직전 포커스 창)가 탐색기이면 그 창을 `path` 로 이동한다.
/// 동작했으면 true, 조용히 무시했으면 false.
pub fn goto(prev_fg: usize, path: &str) -> Result<bool, String> {
    let norm = actions::normalize_path(path);
    let p = Path::new(&norm);
    if !p.exists() {
        return Err(format!("경로가 존재하지 않습니다: {norm}"));
    }
    unsafe {
        let _com = ComGuard::init();
        let sw: IShellWindows =
            CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).map_err(|e| format!("탐색기 창 목록을 읽지 못했습니다: {e}"))?;
        let wins = explorer_windows(&sw);
        if wins.is_empty() {
            actions::run("reveal", &[norm])?;
            return Ok(true);
        }
        let fg = HWND(prev_fg as *mut _);
        let cands: Vec<&Win> = wins.iter().filter(|w| w.top == fg).collect();
        let Some(win) = active_tab(&cands) else { return Ok(false) };

        if p.is_dir() {
            navigate(&win.browser, &norm)?;
        } else {
            let dir = actions::to_dir_path(&norm);
            navigate(&win.browser, &dir)?;
            select_file(&win.browser, &dir, &norm);
        }
        let _ = SetForegroundWindow(win.top);
        Ok(true)
    }
}
