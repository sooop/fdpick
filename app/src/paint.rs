//! 레이아웃 계산과 GDI 더블버퍼 그리기. `ViewState` 만 보고 그린다.

use std::mem::zeroed;
use std::ptr::null;

use fdpick_core::engine::Mode;
use fdpick_logic::view::{Row, StatusKind, ViewState};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;

use crate::gfx::{metrics_of, palette, sc};
use crate::state::app;
use crate::util::{blend, w};

// 논리 픽셀(96dpi 기준) 치수. 이전 구현 기본 크기의 80%
pub const L_WIN_W: f32 = 608.0;
pub const L_WIN_H: f32 = 416.0;
const L_BAR_H: f32 = 56.0;
const L_FOOT_H: f32 = 28.0;
pub const L_ROW_H: f32 = 29.0;
const L_PAD: f32 = 14.5;

#[derive(Clone, Copy)]
pub struct Layout {
    pub w: i32,
    pub h: i32,
    pub bar_h: i32,
    pub list_top: i32,
    pub list_bot: i32,
    pub row_h: i32,
}

pub fn layout() -> Layout {
    let h = sc(L_WIN_H);
    Layout { w: sc(L_WIN_W), h, bar_h: sc(L_BAR_H), list_top: sc(L_BAR_H) + 1, list_bot: h - sc(L_FOOT_H), row_h: sc(L_ROW_H) }
}

/// 목록 한 화면에 들어가는 행 수
pub fn page_size() -> usize {
    let l = layout();
    ((l.list_bot - l.list_top) / l.row_h).max(1) as usize
}

pub fn pad() -> i32 {
    sc(L_PAD)
}

/// 입력줄 오른쪽 여백(스크롤 표시자와 겹치지 않게 약간 더 둔다)
pub fn edit_right_pad() -> i32 {
    sc(L_PAD)
}

