// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Colour emoji over RichEdit.
//!
//! The classic RichEdit draws emoji with GDI, which has no colour fonts, so they come out
//! black and white. After RichEdit draws, each visible emoji is covered with what was
//! under it (paper, or the selection) and drawn again in colour with DirectWrite, at the
//! size RichEdit gives that character. The text itself is untouched.
//!
//! All of them go on one canvas taken from the window and put back in one copy: each
//! read of the window waits on the screen (a millisecond under load), and one emoji at a
//! time read it once or twice apiece. Each emoji, once drawn, is kept as a picture
//! (`SPRITES`): DirectWrite's colour glyphs cost over half a millisecond each, and a
//! keystroke or a frame of scrolling draws the same ones again.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_SINGLELINE, DT_VCENTER, GetDC, IntersectClipRect, ReleaseDC};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, HideCaret, ShowCaret};

use crate::win::editor::rich;
use crate::win::render::gfx::{Canvas, Font, Image};
use crate::win::util::rect;

const FACE: &str = "Segoe UI Emoji";

/// An emoji as drawn: its text, its size in px and its cell (w, h).
type SpriteKey = (String, u32, i32, i32);

/// How many drawn emoji are kept (a few KB each): more than a screenful of different ones.
const SPRITES_KEPT: usize = 64;

thread_local! {
    /// Emoji fonts by pixel size (a note uses two or three sizes).
    static FONTS: RefCell<HashMap<u32, Font>> = RefCell::new(HashMap::new());
    /// Emoji drawn on a clear canvas, most recently used last: put over the paper, they
    /// come out exactly as drawn there.
    static SPRITES: RefCell<Vec<(SpriteKey, Image)>> = const { RefCell::new(Vec::new()) };
}

/// Let go of the emoji fonts and pictures (with no note open).
pub fn trim() {
    FONTS.with(|f| f.borrow_mut().clear());
    SPRITES.with(|s| s.borrow_mut().clear());
}

