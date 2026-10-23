// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows and surfaces: the edge deck, note windows, All Notes, Settings, menus, toasts.

pub mod allnotes;
pub mod deck;
pub mod linkbox;
pub mod ghost;
pub mod menu;
pub mod note;
pub mod settings;
pub mod toast;
pub mod tooltip;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{DT_END_ELLIPSIS, DT_LEFT, DT_WORDBREAK};

use crate::win::render::gfx::{Canvas, Font, Shape};

/// A note's preview body (from `store::body_of`) in `r`, on paper `color`: list markers
/// hang in their own column so wrapped lines line up under the text, nested items step
/// in, checklist items get the editor's circles (ticked ones filled, their text struck
/// and dimmed), and the last line that fits ends in an ellipsis. Returns the height used
/// and whether all of it fitted.
#[allow(clippy::too_many_arguments)]
pub fn draw_body(c: &mut Canvas, body: &str, f: &Font, r: RECT, color: u8, dark: bool, alpha: f32, s: f32) -> (i32, bool) {
    body_pass(Some(c), body, f, r, color, dark, alpha, s)
}

/// Height of `body` laid out by `draw_body` at width `w`.
pub fn body_height(body: &str, f: &Font, w: i32, s: f32) -> i32 {
    body_pass(None, body, f, crate::win::util::rect(0, 0, w, i32::MAX / 2), 0, false, 1.0, s).0
}

#[allow(clippy::too_many_arguments)]
fn body_pass(mut c: Option<&mut Canvas>, body: &str, f: &Font, r: RECT, color: u8, dark: bool, alpha: f32, s: f32) -> (i32, bool) {
    let paper = crate::win::theme::paper(color, dark);
    let ink = crate::win::theme::paper_ink(color, dark);
    let text_rgb = crate::win::theme::paper_subtext(color, dark);
    let done_rgb = crate::win::util::mix(text_rgb, paper, 0.3);
    let struck = f.struck();
    let line_h = (f.ascent + f.descent).max(1);
    let step = (16.0 * s) as i32;
    // Circle size and column, in proportion to the editor's (diameter ~1.45 cap heights).
    let d = (f.cap as f32 * 1.45).round();
    let box_w = (d + 6.0 * s) as i32;
    let mut y = r.top;
    for l in body.lines() {
        // Past the bottom the text is simply cut off; `fade_out` then softens the cut.
        let room = r.bottom - y;
        if room <= 0 {
            return fade_out(c, r, line_h, paper);
        }
        let t = l.trim_start_matches(' ');
        let x = r.left + ((l.len() - t.len()) / 2) as i32 * step;
        let (mark, text) = split_marker(t);
        let task = match mark {
            "○  " => Some(false),
            "●  " => Some(true),
            _ => None,
        };
        let tx = match task {
            Some(_) => x + box_w,
            None if mark.is_empty() => x,
            None => x + f.measure(mark).0,
        };
        let w = r.right - tx;
        if w < step {
            continue;
        }
        let (font, rgb) = if task == Some(true) { (&struck, done_rgb) } else { (f, text_rgb) };
        let h = font.wrapped_height(text, w);
        let Some(c) = c.as_deref_mut() else {
            y += h.max(line_h);
            continue;
        };
        match task {
            // A circle never pokes out below the cut.
            Some(_) if room < line_h => {}
            Some(done) => {
                let (cx, cy) = (x as f32 + d / 2.0, (y + f.ascent) as f32 - f.cap as f32 * 0.5);
                let circle = Shape::new(cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0);
                if done {
                    c.shape(&circle, ink, alpha, 1.0, 0.0);
                    crate::win::editor::checks::check(c, cx, cy, d, 0xFFFFFF, alpha, s);
                } else {
                    c.shape(&circle, crate::win::util::mix(ink, paper, 0.42), alpha, 1.0, 1.3 * s);
                }
            }
            None if !mark.is_empty() => {
                let clip = crate::win::util::rect(x - 2, y - line_h, tx - x + 4, r.bottom - y + line_h);
                c.line(mark, f, x, y + f.ascent, tx - x, Some(clip), text_rgb, alpha, DT_LEFT);
            }
            None => {}
        }
        let fits = h <= room;
        let flags = if fits { DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS } else { DT_LEFT | DT_WORDBREAK };
        if fits {
            c.text(text, font, crate::win::util::rect(tx, y, w, h), rgb, alpha, flags);
        } else {
            // Stop at the cut: glyph slack past it would show the tops of the next line
            // below the fade.
            c.text_above(text, font, crate::win::util::rect(tx, y, w, room), r.bottom, rgb, alpha, flags);
            return fade_out(Some(c), r, line_h, paper);
        }
        y += h.max(line_h);
    }
    (y - r.top, true)
}

/// The body ran past the bottom of `r`: fade its last line and a half into the paper, so
/// the cut reads as "continues" rather than as a sliced line.
fn fade_out(c: Option<&mut Canvas>, r: RECT, line_h: i32, paper: u32) -> (i32, bool) {
    if let Some(c) = c {
        let batching = c.batching();
        c.flush();
        let fade = (line_h * 3 / 2).min(r.bottom - r.top);
        for k in 0..fade {
            let row = crate::win::util::rect(r.left - 2, r.bottom - fade + k, r.right - r.left + 4, 1);
            c.fade_rect(row, paper, 1.0 - (k + 1) as f32 / fade as f32);
        }
        if batching {
            c.begin_batch();
        }
    }
    (r.bottom - r.top, false)
}

/// Split a preview line into its list marker ("•  ", "○  ", "1. ", "a. ") and its text.
fn split_marker(t: &str) -> (&str, &str) {
    for m in ["•  ", "○  ", "●  "] {
        if t.starts_with(m) {
            return t.split_at(m.len());
        }
    }
    let n = t.chars().take_while(|c| c.is_ascii_alphanumeric()).count();
    if (1..=5).contains(&n) && t[n..].starts_with(". ") && (t[..n].chars().all(|c| c.is_ascii_digit()) || t[..n].chars().all(|c| c.is_ascii_lowercase())) {
        return t.split_at(n + 2);
    }
    ("", t)
}
