// fdpick Win32 직접 구현 프로토타입 (유휴 메모리 하한 측정용)
// - 입력창: 시스템 EDIT 자식 컨트롤 (IME 는 시스템이 처리)
// - 목록/상태줄: 부모 WM_PAINT 에서 메모리 DC 더블버퍼링 GDI 직접 그리기
#![windows_subsystem = "windows"]
#![allow(static_mut_refs)]
#![allow(clippy::missing_safety_doc)]

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::*;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Controls::EM_SETMARGINS;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "FdPickProtoWin32";
const ID_HOTKEY_TOGGLE: i32 = 1;
const ID_HOTKEY_QUIT: i32 = 2;
const ID_TIMER_HIDE: usize = 1;

// 논리 픽셀(96dpi 기준) 레이아웃 상수
const L_WIN_W: i32 = 608;
const L_WIN_H: i32 = 416;
const L_INPUT_H: i32 = 56;
const L_STATUS_H: i32 = 28;
const L_ROW_H: i32 = 29;
const L_PAD: i32 = 20;
const L_RADIUS: i32 = 12;

struct State {
    hwnd: HWND,
    edit: HWND,
    dpi: u32,
    dark: bool,
    dwm_round_ok: bool,
    f_input: HFONT,
    f_name: HFONT,
    f_dir: HFONT,
    brush_bg: HBRUSH,
    face: String,
    rows: Vec<(Vec<u16>, Vec<u16>)>, // (상위 경로, 파일명)
    sel: usize,
    scroll: i32,
    chars: usize,
    en_change: u32,
    en_change_comp: u32,
    composing: bool,
    ime_start: u32,
    ime_end: u32,
    orig_edit_proc: WNDPROC,
    trim: bool,
}

static mut ST: State = State {
    hwnd: null_mut(),
    edit: null_mut(),
    dpi: 0,
    dark: true,
    dwm_round_ok: false,
    f_input: null_mut(),
    f_name: null_mut(),
    f_dir: null_mut(),
    brush_bg: null_mut(),
    face: String::new(),
    rows: Vec::new(),
    sel: 0,
    scroll: 0,
    chars: 0,
    en_change: 0,
    en_change_comp: 0,
    composing: false,
    ime_start: 0,
    ime_end: 0,
    orig_edit_proc: None,
    trim: false,
};

fn st() -> &'static mut State {
    unsafe { &mut ST }
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

// 진단 로그: 실행 파일 옆 proto_win32.log
fn log(msg: &str) {
    use std::io::Write;
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.with_file_name("proto_win32.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{}", msg);
        }
    }
}

// 논리 -> 물리 픽셀
fn s(x: i32) -> i32 {
    (x as i64 * st().dpi.max(96) as i64 + 48) as i32 / 96
}

struct Theme {
    bg: COLORREF,
    text: COLORREF,
    dim: COLORREF,
    sel: COLORREF,
    hit: COLORREF,
    sep: COLORREF,
}

fn theme() -> Theme {
    if st().dark {
        Theme {
            bg: rgb(32, 33, 36),
            text: rgb(240, 240, 242),
            dim: rgb(140, 143, 150),
            sel: rgb(58, 62, 78),
            hit: rgb(255, 201, 87),
            sep: rgb(60, 62, 68),
        }
    } else {
        Theme {
            bg: rgb(250, 250, 251),
            text: rgb(24, 24, 28),
            dim: rgb(128, 132, 140),
            sel: rgb(222, 232, 250),
            hit: rgb(206, 92, 0),
            sep: rgb(222, 224, 228),
        }
    }
}

// 레지스트리 AppsUseLightTheme 읽기. 읽기 실패 시 라이트로 간주
unsafe fn read_dark_mode() -> bool {
    let mut data: u32 = 1;
    let mut sz: u32 = 4;
    let r = RegGetValueW(
        HKEY_CURRENT_USER,
        w("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
        w("AppsUseLightTheme").as_ptr(),
        RRF_RT_REG_DWORD,
        null_mut(),
        &mut data as *mut u32 as *mut _,
        &mut sz,
    );
    r == 0 && data == 0
}

unsafe fn refresh_theme() {
    let dark = read_dark_mode();
    let t = st();
    if !t.brush_bg.is_null() && t.dark == dark {
        return;
    }
    t.dark = dark;
    if !t.brush_bg.is_null() {
        DeleteObject(t.brush_bg);
    }
    t.brush_bg = CreateSolidBrush(theme().bg);
    if !t.hwnd.is_null() {
        InvalidateRect(t.hwnd, null(), 0);
    }
}

unsafe fn make_font(face: &str, height: i32, weight: i32) -> HFONT {
    CreateFontW(
        height, 0, 0, 0, weight, 0, 0, 0, 1, // DEFAULT_CHARSET
        0, 0, 5, // CLEARTYPE_QUALITY
        0, w(face).as_ptr(),
    )
}

// 이름만으로 만든 글꼴이 실제 그 이름으로 해석되는지 확인(GetTextFaceW)
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
    log(&format!("face check '{}' -> '{}'", name, got));
    got.eq_ignore_ascii_case(name)
}

