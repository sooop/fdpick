//! 공용 보조 함수(UTF-16 변환, 색 상수).

use windows_sys::Win32::Foundation::COLORREF;

/// NUL 종료 UTF-16 문자열
pub fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `#rrggbb` 형태의 상수를 COLORREF 로 바꾼다.
pub const fn hex(v: u32) -> COLORREF {
    ((v >> 16) & 0xff) | (v & 0xff00) | ((v & 0xff) << 16)
}

/// 두 색을 t(0..=255) 비율로 섞는다(a 가 t=0, b 가 t=255).
pub fn blend(a: COLORREF, b: COLORREF, t: u32) -> COLORREF {
    let mix = |sh: u32| {
        let x = (a >> sh) & 0xff;
        let y = (b >> sh) & 0xff;
        (x * (255 - t) + y * t) / 255
    };
    mix(0) | (mix(8) << 8) | (mix(16) << 16)
}

pub fn lo_word(v: usize) -> u32 {
    (v & 0xffff) as u32
}

pub fn hi_word(v: usize) -> u32 {
    ((v >> 16) & 0xffff) as u32
}

/// LPARAM 의 부호 있는 x, y 좌표
pub fn point_of(lp: isize) -> (i32, i32) {
    ((lp & 0xffff) as u16 as i16 as i32, ((lp >> 16) & 0xffff) as u16 as i16 as i32)
}

/// 실행 파일 이름(확장자 제외). 정식 이름("fdpick")이 아니면 테스트 사본으로 보고
/// 단일 인스턴스 이름과 자동 시작 동기화를 분리한다.
pub fn exe_stem() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "fdpick".to_string())
}

/// 정식 실행 파일이 아니면 인스턴스 접미사를 돌려준다.
pub fn instance_suffix() -> String {
    let stem = exe_stem();
    if stem.eq_ignore_ascii_case("fdpick") {
        String::new()
    } else {
        format!("-{stem}")
    }
}
