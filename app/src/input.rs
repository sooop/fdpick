//! 입력창(EDIT) 서브클래스: 키를 `KeyEvent` 로 번역해 컨트롤러에 넘기고, IME 조합 상태를 추적한다.

use std::mem::zeroed;

use fdpick_logic::view::KeyEvent;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::UI::Controls::{EM_GETSEL, EM_SETSEL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::app;
use crate::paint::draw_placeholder;
use crate::state::app as st;

/// 가상 키 -> `KeyEvent.key` 이름. 수식키 단독/IME 처리 키는 None.
fn vk_name(vk: u16) -> Option<String> {
    let s = match vk {
        VK_RETURN => "Enter",
        VK_TAB => "Tab",
        VK_ESCAPE => "Escape",
        VK_UP => "ArrowUp",
        VK_DOWN => "ArrowDown",
        VK_LEFT => "ArrowLeft",
        VK_RIGHT => "ArrowRight",
        VK_PRIOR => "PageUp",
        VK_NEXT => "PageDown",
        VK_HOME => "Home",
        VK_END => "End",
        VK_DELETE => "Delete",
        VK_INSERT => "Insert",
        VK_BACK => "Backspace",
        VK_SPACE => "Space",
        VK_OEM_2 => "/",
        VK_OEM_5 => "\\",
        VK_OEM_1 => ";",
        VK_OEM_PLUS => "=",
        VK_OEM_COMMA => ",",
        VK_OEM_MINUS => "-",
        VK_OEM_PERIOD => ".",
        VK_OEM_3 => "`",
        VK_OEM_4 => "[",
        VK_OEM_6 => "]",
        VK_OEM_7 => "'",
        0x30..=0x39 | 0x41..=0x5A => return Some((vk as u8 as char).to_string()),
        VK_NUMPAD0..=VK_NUMPAD9 => return Some(((b'0' + (vk - VK_NUMPAD0) as u8) as char).to_string()),
        VK_DIVIDE => "/",
        VK_F1..=VK_F24 => return Some(format!("F{}", vk - VK_F1 + 1)),
        _ => return None,
    };
    Some(s.to_string())
}

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

/// 입력창 텍스트
pub fn edit_text(edit: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(edit);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(edit, buf.as_mut_ptr(), len + 1);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// 캐럿이 텍스트 끝이고 선택 영역이 없는가
fn caret_at_end(edit: HWND) -> bool {
    unsafe {
        let (mut s, mut e) = (0u32, 0u32);
        SendMessageW(edit, EM_GETSEL, &mut s as *mut u32 as WPARAM, &mut e as *mut u32 as LPARAM);
        let len = GetWindowTextLengthW(edit).max(0) as u32;
        s == e && e == len
    }
}

/// 이미 큐에 들어온 WM_CHAR 류를 지운다(처리된 키가 글자로 입력되거나 경고음이 나지 않게).
unsafe fn drop_pending_chars(edit: HWND) {
    let mut m: MSG = zeroed();
    while PeekMessageW(&mut m, edit, WM_CHAR, WM_CHAR, PM_REMOVE) != 0 {}
    while PeekMessageW(&mut m, edit, WM_SYSCHAR, WM_SYSCHAR, PM_REMOVE) != 0 {}
    while PeekMessageW(&mut m, edit, WM_DEADCHAR, WM_DEADCHAR, PM_REMOVE) != 0 {}
}

/// WM_KEYDOWN / WM_SYSKEYDOWN. 처리했으면 Some(0).
unsafe fn on_keydown(edit: HWND, wp: WPARAM) -> Option<LRESULT> {
    let key = vk_name(wp as u16)?;
    let ev = KeyEvent {
        key,
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        caret_at_end: caret_at_end(edit),
        composing: st().composing,
    };
    let text = edit_text(edit);
    if app::on_key(&ev, &text) {
        drop_pending_chars(edit);
        return Some(0);
    }
    // 단일행 EDIT 은 Ctrl+A 를 처리하지 않는다
    if ev.ctrl && !ev.alt && !ev.shift && ev.key == "A" {
        SendMessageW(edit, EM_SETSEL, 0, -1);
        drop_pending_chars(edit);
        return Some(0);
    }
    None
}

pub unsafe extern "system" fn edit_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let orig = st().orig_edit_proc;
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if let Some(r) = on_keydown(h, wp) {
                return r;
            }
        }
        // Tab/Enter/Esc 의 WM_CHAR 는 EDIT 가 경고음을 내므로 항상 삼킨다
        WM_CHAR if wp == 9 || wp == 13 || wp == 27 => return 0,
        WM_IME_STARTCOMPOSITION => {
            st().composing = true;
        }
        WM_IME_ENDCOMPOSITION => {
            let r = CallWindowProcW(orig, h, msg, wp, lp);
            st().composing = false;
            // 확정 문자열의 EN_CHANGE 는 조합 중 상태로 도착하므로 조합이 끝난 뒤 한 번 더 알린다
            app::on_edit_changed();
            return r;
        }
        WM_MOUSEWHEEL => {
            // 포커스가 입력창에 있어도 휠은 목록을 스크롤한다
            return SendMessageW(GetParent(h), msg, wp, lp);
        }
        WM_ENTERMENULOOP => st().in_menu = true,
        WM_EXITMENULOOP => st().in_menu = false,
        WM_PAINT => {
            let r = CallWindowProcW(orig, h, msg, wp, lp);
            if GetWindowTextLengthW(h) == 0 {
                draw_placeholder(h);
            }
            return r;
        }
        _ => {}
    }
    CallWindowProcW(orig, h, msg, wp, lp)
}
