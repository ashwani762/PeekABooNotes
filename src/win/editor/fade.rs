// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The soft edges of a scrolled note: text scrolling up under the header, or running on
//! past the bottom, melts into the paper instead of being sliced off. Each edge shows only
//! while there's more text beyond it, growing in over the first pixels of scrolling.
//!
//! RichEdit draws itself, outside WM_PAINT too, so the fades go on afterwards, over its
//! pixels. Fading twice would fade more, so a band is always drawn fresh by RichEdit
//! first, and only the bands just drawn fresh are faded. When the view has scrolled,
//! RichEdit's own scroll has moved faded rows into the text (down from the top band when
//! scrolling up, up from the bottom one when scrolling down), so those rows are drawn
//! fresh too. When a band wasn't touched (typing in the middle) it's left as it is:
//! redrawing it on every key showed it unfaded for a moment, a flicker at the edge.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, DeleteObject, GetDC, GetUpdateRect, IntersectClipRect, RDW_INVALIDATE, RDW_UPDATENOW, RGN_OR, RedrawWindow, ReleaseDC,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, HideCaret, ShowCaret};

use crate::win::editor::rich;
use crate::win::render::gfx::Canvas;
use crate::win::util::rect;

/// Height of each fade, in device-independent pixels.
const BAND: f32 = 14.0;

pub const TOP: u8 = 1;
pub const BOTTOM: u8 = 2;

thread_local! {
    /// Inside our own redraw: the bands RichEdit has just drawn afresh.
    static FRESH: Cell<u8> = const { Cell::new(0) };
    /// Each editor's scroll position when its fades were last drawn.
    static SEEN: RefCell<HashMap<isize, i32>> = RefCell::new(HashMap::new());
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

/// After RichEdit has drawn editor `e` (on `paper`, at scale `s`): fade its edges.
/// `touched`: the bands (TOP, BOTTOM) what it drew may have reached.
pub fn paint(e: HWND, paper: u32, s: f32, touched: u8) {
    let fresh = FRESH.get();
    if fresh != 0 {
        draw(e, paper, s, fresh);
        return;
    }
    let y = rich::scroll_y(e);
    let was = SEEN.with(|m| m.borrow_mut().insert(e as isize, y));
    let moved = was != Some(y);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let band = band(s);
    // A repaint still waiting: if it reaches a band, it's drawn fresh along with ours.
    let mut u: RECT = unsafe { std::mem::zeroed() };
    let pending = if unsafe { GetUpdateRect(e, &mut u, 0) } != 0 { reach(u.top, u.bottom, rc.bottom, s) } else { 0 };
    let mut need = 0;
    if moved || ((touched | pending) & TOP != 0 && y > 0) {
        need |= TOP;
    }
    if moved || ((touched | pending) & BOTTOM != 0 && below(e, &rc) > 0) {
        need |= BOTTOM;
    }
    if need == 0 {
        return;
    }
    // Scrolled up by `up`, the faded top rows went down by `up`; scrolled down by `down`,
    // the faded bottom rows went up by `down`. (Any mix of the two since stays within
    // these.)
    let (up, down) = match was {
        Some(w) => ((w - y).max(0), (y - w).max(0)),
        None => (rc.bottom, rc.bottom),
    };
    unsafe {
        let rgn = CreateRectRgn(0, 0, 0, 0);
        if need & TOP != 0 {
            let r = CreateRectRgn(rc.left, rc.top, rc.right, (rc.top + band + up).min(rc.bottom));
            CombineRgn(rgn, rgn, r, RGN_OR);
            DeleteObject(r);
        }
        if need & BOTTOM != 0 {
            let r = CreateRectRgn(rc.left, (rc.bottom - band - down).max(rc.top), rc.right, rc.bottom);
            CombineRgn(rgn, rgn, r, RGN_OR);
            DeleteObject(r);
        }
        FRESH.set(need | pending);
        RedrawWindow(e, null_mut(), rgn, RDW_INVALIDATE | RDW_UPDATENOW);
        FRESH.set(0);
        DeleteObject(rgn);
    }
}

/// The client rows of editor `e` (at scale `s`) that other overlays mustn't draw on now:
/// above the first, the faded top band, and from the second, the faded bottom one. A band
/// being drawn fresh this moment (inside our own redraw) or with nothing beyond it isn't
/// kept out, since it's faded after. Drawing there otherwise put full-strength checkbox
/// circles on faded text, as no fade followed it.
pub fn kept_clear(e: HWND, s: f32) -> (i32, i32) {
    let fresh = FRESH.get();
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let band = band(s);
    let top = if fresh & TOP == 0 && rich::scroll_y(e) > 0 { rc.top + band } else { rc.top };
    let bottom = if fresh & BOTTOM == 0 && below(e, &rc) > 0 { rc.bottom - band } else { rc.bottom };
    (top, bottom)
}

pub fn forget(e: HWND) {
    SEEN.with(|m| m.borrow_mut().remove(&(e as isize)));
}

fn band(s: f32) -> i32 {
    (BAND * s).ceil() as i32
}

/// The bands that drawing between client rows `top` and `bottom` of an editor `height`
/// tall, at scale `s`, reaches.
pub fn reach(top: i32, bottom: i32, height: i32, s: f32) -> u8 {
    let mut m = 0;
    if top < band(s) {
        m |= TOP;
    }
    if bottom > height - band(s) {
        m |= BOTTOM;
    }
    m
}

/// How far the text runs on below the editor's bottom edge, in px.
fn below(e: HWND, rc: &RECT) -> i32 {
    rich::content_bottom(e).map_or(0, |b| b - rc.bottom)
}

fn draw(e: HWND, paper: u32, s: f32, which: u8) {
    let band = band(s);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let w = rc.right - rc.left;
    if w <= 0 || rc.bottom - rc.top < 3 * band {
        return;
    }
    // Each grows in over the first pixels of scrolling, so it never pops in.
    let top = if which & TOP != 0 { (rich::scroll_y(e) as f32 / band as f32).min(1.0) } else { 0.0 };
    let bottom = if which & BOTTOM != 0 { (below(e, &rc) as f32 / band as f32).min(1.0) } else { 0.0 };
    if top <= 0.0 && bottom <= 0.0 {
        return;
    }
    CANVAS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().map(|c| (c.w, c.h)) != Some((w, band)) {
            *slot = Some(Canvas::new(w, band));
        }
        let c = slot.as_mut().unwrap();
        unsafe {
            let dc = GetDC(e);
            // RichEdit's DC is its parent's (CS_PARENTDC): keep to the editor.
            IntersectClipRect(dc, rc.left, rc.top, rc.right, rc.bottom);
            HideCaret(e);
            for (strength, y0, edge_first) in [(top, rc.top, true), (bottom, rc.bottom - band, false)] {
                if strength <= 0.0 {
                    continue;
                }
                c.grab(dc, rc.left, y0);
                for k in 0..band {
                    // `t`: 0 at the edge, 1 where the band meets the text.
                    let t = if edge_first { k as f32 + 0.5 } else { (band - k) as f32 - 0.5 } / band as f32;
                    c.fade_rect(rect(0, k, w, 1), paper, 1.0 - strength * (1.0 - t).powf(1.6));
                }
                c.blit(dc, rc.left, y0);
            }
            ShowCaret(e);
            ReleaseDC(e, dc);
        }
    });
}