unsafe fn choose_face() -> String {
    if face_resolves("Jetendard") {
        log("Jetendard: CreateFontW 이름만으로 해석됨");
        return "Jetendard".into();
    }
    log("Jetendard: 이름만으로 해석 실패, AddFontResourceExW 시도");
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        for f in ["Jetendard-Regular.ttf", "Jetendard-Bold.ttf"] {
            let p = format!("{}\\Microsoft\\Windows\\Fonts\\{}", la, f);
            let n = AddFontResourceExW(w(&p).as_ptr(), FR_PRIVATE, null());
            log(&format!("AddFontResourceExW {} -> {}", p, n));
        }
        if face_resolves("Jetendard") {
            return "Jetendard".into();
        }
    }
    log("Jetendard 사용 불가, Malgun Gothic 대체");
    "Malgun Gothic".into()
}

fn build_rows() -> Vec<(Vec<u16>, Vec<u16>)> {
    let dirs = [
        "D:\\projects\\보고서\\2026",
        "D:\\projects\\fdpick\\src-tauri\\src",
        "C:\\Users\\yepben\\Documents\\회의록",
        "D:\\work\\물류시스템\\정산\\2026-10",
        "C:\\Users\\yepben\\Downloads",
        "D:\\projects\\tracx\\frontend\\src\\components",
        "E:\\백업\\사진\\2025\\여름휴가",
        "D:\\projects\\QxFront\\Pages\\주문관리\\배송조회",
        "C:\\Program Files\\Common Files\\Microsoft Shared",
        "D:\\data\\export\\2026\\Q3\\거래처별",
    ];
    let names = [
        ("report_final_v2", ".xlsx"),
        ("최종보고서_수정본", ".docx"),
        ("main", ".rs"),
        ("README", ".md"),
        ("2026년_매출현황(확정)", ".xlsx"),
        ("invoice_2026-10-01", ".pdf"),
        ("설계문서_v3", ".pptx"),
        ("package", ".json"),
        ("배송조회_화면정의서", ".hwp"),
        ("Cargo", ".lock"),
        ("screenshot", ".png"),
    ];
    (0..100usize)
        .map(|i| {
            let (d, n) = if i == 0 {
                (dirs[0], names[0])
            } else {
                (dirs[(i * 7) % dirs.len()], names[(i * 3 + i / 10) % names.len()])
            };
            let name = if i < 20 { format!("{}{}", n.0, n.1) } else { format!("{}_{}{}", n.0, i, n.1) };
            (d.encode_utf16().collect(), name.encode_utf16().collect())
        })
        .collect()
}

struct Metrics {
    w: i32,
    h: i32,
    list_top: i32,
    list_bot: i32,
    row_h: i32,
}

fn metrics() -> Metrics {
    let h = s(L_WIN_H);
    Metrics { w: s(L_WIN_W), h, list_top: s(L_INPUT_H) + 1, list_bot: h - s(L_STATUS_H), row_h: s(L_ROW_H) }
}

unsafe fn clamp_scroll() {
    let m = metrics();
    let total = st().rows.len() as i32 * m.row_h;
    let max = (total - (m.list_bot - m.list_top)).max(0);
    st().scroll = st().scroll.clamp(0, max);
}

unsafe fn ensure_visible() {
    let m = metrics();
    let top = st().sel as i32 * m.row_h;
    let view = m.list_bot - m.list_top;
    if top < st().scroll {
        st().scroll = top;
    } else if top + m.row_h > st().scroll + view {
        st().scroll = top + m.row_h - view;
    }
    clamp_scroll();
}

