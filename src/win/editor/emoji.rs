// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Colour emoji over RichEdit.
//!
//! The classic RichEdit draws emoji with GDI, which has no colour fonts, so they come out
//! black and white. After RichEdit draws, each visible emoji is covered with what was
//! under it (paper, or the selection) and drawn again in colour with DirectWrite, at the
//! size RichEdit gives that character. The text itself is untouched.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_SINGLELINE, DT_VCENTER, GetDC, GetPixel, IntersectClipRect, ReleaseDC};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, HideCaret, ShowCaret};

use crate::win::editor::rich;
use crate::win::render::gfx::{Canvas, Font};
use crate::win::util::rect;

const FACE: &str = "Segoe UI Emoji";

thread_local! {
    /// Emoji fonts by pixel size (a note uses two or three sizes).
    static FONTS: RefCell<HashMap<u32, Font>> = RefCell::new(HashMap::new());
}

/// Let go of the emoji fonts (with no note open).
pub fn trim() {
    FONTS.with(|f| f.borrow_mut().clear());
}

/// Draw every visible emoji of editor `e` in colour, straight onto its window.
pub fn paint(e: HWND) {
    let t = rich::text(e);
    let found = crate::common::emoji::find(&t, &[rich::BOX, rich::CHECKED]);
    if found.is_empty() {
        return;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let s = unsafe { GetDpiForWindow(e) }.max(96) as f32 / 96.0;
    unsafe {
        let dc = GetDC(e);
        // RichEdit's DC is its parent's (CS_PARENTDC): keep to the editor.
        IntersectClipRect(dc, rc.left, rc.top, rc.right, rc.bottom);
        HideCaret(e);
        for (a, b) in found {
            let p0 = rich::pos_from_char(e, a as i32);
            if p0.y > rc.bottom || p0.y < rc.top - (80.0 * s) as i32 {
                continue;
            }
            let p1 = rich::pos_from_char(e, b as i32);
            let pt = rich::size_at(e, a as i32).unwrap_or(11.0);
            let px = (pt * 96.0 / 72.0 * s).round();
            // RichEdit's own advance for it, so nothing after it is covered.
            let w = if p1.y == p0.y && p1.x > p0.x { p1.x - p0.x } else { (px * 1.25) as i32 };
            let h = (px * 1.36).ceil() as i32;
            // Paper, or the selection's colour, from the cell's corner.
            let under = GetPixel(dc, p0.x + 1, p0.y + 1);
            if under == u32::MAX {
                continue;
            }
            let bg = crate::win::util::cref(under);
            let mut c = Canvas::new(w.max(1), h.max(1));
            c.clear(0xFF00_0000 | bg);
            c.begin_batch();
            let s16 = String::from_utf16_lossy(&t[a..b]);
            FONTS.with(|fonts| {
                let mut fonts = fonts.borrow_mut();
                let f = fonts.entry(px as u32).or_insert_with(|| Font::new(px, 400, FACE));
                c.text_ct(&s16, f, rect(0, 0, w, h), None, 0, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            });
            c.flush();
            c.make_opaque();
            c.blit(dc, p0.x, p0.y);
        }
        ShowCaret(e);
        ReleaseDC(e, dc);
    }
}
