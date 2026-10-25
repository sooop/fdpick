//! 로그인 시 자동 시작: HKCU\Software\Microsoft\Windows\CurrentVersion\Run 의 "fdpick" 값을
//! 설정(`config.autostart`)에 맞춘다. 이미 같으면 아무것도 쓰지 않는다.

use std::ptr::null_mut;

use windows_sys::Win32::System::Registry::*;

use crate::util::{exe_stem, w};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "fdpick";

/// 현재 등록된 값(없으면 None)
fn read_value() -> Option<String> {
    unsafe {
        let key = w(RUN_KEY);
        let name = w(VALUE_NAME);
        let mut size: u32 = 0;
        let r = RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size);
        if r != 0 || size < 2 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size2 = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size2,
        );
        if r != 0 {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

/// 설정에 맞춰 Run 키를 동기화한다. 디버그 빌드와 테스트 사본(exe 이름이 fdpick 이 아님)은 건너뛴다.
pub fn sync(enabled: bool) -> Result<(), String> {
    if cfg!(debug_assertions) || !exe_stem().eq_ignore_ascii_case("fdpick") {
        return Ok(());
    }
    let current = read_value();
    unsafe {
        let key = w(RUN_KEY);
        let name = w(VALUE_NAME);
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let value = format!("\"{}\"", exe.display());
            if current.as_deref() == Some(value.as_str()) {
                return Ok(());
            }
            let data = w(&value);
            let r = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr() as *const _,
                (data.len() * 2) as u32,
            );
            if r != 0 {
                return Err("자동 시작 등록에 실패했습니다".into());
            }
        } else if current.is_some() {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            if r != 0 {
                return Err("자동 시작 해제에 실패했습니다".into());
            }
        }
    }
    Ok(())
}