fn text_width(hdc: HDC, font: HFONT, t: &[u16]) -> i32 {
    unsafe {
        let old = SelectObject(hdc, font);
        let mut sz: SIZE = zeroed();
        GetTextExtentPoint32W(hdc, t.as_ptr(), t.len() as i32, &mut sz);
        SelectObject(hdc, old);
        sz.cx
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 모드 칩 사각형(라벨이 비면 None)
pub fn chip_rect(hdc: HDC, label: &str) -> Option<RECT> {
    if label.is_empty() {
        return None;
    }
    let a = app();
    let l = layout();
    let tm = unsafe { metrics_of(hdc, a.f_chip) };
    let tw = text_width(hdc, a.f_chip, &wide(label));
    let cw = tw + 2 * sc(7.0);
    let ch = tm.tmHeight + 2 * sc(3.0);
    let top = (l.bar_h - ch) / 2;
    Some(RECT { left: pad(), top, right: pad() + cw, bottom: top + ch })
}

/// 전체 콘텐츠 높이와 스크롤 가능한 최대값
pub fn max_scroll(rows: usize) -> i32 {
    let l = layout();
    (rows as i32 * l.row_h - (l.list_bot - l.list_top)).max(0)
}

/// 커서 행이 보이도록 스크롤을 조정한다.
pub fn ensure_visible(cursor: usize, rows: usize) {
    let l = layout();
    let a = app();
    let top = cursor as i32 * l.row_h;
    let view = l.list_bot - l.list_top;
    if top < a.scroll {
        a.scroll = top;
    } else if top + l.row_h > a.scroll + view {
        a.scroll = top + l.row_h - view;
    }
    a.scroll = a.scroll.clamp(0, max_scroll(rows));
}

/// 클릭 좌표 -> 행 인덱스
pub fn row_at(y: i32, rows: usize) -> Option<usize> {
    let l = layout();
    if y < l.list_top || y >= l.list_bot {
        return None;
    }
    let idx = ((y - l.list_top + app().scroll) / l.row_h) as usize;
    (idx < rows).then_some(idx)
}

unsafe fn fill(hdc: HDC, r: &RECT, color: COLORREF) {
    SetDCBrushColor(hdc, color);
    FillRect(hdc, r, GetStockObject(DC_BRUSH) as HBRUSH);
}

unsafe fn round_fill(hdc: HDC, r: &RECT, color: COLORREF, radius: i32) {
    SetDCBrushColor(hdc, color);
    let ob = SelectObject(hdc, GetStockObject(DC_BRUSH));
    let op = SelectObject(hdc, GetStockObject(NULL_PEN));
    RoundRect(hdc, r.left, r.top, r.right + 1, r.bottom + 1, radius * 2, radius * 2);
    SelectObject(hdc, op);
    SelectObject(hdc, ob);
}

/// 기준선(baseline) 좌표로 한 덩어리 글자를 그린다.
unsafe fn text_at(hdc: HDC, x: i32, yb: i32, t: &[u16], color: COLORREF) {
    if t.is_empty() {
        return;
    }
    SetTextColor(hdc, color);
    ExtTextOutW(hdc, x, yb, 0, null(), t.as_ptr(), t.len() as u32, null());
}

/// 현재 선택된 글꼴 기준 누적 폭(길이 n+1, [0]=0)
unsafe fn extents(hdc: HDC, t: &[u16]) -> Vec<i32> {
    let n = t.len();
    let mut out = vec![0i32; n + 1];
    if n == 0 {
        return out;
    }
    let mut dx = vec![0i32; n];
    let mut fit = 0i32;
    let mut sz: SIZE = zeroed();
    GetTextExtentExPointW(hdc, t.as_ptr(), n as i32, i32::MAX, &mut fit, dx.as_mut_ptr(), &mut sz);
    out[1..].copy_from_slice(&dx);
    out
}

/// 매치 글자(hit)를 다른 색으로 구간별로 그린다. t[from..to] 를 x 에서 시작해 그리고 끝 x 를 돌려준다.
#[allow(clippy::too_many_arguments)]
unsafe fn marked(hdc: HDC, x: i32, yb: i32, t: &[u16], hit: &[bool], pre: &[i32], from: usize, to: usize, normal: COLORREF, mark: COLORREF) -> i32 {
    let mut i = from;
    while i < to {
        let h = hit[i];
        let mut j = i + 1;
        while j < to && hit[j] == h {
            j += 1;
        }
        text_at(hdc, x + pre[i] - pre[from], yb, &t[i..j], if h { mark } else { normal });
        i = j;
    }
    x + pre[to] - pre[from]
}

/// 경로 문자열을 UTF-16 로 바꾸고, 각 UTF-16 단위의 매치 여부와 파일명 시작 오프셋을 구한다.
fn prepare(row: &Row) -> (Vec<u16>, Vec<bool>, usize) {
    let mut u: Vec<u16> = Vec::with_capacity(row.path.len());
    let mut hit: Vec<bool> = Vec::with_capacity(row.path.len());
    let mut name_u = None;
    let mut ip = 0usize;
    let mut buf = [0u16; 2];
    for (ci, ch) in row.path.chars().enumerate() {
        if ci == row.name_start {
            name_u = Some(u.len());
        }
        let is_hit = if ip < row.indices.len() && row.indices[ip] as usize == ci {
            ip += 1;
            true
        } else {
            false
        };
        for unit in ch.encode_utf16(&mut buf) {
            u.push(*unit);
            hit.push(is_hit);
        }
    }
    let ns = name_u.unwrap_or(u.len());
    (u, hit, ns)
}

const ELLIPSIS: [u16; 1] = [0x2026];

unsafe fn paint_row(hdc: HDC, row: &Row, selected: bool, y: i32) {
    let a = app();
    let l = layout();
    let p = palette();
    let rx0 = sc(6.5);
    let rx1 = l.w - sc(6.5);
    let rrect = RECT { left: rx0, top: y, right: rx1, bottom: y + l.row_h };
    if selected {
        round_fill(hdc, &rrect, p.row_sel, sc(6.5));
    }
    if row.picked {
        fill(hdc, &RECT { left: rx0, top: y, right: rx0 + sc(3.0), bottom: y + l.row_h }, p.accent);
    }
    if row.recent_last {
        fill(hdc, &RECT { left: rx0, top: y + l.row_h - 1, right: rx1, bottom: y + l.row_h }, p.divider);
    }
    let bg = if selected { p.row_sel } else { p.panel_bg };
    let fade = |c: COLORREF| if row.missing { blend(c, bg, 128) } else { c };

    // 공통 기준선: 이름 글꼴의 수직 중앙 기준
    let tm = metrics_of(hdc, a.f_name);
    let yb = y + (l.row_h - (tm.tmAscent + tm.tmDescent)) / 2 + tm.tmAscent;

    let mut x = rx0 + sc(8.0);
    // 아이콘 글리프: 디렉터리 ▸ / 파일 ·
    let (glyph, gcol) = if row.is_dir { (0x25B8u16, p.chip_text) } else { (0x00B7u16, p.muted) };
    SelectObject(hdc, a.f_small);
    text_at(hdc, x, yb, &[glyph], fade(gcol));
    x += sc(8.0) + sc(6.5);

    // 최근 배지
    if row.recent_badge {
        let label = wide("최근");
        let tw = text_width(hdc, a.f_badge, &label);
        let bm = metrics_of(hdc, a.f_badge);
        let bw = tw + sc(8.0);
        let bh = bm.tmHeight.min(sc(13.0)).max(bm.tmHeight - 2);
        let top = y + (l.row_h - bh) / 2;
        round_fill(hdc, &RECT { left: x, top, right: x + bw, bottom: top + bh }, fade(p.divider), sc(3.0));
        SelectObject(hdc, a.f_badge);
        text_at(hdc, x + sc(4.0), top + (bh - (bm.tmAscent + bm.tmDescent)) / 2 + bm.tmAscent, &label, fade(p.muted));
        x += bw + sc(6.5);
    }

    // 오른쪽 보조 문구
    let mut right = rx1 - sc(8.0);
    if let Some(h) = &row.hint {
        let ht = wide(h);
        let hw = text_width(hdc, a.f_small, &ht);
        SelectObject(hdc, a.f_small);
        text_at(hdc, right - hw, yb, &ht, fade(p.muted));
        right -= hw + sc(12.0);
    }

    let avail = right - x;
    if avail <= 0 {
        return;
    }
    let (u, hit, ns) = prepare(row);
    let (dir_u, name_u) = (&u[..ns], &u[ns..]);

    SelectObject(hdc, a.f_dir);
    let dir_pre = extents(hdc, dir_u);
    let dir_w = *dir_pre.last().unwrap();
    let ell_dir = *extents(hdc, &ELLIPSIS).last().unwrap();
    SelectObject(hdc, a.f_name);
    let name_pre = extents(hdc, name_u);
    let name_w = *name_pre.last().unwrap();
    let ell_name = *extents(hdc, &ELLIPSIS).last().unwrap();

    // 폭 배분: 파일명은 항상 보이게(최소 전체의 70% 까지 보장), 남는 폭을 상위 경로가 쓴다
    let name_draw = if name_w + dir_w <= avail { name_w } else { name_w.min(((avail * 7) / 10).max(avail - dir_w)) };
    let dir_alloc = avail - name_draw;

    let mut cx = x;
    // 상위 경로: 넘치면 왼쪽을 "…" 로 줄인다
    SelectObject(hdc, a.f_dir);
    let dir_col = fade(p.muted);
    let mark_col = fade(p.mark);
    if dir_w <= dir_alloc {
        cx = marked(hdc, cx, yb, dir_u, &hit[..ns], &dir_pre, 0, ns, dir_col, mark_col);
    } else if dir_alloc > ell_dir && !dir_u.is_empty() {
        let mut k = 0usize;
        while k < ns && ell_dir + (dir_w - dir_pre[k]) > dir_alloc {
            k += 1;
        }
        if k < ns && (0xDC00..0xE000).contains(&dir_u[k]) {
            k += 1;
        }
        text_at(hdc, cx, yb, &ELLIPSIS, dir_col);
        cx += ell_dir;
        if k < ns {
            cx = marked(hdc, cx, yb, dir_u, &hit[..ns], &dir_pre, k, ns, dir_col, mark_col);
        }
    }

    // 파일명(굵게): 넘치면 끝을 "…" 로 줄인다
    SelectObject(hdc, a.f_name);
    let name_col = fade(p.text);
    let nhit = &hit[ns..];
    if name_w <= name_draw {
        marked(hdc, cx, yb, name_u, nhit, &name_pre, 0, name_u.len(), name_col, mark_col);
    } else if name_draw > ell_name {
        let mut k = name_u.len();
        while k > 0 && name_pre[k] + ell_name > name_draw {
            k -= 1;
        }
        if k > 0 && k < name_u.len() && (0xDC00..0xE000).contains(&name_u[k]) {
            k -= 1;
        }
        let ex = marked(hdc, cx, yb, name_u, nhit, &name_pre, 0, k, name_col, mark_col);
        text_at(hdc, ex, yb, &ELLIPSIS, name_col);
    }
}

unsafe fn paint(hdc: HDC, v: &ViewState) {
    let a = app();
    let l = layout();
    let p = palette();
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextAlign(hdc, TA_BASELINE);
    let full = RECT { left: 0, top: 0, right: l.w, bottom: l.h };
    FillRect(hdc, &full, a.brush_bg);

    // 모드 칩
    if let Some(r) = chip_rect(hdc, &v.mode_label) {
        let (bg, fg) = match v.mode {
            Mode::Fuzzy => (p.chip_bg, p.chip_text),
            Mode::Path => (p.accent, 0x00ff_ffff),
            Mode::Recent => p.recent_chip,
        };
        round_fill(hdc, &r, bg, sc(5.0));
        let tm = metrics_of(hdc, a.f_chip);
        SelectObject(hdc, a.f_chip);
        text_at(hdc, r.left + sc(7.0), r.top + sc(3.0) + tm.tmAscent, &wide(&v.mode_label), fg);
    }

    // 구분선(입력줄 아래, 푸터 위)
    fill(hdc, &RECT { left: 0, top: l.bar_h, right: l.w, bottom: l.bar_h + 1 }, p.divider);
    fill(hdc, &RECT { left: 0, top: l.list_bot, right: l.w, bottom: l.list_bot + 1 }, p.divider);

    // 목록(클리핑)
    let saved = SaveDC(hdc);
    IntersectClipRect(hdc, 0, l.list_top, l.w, l.list_bot);
    let first = (a.scroll / l.row_h) as usize;
    let mut i = first;
    while i < v.rows.len() {
        let y = l.list_top + i as i32 * l.row_h - a.scroll;
        if y >= l.list_bot {
            break;
        }
        paint_row(hdc, &v.rows[i], v.cursor == Some(i), y);
        i += 1;
    }
    // 빈 목록 안내
    if v.rows.is_empty() {
        if let Some(h) = &v.empty_hint {
            let t = wide(h);
            let tw = text_width(hdc, a.f_dir, &t);
            let tm = metrics_of(hdc, a.f_dir);
            SelectObject(hdc, a.f_dir);
            let yb = l.list_top + ((l.list_bot - l.list_top) - (tm.tmAscent + tm.tmDescent)) / 2 + tm.tmAscent;
            text_at(hdc, (l.w - tw) / 2, yb, &t, p.muted);
        }
    }
    // 스크롤 표시자
    let total = v.rows.len() as i32 * l.row_h;
    let view = l.list_bot - l.list_top;
    if total > view {
        let bar_h = (view * view / total).max(sc(24.0));
        let bar_y = l.list_top + (view - bar_h) * a.scroll / (total - view);
        round_fill(hdc, &RECT { left: l.w - sc(6.0), top: bar_y, right: l.w - sc(3.0), bottom: bar_y + bar_h }, p.divider, sc(1.5));
    }
    RestoreDC(hdc, saved);

    // 푸터: 도움말(왼쪽) + 상태(오른쪽)
    let tm = metrics_of(hdc, a.f_small);
    let fy = l.list_bot + 1;
    let yb = fy + ((l.h - fy) - (tm.tmAscent + tm.tmDescent)) / 2 + tm.tmAscent;
    SelectObject(hdc, a.f_small);
    let status = wide(&v.status);
    let sw = text_width(hdc, a.f_small, &status);
    let scol = match v.status_kind {
        StatusKind::Normal => p.muted,
        StatusKind::Running => p.chip_text,
        StatusKind::Error => p.err,
    };
    let max_status = (l.w - 2 * pad()) * 2 / 3;
    let sdraw = sw.min(max_status);
    draw_ellipsized(hdc, l.w - pad() - sdraw, yb, &status, sdraw, scol, false);
    let help = wide(&v.help);
    let help_w = (l.w - 2 * pad() - sdraw - sc(13.0)).max(0);
    draw_ellipsized(hdc, pad(), yb, &help, help_w, p.muted, false);

    // 패널 테두리
    SetDCBrushColor(hdc, p.border);
    FrameRect(hdc, &full, GetStockObject(DC_BRUSH) as HBRUSH);
}

/// 폭 max_w 안에 들어가게 끝을 줄여 그린다(현재 선택된 글꼴 기준).
unsafe fn draw_ellipsized(hdc: HDC, x: i32, yb: i32, t: &[u16], max_w: i32, color: COLORREF, _left: bool) {
    if t.is_empty() || max_w <= 0 {
        return;
    }
    let pre = extents(hdc, t);
    if *pre.last().unwrap() <= max_w {
        text_at(hdc, x, yb, t, color);
        return;
    }
    let ell = *extents(hdc, &ELLIPSIS).last().unwrap();
    let mut k = t.len();
    while k > 0 && pre[k] + ell > max_w {
        k -= 1;
    }
    if k > 0 && k < t.len() && (0xDC00..0xE000).contains(&t[k]) {
        k -= 1;
    }
    text_at(hdc, x, yb, &t[..k], color);
    text_at(hdc, x + pre[k], yb, &ELLIPSIS, color);
}

/// WM_PAINT: 메모리 DC 에 모두 그린 뒤 한 번에 복사한다(깜빡임 없음).
pub unsafe fn on_paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let l = layout();
    if let Some(v) = app().view.as_ref() {
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, l.w, l.h);
        let old = SelectObject(mem, bmp);
        paint(mem, v);
        BitBlt(hdc, 0, 0, l.w, l.h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
    } else {
        FillRect(hdc, &RECT { left: 0, top: 0, right: l.w, bottom: l.h }, app().brush_bg);
    }
    EndPaint(hwnd, &ps);
}

/// 입력창이 비었을 때 placeholder 를 EDIT 위에 그린다(EDIT 서브클래스의 WM_PAINT 뒤에서 호출).
pub unsafe fn draw_placeholder(edit: HWND) {
    let a = app();
    let Some(v) = a.view.as_ref() else { return };
    if v.placeholder.is_empty() {
        return;
    }
    let hdc = GetDC(edit);
    if hdc.is_null() {
        return;
    }
    SetBkMode(hdc, TRANSPARENT as i32);
    let old = SelectObject(hdc, a.f_input);
    SetTextColor(hdc, palette().muted);
    let mut rc: RECT = zeroed();
    windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(edit, &mut rc);
    rc.left += sc(2.0);
    let t = w(&v.placeholder);
    DrawTextW(
        hdc,
        t.as_ptr(),
        (t.len() - 1) as i32,
        &mut rc,
        DT_SINGLELINE | DT_LEFT | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
    );
    SelectObject(hdc, old);
    ReleaseDC(edit, hdc);
}
