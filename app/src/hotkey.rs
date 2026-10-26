//! 전역 단축키 문자열 파싱("Ctrl+Alt+Space")과 등록.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

pub const ID_HOTKEY: i32 = 1;
pub const ID_HOTKEY2: i32 = 2;

/// 파싱 결과: (HOT_KEY_MODIFIERS 비트, 가상 키 코드)
pub fn parse(spec: &str) -> Result<(u32, u32), String> {
    let mut mods = 0u32;
    let mut vk: Option<u32> = None;
    for raw in spec.split('+') {
        let t = raw.trim();
        if t.is_empty() {
            return Err(format!("단축키 \"{spec}\" 형식이 잘못되었습니다"));
        }
        match t.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" | "meta" | "cmd" => mods |= MOD_WIN,
            _ => {
                if vk.is_some() {
                    return Err(format!("단축키 \"{spec}\" 에 일반 키가 둘 이상입니다"));
                }
                vk = Some(key_code(t).ok_or_else(|| format!("알 수 없는 단축키 \"{t}\""))?);
            }
        }
    }
    match vk {
        Some(k) => Ok((mods, k)),
        None => Err(format!("단축키 \"{spec}\" 에 일반 키가 없습니다")),
    }
}

fn key_code(t: &str) -> Option<u32> {
    let lower = t.to_ascii_lowercase();
    let named = match lower.as_str() {
        "space" => Some(VK_SPACE),
        "enter" | "return" => Some(VK_RETURN),
        "tab" => Some(VK_TAB),
        "esc" | "escape" => Some(VK_ESCAPE),
        "backspace" => Some(VK_BACK),
        "delete" | "del" => Some(VK_DELETE),
        "insert" | "ins" => Some(VK_INSERT),
        "home" => Some(VK_HOME),
        "end" => Some(VK_END),
        "pageup" => Some(VK_PRIOR),
        "pagedown" => Some(VK_NEXT),
        "up" | "arrowup" => Some(VK_UP),
        "down" | "arrowdown" => Some(VK_DOWN),
        "left" | "arrowleft" => Some(VK_LEFT),
        "right" | "arrowright" => Some(VK_RIGHT),
        _ => None,
    };
    if let Some(v) = named {
        return Some(v as u32);
    }
    // F1..F24
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(VK_F1 as u32 + n - 1);
        }
    }
    let mut it = t.chars();
    let c = it.next()?;
    if it.next().is_some() {
        return None;
    }
    if c.is_ascii_alphanumeric() {
        return Some(c.to_ascii_uppercase() as u32);
    }
    // 기호 키는 현재 자판 배열 기준으로 찾는다
    let r = unsafe { VkKeyScanW(c as u16) };
    if r == -1 {
        None
    } else {
        Some((r as u16 & 0xff) as u32)
    }
}

/// 등록한다. 실패하면 사유 문자열을 돌려준다.
pub fn register(hwnd: HWND, id: i32, spec: &str) -> Result<(), String> {
    let (mods, vk) = parse(spec)?;
    let ok = unsafe { RegisterHotKey(hwnd, id, mods | MOD_NOREPEAT, vk) };
    if ok == 0 {
        Err(format!("단축키 {spec} 등록 실패(다른 앱이 사용 중일 수 있음)"))
    } else {
        Ok(())
    }
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        UnregisterHotKey(hwnd, ID_HOTKEY);
        UnregisterHotKey(hwnd, ID_HOTKEY2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse("Ctrl+Alt+Space"), Ok((MOD_CONTROL | MOD_ALT, VK_SPACE as u32)));
        assert_eq!(parse("ctrl + alt + f"), Ok((MOD_CONTROL | MOD_ALT, 'F' as u32)));
        assert_eq!(parse("Win+Shift+F12"), Ok((MOD_WIN | MOD_SHIFT, VK_F12 as u32)));
        assert!(parse("Ctrl+Alt").is_err());
        assert!(parse("Ctrl+A+B").is_err());
        assert!(parse("").is_err());
    }
}