/// Draw every visible emoji of editor `e` in colour, straight onto its window. `s` holds
/// the text on screen, `a0..b0` is what shows (`rich::screen_text`).
pub fn paint_in(e: HWND, s: &rich::Span, a0: usize, b0: usize) {
    let t: Vec<u16> = s.t[a0 - s.from..b0 - s.from].iter().map(|&c| rich::unmark(c)).collect();
    let found = crate::common::emoji::find(&t, &[rich::BOX, rich::CHECKED]);
    if found.is_empty() {
        return;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let sc = unsafe { GetDpiForWindow(e) }.max(96) as f32 / 96.0;
    // Each emoji's cell, and what goes in it.
    let mut cells = Vec::with_capacity(found.len());
    let (mut ux0, mut uy0, mut ux1, mut uy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (a, b) in found {
        let (cp, cq) = ((a + a0) as i32, (b + a0) as i32);
        let p0 = rich::pos_from_char(e, cp);
        if p0.y > rc.bottom || p0.y < rc.top - (80.0 * sc) as i32 {
            continue;
        }
        let p1 = rich::pos_from_char(e, cq);
        let pt = rich::size_at(e, cp).unwrap_or(11.0);
        let px = (pt * 96.0 / 72.0 * sc).round();
        // RichEdit's own advance for it, so nothing after it is covered.
        let w = if p1.y == p0.y && p1.x > p0.x { p1.x - p0.x } else { (px * 1.25) as i32 }.max(1);
        let h = ((px * 1.36).ceil() as i32).max(1);
        ux0 = ux0.min(p0.x);
        uy0 = uy0.min(p0.y);
        ux1 = ux1.max(p0.x + w);
        uy1 = uy1.max(p0.y + h);
        cells.push((p0, w, h, px, String::from_utf16_lossy(&t[a..b])));
    }
    // RichEdit's DC is its parent's (CS_PARENTDC): keep to the editor.
    let (ux0, uy0, ux1, uy1) = (ux0.max(rc.left), uy0.max(rc.top), ux1.min(rc.right), uy1.min(rc.bottom));
    if cells.is_empty() || ux1 <= ux0 || uy1 <= uy0 {
        return;
    }
    let mut c = Canvas::new(ux1 - ux0, uy1 - uy0);
    unsafe {
        let dc = GetDC(e);
        IntersectClipRect(dc, ux0, uy0, ux1, uy1);
        HideCaret(e);
        c.grab(dc, ux0, uy0);
        for (p0, w, h, px, s16) in cells {
            let (x, y) = (p0.x - ux0, p0.y - uy0);
            // Paper, or the selection's colour, from the cell's corner (inside the editor:
            // a line cut by its top edge has its corner above it), as it is now: an emoji
            // just drawn above can reach into this one's cell.
            let (sx, sy) = (p0.x + 1 - ux0, (p0.y + 1).max(rc.top + 1) - uy0);
            if sx < 0 || sy < 0 || sx >= c.w || sy >= c.h {
                continue;
            }
            let Some(&under) = c.copy_out(RECT { left: sx, top: sy, right: sx + 1, bottom: sy + 1 }).first() else { continue };
            c.fill_raw(x, y, x + w, y + h, 0xFF00_0000 | (under & 0xFF_FFFF));
            with_sprite((s16, px as u32, w, h), |img| c.over(img, x, y));
        }
        c.make_opaque();
        c.blit(dc, ux0, uy0);
        ShowCaret(e);
        ReleaseDC(e, dc);
    }
}

/// Run `f` with the emoji `key` as DirectWrite draws it on a clear canvas, drawn now if
/// it isn't kept.
fn with_sprite(key: SpriteKey, f: impl FnOnce(&Image)) {
    SPRITES.with(|sp| {
        let mut sp = sp.borrow_mut();
        let at = match sp.iter().position(|(k, _)| *k == key) {
            Some(i) => {
                // The most recently used go last; the least go first when there's no room.
                let hit = sp.remove(i);
                sp.push(hit);
                sp.len() - 1
            }
            None => {
                let (s16, px, w, h) = &key;
                let mut c = Canvas::new(*w, *h);
                c.clear(0);
                c.begin_batch();
                FONTS.with(|fonts| {
                    let mut fonts = fonts.borrow_mut();
                    let f = fonts.entry(*px).or_insert_with(|| Font::new(*px as f32, 400, FACE));
                    c.text_ct(s16, f, rect(0, 0, *w, *h), None, 0, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                });
                c.flush();
                let Some(img) = c.snapshot(rect(0, 0, *w, *h)) else { return };
                if sp.len() >= SPRITES_KEPT {
                    sp.remove(0);
                }
                sp.push((key, img));
                sp.len() - 1
            }
        };
        f(&sp[at].1);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An emoji kept as a picture and put over the paper comes out exactly as drawing it
    /// there did, on any paper and wherever it goes on the canvas.
    #[test]
    fn a_kept_emoji_comes_out_as_drawn() {
        let flags = DT_CENTER | DT_VCENTER | DT_SINGLELINE;
        for (s16, px, w, h) in [("\u{1F389}", 22u32, 30, 40), ("\u{2764}\u{FE0F}", 30, 38, 41), ("\u{1F44D}\u{1F3FD}", 15, 19, 21)] {
            for bg in [0xFFF8E0u32, 0x202124, 0x3399FF] {
                let mut direct = Canvas::new(w, h);
                direct.clear(0xFF00_0000 | bg);
                direct.begin_batch();
                direct.text_ct(s16, &Font::new(px as f32, 400, FACE), rect(0, 0, w, h), None, 0, flags);
                direct.flush();
                direct.make_opaque();
                let want = direct.copy_out(rect(0, 0, w, h));
                assert!(want.iter().any(|&p| p & 0xFF_FFFF != bg), "something was drawn");
                // Twice: drawn and kept, then from what was kept.
                for _ in 0..2 {
                    let (x, y) = (13, 7);
                    let mut c = Canvas::new(w + 20, h + 20);
                    c.clear(0xFF00_0000 | 0x123456);
                    c.fill_raw(x, y, x + w, y + h, 0xFF00_0000 | bg);
                    with_sprite((s16.to_string(), px, w, h), |img| c.over(img, x, y));
                    c.make_opaque();
                    assert_eq!(c.copy_out(rect(x, y, w, h)), want, "{s16} on {bg:06x}");
                }
            }
        }
        trim();
    }

    /// Emoji typed a character at a time (as keys and the emoji panel put them in), or
    /// pasted: once the note restyles, RichEdit makes each as wide as its colour picture,
    /// so none overlaps the next and none leaves a gap.
    #[test]
    fn emoji_are_laid_out_as_wide_as_their_pictures() {
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, WS_POPUP};
        let s = "emoji \u{1F680}\u{2764}\u{FE0F}\u{1F389}\u{1F44D}\u{1F3FD}\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467} ok";
        for typed in [true, false] {
            let h = unsafe {
                LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
                CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 600, 300, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            rich::setup(h);
            rich::insert_text(h, "T\r");
            rich::apply_base_format(h, 0, 0xFFFFFF);
            let n = rich::text(h).len() as i32;
            rich::set_sel(h, n, n);
            if typed {
                for u in s.encode_utf16() {
                    const WM_CHAR: u32 = 0x0102;
                    rich::send(h, WM_CHAR, rich::marker_for(u).unwrap_or(u) as usize, 0);
                }
            } else {
                rich::replace_sel(h, s);
            }
            rich::hide_marks(h);
            rich::restyle(h);
            let t: Vec<u16> = rich::text(h).iter().map(|&c| rich::unmark(c)).collect();
            let found = crate::common::emoji::find(&t, &[]);
            assert_eq!(found.len(), 5);
            for (a, b) in found {
                let cell = rich::pos_from_char(h, b as i32).x - rich::pos_from_char(h, a as i32).x;
                let px = (rich::size_at(h, a as i32).unwrap() * 96.0 / 72.0).round();
                let s16 = String::from_utf16_lossy(&t[a..b]);
                let picture = Font::new(px, 400, FACE).measure(&s16).0;
                assert!((cell - picture).abs() <= 1, "{s16} typed {typed}: {cell} px laid out for a {picture} px picture");
            }
            // Only the look changed: the text is as typed.
            assert_eq!(rich::unmark_str(&rich::text(h)), format!("T\r{s}"));
            // An emoji in the title keeps its face, and the title its look, restyle after
            // restyle (each would undo the other, a selection moved at every key).
            rich::set_sel(h, 1, 1);
            rich::replace_sel(h, " \u{1F680}");
            for _ in 0..2 {
                rich::restyle(h);
                let cell = rich::pos_from_char(h, 4).x - rich::pos_from_char(h, 2).x;
                let px = (rich::size_at(h, 2).unwrap() * 96.0 / 72.0).round();
                assert!((cell - Font::new(px, 400, FACE).measure("\u{1F680}").0).abs() <= 1, "{cell} at {px}");
                assert!(px > 16.0, "the title's size, not the body's: {px}");
            }
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(h) };
        }
    }
}
