// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The soft top edge of a scrolled note: text scrolling up under the header melts into
//! the paper instead of being sliced off.
//!
//! RichEdit draws itself, outside WM_PAINT too, so the fade goes on afterwards, over its
//! pixels. Fading twice would fade more, so the band is always drawn fresh by RichEdit
//! first; and when the view has scrolled since, RichEdit's own scroll has moved faded
//! rows down into the text, so then the whole editor is drawn fresh. When neither
//! happened (typing further down) the band is left as it is: redrawing it on every key
//! showed it unfaded for a moment, a flicker at the top of the note.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetDC, IntersectClipRect, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow, ReleaseDC};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, HideCaret, ShowCaret};

use crate::win::editor::rich;
use crate::win::render::gfx::Canvas;
use crate::win::util::rect;

/// Height of the fade, in device-independent pixels.
const BAND: f32 = 14.0;

thread_local! {
    /// Inside our own redraw: RichEdit has just drawn the band afresh.
    static FRESH: Cell<bool> = const { Cell::new(false) };
    /// Each editor's scroll position when its fade was last drawn.
    static SEEN: RefCell<HashMap<isize, i32>> = RefCell::new(HashMap::new());
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

/// After RichEdit has drawn editor `e` (on `paper`, at scale `s`): fade its top edge.
/// `touched`: what it drew may have reached the band.
pub fn paint(e: HWND, paper: u32, s: f32, touched: bool) {
    if FRESH.get() {
        draw(e, paper, s);
        return;
    }
    let y = rich::scroll_y(e);
    let moved = SEEN.with(|m| m.borrow_mut().insert(e as isize, y)) != Some(y);
    if (y <= 0 && !moved) || (!moved && !touched) {
        return;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let r = if moved { rc } else { RECT { bottom: rc.top + band(s), ..rc } };
    FRESH.set(true);
    unsafe { RedrawWindow(e, &r, null_mut(), RDW_INVALIDATE | RDW_UPDATENOW) };
    FRESH.set(false);
}

pub fn forget(e: HWND) {
    SEEN.with(|m| m.borrow_mut().remove(&(e as isize)));
}

fn band(s: f32) -> i32 {
    (BAND * s).ceil() as i32
}

/// Whether drawing between client rows `top` and `bottom` of an editor at scale `s`
/// reaches the band.
pub fn reaches(top: i32, s: f32) -> bool {
    top < band(s)
}

fn draw(e: HWND, paper: u32, s: f32) {
    let y = rich::scroll_y(e);
    if y <= 0 {
        return;
    }
    let band = band(s);
    // It grows in over the first pixels of scrolling, so it never pops in.
    let strength = (y as f32 / band as f32).min(1.0);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let w = rc.right - rc.left;
    if w <= 0 || rc.bottom - rc.top < band {
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
            c.grab(dc, rc.left, rc.top);
            for k in 0..band {
                let t = (k as f32 + 0.5) / band as f32;
                c.fade_rect(rect(0, k, w, 1), paper, 1.0 - strength * (1.0 - t).powf(1.6));
            }
            c.blit(dc, rc.left, rc.top);
            ShowCaret(e);
            ReleaseDC(e, dc);
        }
    });
}