unsafe fn move_sel(delta: i32) {
    let n = st().rows.len() as i32;
    st().sel = (st().sel as i32 + delta).clamp(0, n - 1) as usize;
    ensure_visible();
    InvalidateRect(st().hwnd, null(), 0);
}

// DPI 변경 적용: 글꼴 재생성 + 입력창 재배치
unsafe fn apply_dpi(d: u32) {
    let t = st();
    if d == t.dpi || d == 0 {
        return;
    }
    t.dpi = d;
    for f in [t.f_input, t.f_name, t.f_dir] {
        if !f.is_null() {
            DeleteObject(f);
        }
    }
    let face = t.face.clone();
    t.f_input = make_font(&face, -s(15), 400);
    t.f_name = make_font(&face, -s(11), 700);
    t.f_dir = make_font(&face, -s(11), 400);
    if !t.edit.is_null() {
        SendMessageW(t.edit, WM_SETFONT, t.f_input as WPARAM, 1);
        let hdc = GetDC(t.edit);
        let old = SelectObject(hdc, t.f_input);
        let mut tm: TEXTMETRICW = zeroed();
        GetTextMetricsW(hdc, &mut tm);
        SelectObject(hdc, old);
        ReleaseDC(t.edit, hdc);
        let th = tm.tmHeight;
        MoveWindow(t.edit, s(L_PAD), (s(L_INPUT_H) - th) / 2, s(L_WIN_W) - 2 * s(L_PAD), th, 1);
    }
    clamp_scroll();
}

// 커서가 있는 모니터 기준으로 창 배치
unsafe fn place_window() {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(mon, &mut mi);
    let (mut dx, mut dy) = (96u32, 96u32);
    GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    apply_dpi(dx);
    let (ww, wh) = (s(L_WIN_W), s(L_WIN_H));
    let work = mi.rcWork;
    let x = work.left + ((work.right - work.left) - ww) / 2;
    let y = work.top + (work.bottom - work.top) * 15 / 100;
    let hwnd = st().hwnd;
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, ww, wh, SWP_NOACTIVATE);
    // 이동 중 WM_DPICHANGED 로 DPI 가 바뀌었을 수 있어 한 번 더 확정
    apply_dpi(dx);
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, ww, wh, SWP_NOACTIVATE);
    if !st().dwm_round_ok {
        let r = 2 * s(L_RADIUS);
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, ww + 1, wh + 1, r, r), 1);
    }
}

unsafe fn show_window() {
    refresh_theme();
    place_window();
    let t = st();
    ShowWindow(t.hwnd, SW_SHOW);
    SetForegroundWindow(t.hwnd);
    SetFocus(t.edit);
}

