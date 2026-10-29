//! 팔레트, DPI 배율, 글꼴/브러시 수명 관리.

use std::mem::zeroed;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::COLORREF;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::*;

use crate::state::app;
use crate::util::{hex, w};

/// 이전 Tauri 구현 style.css 의 라이트/다크 값
#[derive(Clone, Copy)]
pub struct Palette {
    pub panel_bg: COLORREF,
    pub border: COLORREF,
    pub text: COLORREF,
    pub muted: COLORREF,
    pub accent: COLORREF,
    pub divider: COLORREF,
    pub chip_bg: COLORREF,
    pub chip_text: COLORREF,
    pub row_sel: COLORREF,
    pub mark: COLORREF,
    pub invalid: COLORREF,
    pub err: COLORREF,
    /// 최근 모드 칩 (배경, 글자)
    pub recent_chip: (COLORREF, COLORREF),
    /// 앱 모드 칩 (배경, 글자)
    pub apps_chip: (COLORREF, COLORREF),
}

const LIGHT: Palette = Palette {
    panel_bg: hex(0xffffff),
    border: hex(0xd0d0d4),
    text: hex(0x1c1c1e),
    muted: hex(0x8a8a8e),
    accent: hex(0x3478f6),
    divider: hex(0xe6e6ea),
    chip_bg: hex(0xeef3ff),
    chip_text: hex(0x2b5fd0),
    row_sel: hex(0xe8f0ff),
    mark: hex(0xd9480f),
    invalid: hex(0xd92d20),
    err: hex(0xd93025),
    recent_chip: (hex(0xb45309), hex(0xffffff)),
    apps_chip: (hex(0x2f855a), hex(0xffffff)),
};

const DARK: Palette = Palette {
    panel_bg: hex(0x2b2b2f),
    border: hex(0x4a4a50),
    text: hex(0xf2f2f4),
    muted: hex(0x8e8e93),
    accent: hex(0x5a9bff),
    divider: hex(0x3a3a40),
    chip_bg: hex(0x34405a),
    chip_text: hex(0x9bbcff),
    row_sel: hex(0x38415a),
    mark: hex(0xff9f6b),
    invalid: hex(0xff6b5e),
    err: hex(0xff7b72),
    recent_chip: (hex(0xf0b35a), hex(0x1c1c1e)),
    apps_chip: (hex(0x6fcf97), hex(0x1c1c1e)),
};

pub fn palette() -> &'static Palette {
    if app().dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// 논리 픽셀(96dpi 기준, 소수 허용) -> 물리 픽셀
pub fn sc(x: f32) -> i32 {
    (x * app().dpi.max(96) as f32 / 96.0 + 0.5) as i32
}

/// 설정(system/light/dark)에 따른 다크 여부
fn want_dark() -> bool {
    let pref = app().config.as_ref().map(|c| c.theme.as_str()).unwrap_or("system");
    match pref {
        "light" => false,
        "dark" => true,
        _ => system_dark(),
    }
}

/// 레지스트리 AppsUseLightTheme. 읽기 실패 시 라이트로 간주
fn system_dark() -> bool {
    let mut data: u32 = 1;
    let mut sz: u32 = 4;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            w("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut sz,
        )
    };
    r == 0 && data == 0
}

/// 테마를 다시 읽는다. 바뀌었으면 true(배경 브러시도 새로 만든다).
pub fn refresh_theme() -> bool {
    let dark = want_dark();
    let a = app();
    if !a.brush_bg.is_null() && a.dark == dark {
        return false;
    }
    a.dark = dark;
    unsafe {
        if !a.brush_bg.is_null() {
            DeleteObject(a.brush_bg);
        }
        a.brush_bg = CreateSolidBrush(palette().panel_bg);
    }
    true
}

unsafe fn make_font(face: &str, height: i32, weight: i32) -> HFONT {
    CreateFontW(
        height, 0, 0, 0, weight, 0, 0, 0, 1, // DEFAULT_CHARSET
        0, 0, 5, // CLEARTYPE_QUALITY
        0, w(face).as_ptr(),
    )
}

/// 이름만으로 만든 글꼴이 실제 그 이름으로 해석되는지 확인
unsafe fn face_resolves(name: &str) -> bool {
    let hdc = GetDC(null_mut());
    let f = make_font(name, -16, 400);
    let old = SelectObject(hdc, f);
    let mut buf = [0u16; 64];
    let n = GetTextFaceW(hdc, 64, buf.as_mut_ptr());
    SelectObject(hdc, old);
    DeleteObject(f);
    ReleaseDC(null_mut(), hdc);
    let got = String::from_utf16_lossy(&buf[..(n - 1).max(0) as usize]);
    got.eq_ignore_ascii_case(name)
}

/// Jetendard(이름만으로 해석되지 않으면 사용자 글꼴 폴더에서 비공개 로드) -> 없으면 맑은 고딕
pub fn choose_face() -> String {
    unsafe {
        if face_resolves("Jetendard") {
            return "Jetendard".into();
        }
        if let Ok(la) = std::env::var("LOCALAPPDATA") {
            for f in ["Jetendard-Regular.ttf", "Jetendard-Bold.ttf"] {
                let p = format!("{la}\\Microsoft\\Windows\\Fonts\\{f}");
                AddFontResourceExW(w(&p).as_ptr(), FR_PRIVATE, null());
            }
            if face_resolves("Jetendard") {
                return "Jetendard".into();
            }
        }
    }
    "Malgun Gothic".into()
}

/// DPI 가 바뀌었을 때만 글꼴을 다시 만든다. 바뀌었으면 true.
pub fn apply_dpi(dpi: u32) -> bool {
    let a = app();
    if dpi == 0 || dpi == a.dpi {
        return false;
    }
    a.dpi = dpi;
    unsafe {
        for f in [a.f_input, a.f_name, a.f_dir, a.f_small, a.f_chip, a.f_badge] {
            if !f.is_null() {
                DeleteObject(f);
            }
        }
        let face = a.face.clone();
        a.f_input = make_font(&face, -sc(15.0), 400);
        a.f_name = make_font(&face, -sc(11.0), 700);
        a.f_dir = make_font(&face, -sc(11.0), 400);
        a.f_small = make_font(&face, -sc(10.0), 400);
        a.f_chip = make_font(&face, -sc(10.0), 700);
        a.f_badge = make_font(&face, -sc(9.0), 400);
    }
    true
}

/// 글꼴의 TEXTMETRICW
pub unsafe fn metrics_of(hdc: HDC, font: HFONT) -> TEXTMETRICW {
    let old = SelectObject(hdc, font);
    let mut tm: TEXTMETRICW = zeroed();
    GetTextMetricsW(hdc, &mut tm);
    SelectObject(hdc, old);
    tm
}
