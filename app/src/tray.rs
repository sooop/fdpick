//! 트레이 아이콘(Shell_NotifyIconW)과 우클릭 메뉴.

use std::mem::{size_of, zeroed};
use std::ptr::null;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::state::{app, WM_APP_TRAY};
use crate::util::w;

const TRAY_ID: u32 = 1;
pub const ID_MENU_OPEN: usize = 1;
pub const ID_MENU_QUIT: usize = 2;
pub const ID_MENU_RESTART: usize = 3;

/// 아이콘 리소스 ID(app.rc 의 `1 ICON`)
pub const ICON_RES_ID: u16 = 1;

fn data(tooltip: &str) -> NOTIFYICONDATAW {
    let a = app();
    let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = a.hwnd;
    nid.uID = TRAY_ID;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    nid.hIcon = unsafe { LoadIconW(a.hinst, ICON_RES_ID as usize as *const u16) };
    let t = w(tooltip);
    let n = t.len().min(nid.szTip.len() - 1);
    nid.szTip[..n].copy_from_slice(&t[..n]);
    nid
}

/// 트레이 아이콘 등록(탐색기 재시작 후 재등록에도 쓴다)
pub fn add() {
    let a = app();
    let tip = if a.hotkey_ok {
        format!("fdpick ({})", a.hotkey_text)
    } else {
        format!("fdpick (단축키 {} 등록 실패 - 트레이로 열기)", a.hotkey_text)
    };
    let nid = data(&tip);
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub fn remove() {
    let mut nid: NOTIFYICONDATAW = unsafe { zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = app().hwnd;
    nid.uID = TRAY_ID;
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// 우클릭 메뉴: 열기 / 종료. 선택한 항목 ID(없으면 0)
pub fn show_menu() -> usize {
    let a = app();
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        AppendMenuW(menu, MF_STRING, ID_MENU_OPEN, w("열기").as_ptr());
        AppendMenuW(menu, MF_STRING, ID_MENU_RESTART, w("다시 시작 (설정 다시 읽기)").as_ptr());
        AppendMenuW(menu, MF_STRING, ID_MENU_QUIT, w("종료").as_ptr());
        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        // 관례: 메뉴가 바깥 클릭에 닫히려면 소유 창을 전경으로 만들어야 한다
        a.in_menu = true;
        SetForegroundWindow(a.hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, a.hwnd, null());
        PostMessageW(a.hwnd, WM_NULL, 0, 0);
        a.in_menu = false;
        DestroyMenu(menu);
        cmd as usize
    }
}