unsafe fn hide_window() {
    ShowWindow(st().hwnd, SW_HIDE);
    if st().trim {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

// 한 행 안에서 여러 색 세그먼트로 그리기
unsafe fn draw_seg(hdc: HDC, x: &mut i32, y: i32, text: &[u16], color: COLORREF) {
    if text.is_empty() {
        return;
    }
    SetTextColor(hdc, color);
    let mut sz: SIZE = zeroed();
    GetTextExtentPoint32W(hdc, text.as_ptr(), text.len() as i32, &mut sz);
    ExtTextOutW(hdc, *x, y, 0, null(), text.as_ptr(), text.len() as u32, null());
    *x += sz.cx;
}

unsafe fn paint(hdc: HDC) {
    let t = st();
    let m = metrics();
    let th = theme();
    SetBkMode(hdc, TRANSPARENT as i32);
    let full = RECT { left: 0, top: 0, right: m.w, bottom: m.h };
    FillRect(hdc, &full, t.brush_bg);

    let dc_brush = GetStockObject(DC_BRUSH);
    // 구분선
    SetDCBrushColor(hdc, th.sep);
    let sep = RECT { left: 0, top: m.list_top - 1, right: m.w, bottom: m.list_top };
    FillRect(hdc, &sep, dc_brush);
    let sep2 = RECT { left: 0, top: m.list_bot, right: m.w, bottom: m.list_bot + 1 };
    FillRect(hdc, &sep2, dc_brush);

    // 목록
    IntersectClipRect(hdc, 0, m.list_top, m.w, m.list_bot);
    let old = SelectObject(hdc, t.f_name);
    let mut tm: TEXTMETRICW = zeroed();
    GetTextMetricsW(hdc, &mut tm);
    let ty_off = (m.row_h - tm.tmHeight) / 2;
    let first = (t.scroll / m.row_h) as usize;
    let mut i = first;
    while i < t.rows.len() {
        let y = m.list_top + i as i32 * m.row_h - t.scroll;
        if y >= m.list_bot {
            break;
        }
        if i == t.sel {
            SetDCBrushColor(hdc, th.sel);
            let r = RECT { left: s(8), top: y, right: m.w - s(8), bottom: y + m.row_h };
            FillRect(hdc, &r, dc_brush);
        }
        let (dir, name) = &t.rows[i];
        let mut x = s(L_PAD);
        SelectObject(hdc, t.f_name);
        // 매치 글자 강조 예시: 파일명에서 3글자 연속
        let hs = (i % 3).min(name.len());
        let he = (hs + 3).min(name.len());
        draw_seg(hdc, &mut x, y + ty_off, &name[..hs], th.text);
        draw_seg(hdc, &mut x, y + ty_off, &name[hs..he], th.hit);
        draw_seg(hdc, &mut x, y + ty_off, &name[he..], th.text);
        // 상위 경로: 남은 폭에 맞춰 경로 말줄임
        x += s(12);
        SelectObject(hdc, t.f_dir);
        SetTextColor(hdc, th.dim);
        let mut r = RECT { left: x, top: y, right: m.w - s(L_PAD), bottom: y + m.row_h };
        if r.right > r.left {
            DrawTextW(
                hdc,
                dir.as_ptr(),
                dir.len() as i32,
                &mut r,
                DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX | DT_PATH_ELLIPSIS,
            );
        }
        i += 1;
    }
    // 스크롤 표시자
    let total = t.rows.len() as i32 * m.row_h;
    let view = m.list_bot - m.list_top;
    if total > view {
        let bar_h = (view * view / total).max(s(24));
        let bar_y = m.list_top + (view - bar_h) * t.scroll / (total - view);
        SetDCBrushColor(hdc, th.sep);
        let r = RECT { left: m.w - s(6), top: bar_y, right: m.w - s(3), bottom: bar_y + bar_h };
        FillRect(hdc, &r, dc_brush);
    }
    SelectClipRgn(hdc, null_mut());

    // 상태줄
    SelectObject(hdc, t.f_dir);
    SetTextColor(hdc, th.dim);
    let status = format!(
        "{}/{}  입력 {}자  EN_CHANGE {} (조합중 {})  IME {}/{}  {} {}dpi {}",
        t.sel + 1,
        t.rows.len(),
        t.chars,
        t.en_change,
        t.en_change_comp,
        t.ime_start,
        t.ime_end,
        t.face,
        t.dpi,
        if t.dark { "dark" } else { "light" }
    );
    let sw = w(&status);
    let mut r = RECT { left: s(L_PAD), top: m.list_bot + 1, right: m.w - s(L_PAD), bottom: m.h };
    DrawTextW(hdc, sw.as_ptr(), (sw.len() - 1) as i32, &mut r, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(hdc, old);
}

unsafe fn on_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let m = metrics();
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, m.w, m.h);
    let old = SelectObject(mem, bmp);
    paint(mem);
    BitBlt(hdc, 0, 0, m.w, m.h, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn edit_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => match wp as u16 {
            VK_ESCAPE => {
                hide_window();
                return 0;
            }
            VK_UP => {
                move_sel(-1);
                return 0;
            }
            VK_DOWN => {
                move_sel(1);
                return 0;
            }
            VK_PRIOR => {
                move_sel(-10);
                return 0;
            }
            VK_NEXT => {
                move_sel(10);
                return 0;
            }
            _ => {}
        },
        // Esc/Enter 의 WM_CHAR 는 EDIT 가 경고음을 내므로 삼킨다
        WM_CHAR if wp == 27 || wp == 13 => return 0,
        WM_IME_STARTCOMPOSITION => {
            st().composing = true;
            st().ime_start += 1;
        }
        WM_IME_ENDCOMPOSITION => {
            st().composing = false;
            st().ime_end += 1;
        }
        _ => {}
    }
    CallWindowProcW(st().orig_edit_proc, h, msg, wp, lp)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            on_paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let th = theme();
            SetTextColor(wp as HDC, th.text);
            SetBkColor(wp as HDC, th.bg);
            st().brush_bg as LRESULT
        }
        WM_COMMAND => {
            if (wp >> 16) as u32 == EN_CHANGE && lp as HWND == st().edit {
                let t = st();
                let len = GetWindowTextLengthW(t.edit);
                let mut buf = vec![0u16; len as usize + 1];
                let n = GetWindowTextW(t.edit, buf.as_mut_ptr(), len + 1);
                t.chars = String::from_utf16_lossy(&buf[..n as usize]).chars().count();
                t.en_change += 1;
                if t.composing {
                    t.en_change_comp += 1;
                }
                let m = metrics();
                let r = RECT { left: 0, top: m.list_bot, right: m.w, bottom: m.h };
                InvalidateRect(hwnd, &r, 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as u16 as i16 as i32;
            st().scroll -= delta * s(L_ROW_H) * 3 / 120;
            clamp_scroll();
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let y = (lp >> 16) as u16 as i16 as i32;
            let m = metrics();
            if y >= m.list_top && y < m.list_bot {
                let idx = ((y - m.list_top + st().scroll) / m.row_h) as usize;
                if idx < st().rows.len() {
                    st().sel = idx;
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            SetFocus(st().edit);
            0
        }
        WM_HOTKEY => {
            match wp as i32 {
                ID_HOTKEY_TOGGLE => {
                    if IsWindowVisible(hwnd) != 0 {
                        hide_window();
                    } else {
                        show_window();
                    }
                }
                ID_HOTKEY_QUIT => PostQuitMessage(0),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            if wp == ID_TIMER_HIDE {
                KillTimer(hwnd, ID_TIMER_HIDE);
                hide_window();
            }
            0
        }
        WM_DPICHANGED => {
            apply_dpi((wp & 0xffff) as u32);
            0
        }
        WM_SETTINGCHANGE => {
            refresh_theme();
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    unsafe {
        let _ = std::fs::remove_file(std::env::current_exe().unwrap().with_file_name("proto_win32.log"));
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let args: Vec<String> = std::env::args().collect();
        let mut hide_after: Option<u32> = None;
        let mut start_hidden = false;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--hide-after" => {
                    hide_after = args.get(i + 1).and_then(|v| v.parse().ok());
                    i += 1;
                }
                "--start-hidden" => start_hidden = true,
                "--trim" => st().trim = true,
                _ => {}
            }
            i += 1;
        }

        st().face = choose_face();
        st().rows = build_rows();
        refresh_theme();

        let hinst = GetModuleHandleW(null());
        let cls = w(CLASS_NAME);
        let mut wc: WNDCLASSEXW = zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(wnd_proc);
        wc.hInstance = hinst;
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.lpszClassName = cls.as_ptr();
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            cls.as_ptr(),
            w("fdpick proto win32").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            0, 0, 100, 100,
            null_mut(), null_mut(), hinst, null(),
        );
        st().hwnd = hwnd;

        // Windows 11 라운드 코너. 실패(Windows 10 등)하면 SetWindowRgn 폴백
        let pref: i32 = DWMWCP_ROUND;
        let hr = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const i32 as *const _,
            4,
        );
        st().dwm_round_ok = hr >= 0;
        log(&format!("DwmSetWindowAttribute corner hr=0x{:08x} ok={}", hr as u32, st().dwm_round_ok));

        // 입력창: 시스템 EDIT 자식 컨트롤
        let edit = CreateWindowExW(
            0,
            w("EDIT").as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
            0, 0, 100, 20,
            hwnd, null_mut(), hinst, null(),
        );
        st().edit = edit;
        SendMessageW(edit, EM_SETMARGINS, 3, 0); // EC_LEFTMARGIN|EC_RIGHTMARGIN = 0
        let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as usize as isize);
        st().orig_edit_proc = std::mem::transmute::<isize, WNDPROC>(old);

        let ok1 = RegisterHotKey(hwnd, ID_HOTKEY_TOGGLE, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'F' as u32);
        let ok2 = RegisterHotKey(hwnd, ID_HOTKEY_QUIT, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'Q' as u32);
        log(&format!("RegisterHotKey Ctrl+Alt+F={} Ctrl+Alt+Q={}", ok1, ok2));

        place_window();
        if !start_hidden {
            show_window();
        }
        if let Some(sec) = hide_after {
            SetTimer(hwnd, ID_TIMER_HIDE, sec * 1000, None);
        }
        log(&format!("ready dpi={} dark={} face={}", st().dpi, st().dark, st().face));

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
