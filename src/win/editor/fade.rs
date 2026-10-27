// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The soft edges of a scrolled note: text scrolling up under the header, or running on
//! past the bottom, melts into the paper instead of being sliced off. Each edge shows only
//! while there's more text beyond it, growing in over the first pixels of scrolling.
//!
//! They're faded on the canvas RichEdit has just drawn the editor on (`view`), after
//! everything else that goes over the text, so what's on screen is always faded once, and
//! whatever lies in a band (a circle, an emoji) fades with its line.

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::win::editor::rich;
use crate::win::render::gfx::Canvas;
use crate::win::util::rect;

/// Height of each fade, in device-independent pixels.
const BAND: f32 = 14.0;

fn band(s: f32) -> i32 {
    (BAND * s).ceil() as i32
}

/// How far the text runs on below the editor's bottom edge, in px.
fn below(e: HWND, rc: &RECT) -> i32 {
    rich::content_bottom(e).map_or(0, |b| b - rc.bottom)
}

/// How strong each edge's fade is (top, bottom), 0 to 1, for a view scrolled `scroll` px
/// down with `below` px of text past its bottom, at band height `band`: each grows in over
/// the first band of scrolling, so it never pops in.
fn strengths(scroll: i32, below: i32, band: i32) -> (f32, f32) {
    let k = |d: i32| (d.max(0) as f32 / band.max(1) as f32).min(1.0);
    (k(scroll), k(below))
}

/// Fade editor `e`'s edges on `c`, which holds its client rows from `top` on.
pub fn draw(e: HWND, c: &mut Canvas, top: i32) {
    let Some((paper, s)) = crate::win::editor::checks::paper(e) else { return };
    let band = band(s);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let w = (rc.right - rc.left).min(c.w);
    if w <= 0 || rc.bottom - rc.top < 3 * band {
        return;
    }
    let (strong_top, strong_bottom) = strengths(rich::scroll_y(e), below(e, &rc), band);
    for (strength, y0, edge_first) in [(strong_top, rc.top, true), (strong_bottom, rc.bottom - band, false)] {
        if strength <= 0.0 {
            continue;
        }
        for k in 0..band {
            // `t`: 0 at the edge, 1 where the band meets the text.
            let t = if edge_first { k as f32 + 0.5 } else { (band - k) as f32 - 0.5 } / band as f32;
            c.fade_rect(rect(0, y0 + k - top, w, 1), paper, 1.0 - strength * (1.0 - t).powf(1.6));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An edge fades in over the first band of scrolling, and holds there.
    #[test]
    fn an_edge_grows_in_with_the_scroll() {
        assert_eq!(strengths(0, 0, 14), (0.0, 0.0));
        assert_eq!(strengths(7, 200, 14), (0.5, 1.0));
        assert_eq!(strengths(300, -3, 14), (1.0, 0.0));
    }
}
