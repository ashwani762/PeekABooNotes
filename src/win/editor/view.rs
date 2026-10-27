// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor on screen in one piece. RichEdit draws its text into a canvas (its text
//! services' `TxDraw`), what goes over the text goes on there (colour emoji, the checklist
//! circles and their ticks, a picked picture's frame, the faded edges), and the window gets
//! it all in one copy, in its WM_PAINT.
//!
//! Drawn straight onto the window, as they were, the overlays went on after RichEdit had
//! drawn, each reading the window back: the screen could show RichEdit's own frame in
//! between, the circles gone, the faded line below at full strength, the emoji in black
//! and white, the caret hidden.
//!
//! Changes made outside a paint (`present`: a key, a restyle, a frame of a tick or of the
//! wheel's glide) are drawn whole and compared with what's on screen a row at a time: only
//! the rows that differ are painted, so a restyle that changed nothing puts nothing on
//! screen.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, EndPaint, GetUpdateRect, IntersectClipRect, InvalidateRect, PAINTSTRUCT, SRCCOPY, SetViewportOrgEx, UpdateWindow,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindow, IsWindowVisible};

use crate::win::editor::{checks, emoji, fade, rich};
use crate::win::render::gfx::{Canvas, Image};

/// What an editor's window shows, as last painted.
struct Shown {
    w: i32,
    h: i32,
    scroll: i32,
    /// Each client row's pixels, hashed.
    rows: Vec<u64>,
}

thread_local! {
    /// What editors are drawn on, kept while anything animates (a glide draws every frame).
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
    /// The editor `present` has just drawn whole on the canvas, for the paint it asks for,
    /// and its rows' hashes.
    static FRESH: Cell<isize> = const { Cell::new(0) };
    static FRESH_ROWS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static SHOWN: RefCell<HashMap<isize, Shown>> = RefCell::new(HashMap::new());
    static PAINTER: Cell<Option<Painter>> = const { Cell::new(None) };
}

/// What the note window draws over its editors (a picked picture's frame): editor, canvas,
/// the client row the canvas starts at.
type Painter = fn(HWND, &mut Canvas, i32);

/// Have `f` draw over every editor, after the circles and before the faded edges.
pub fn set_painter(f: Painter) {
    PAINTER.set(Some(f));
}

/// What a drawing of an editor takes in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Parts {
    /// All of it.
    All,
    /// The text as it reads (in colour emoji): what a checklist item's tick fades from.
    Text,
}

/// Draw client rows `top..top + rows` of editor `e`, `w` wide, onto `c` (its row 0 at
/// `top`): RichEdit's text, then what goes over it. False when RichEdit can't draw there.
pub fn compose(e: HWND, c: &mut Canvas, top: i32, w: i32, rows: i32, parts: Parts) -> bool {
    use windows::Win32::Graphics::Gdi::HDC;
    use windows::Win32::System::Com::DVASPECT_CONTENT;
    /// The view a windowed editor shows (TXTVIEW_ACTIVE): its own scroll and selection.
    const ACTIVE_VIEW: i32 = 0;
    let Some(ts) = rich::services(e) else { return false };
    let mut upd = windows::Win32::Foundation::RECT { left: 0, top, right: w, bottom: top + rows };
    let dc = c.dc();
    let drawn = unsafe {
        SetViewportOrgEx(dc, 0, -top, null_mut());
        // No bounds: the editor's own client area, as it lays itself out.
        let r = ts.TxDraw(DVASPECT_CONTENT, 0, null_mut(), null_mut(), HDC(dc as _), HDC(null_mut()), null_mut(), null_mut(), &mut upd, 0, 0, ACTIVE_VIEW);
        SetViewportOrgEx(dc, 0, 0, null_mut());
        r.is_ok()
    };
    if !drawn {
        return false;
    }
    c.make_opaque_rect(RECT { left: 0, top: 0, right: w, bottom: rows });
    let (t, a0, b0) = rich::screen_text(e);
    emoji::draw(e, c, top, &t, a0, b0);
    if parts == Parts::All {
        checks::draw(e, c, top, w, &t, a0, b0);
        if let Some(f) = PAINTER.get() {
            f(e, c, top);
        }
        fade::draw(e, c, top);
    }
    true
}

/// Client rows `top..top + rows` of editor `e` as their text reads now (`Parts::Text`).
pub fn look(e: HWND, top: i32, rows: i32) -> Option<Image> {
    let w = client(e).0;
    if w <= 0 || rows <= 0 {
        return None;
    }
    let mut c = Canvas::new(w, rows);
    compose(e, &mut c, top, w, rows, Parts::Text).then_some(())?;
    c.snapshot(RECT { left: 0, top: 0, right: w, bottom: rows })
}

fn client(e: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    (rc.right, rc.bottom)
}

/// A row's pixels, hashed.
fn hash(px: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in px {
        h = (h ^ *p as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The canvas, at least `w` x `h`, for `f`.
fn with_canvas<R>(w: i32, h: i32, f: impl FnOnce(&mut Canvas) -> R) -> R {
    CANVAS.with(|cell| {
        let mut slot = cell.borrow_mut();
        // Grown, never shrunk: a canvas the size of the note is made once.
        if slot.as_ref().is_none_or(|c| c.w < w || c.h < h) {
            let (cw, ch) = slot.as_ref().map_or((w, h), |c| (c.w.max(w), c.h.max(h)));
            *slot = Some(Canvas::new(cw, ch));
        }
        f(slot.as_mut().unwrap())
    })
}

/// Editor `e`'s WM_PAINT: all of it drawn on the canvas, the part asked for copied to the
/// window. False when RichEdit can't draw it that way (it paints itself then).
pub fn on_paint(e: HWND) -> bool {
    let (w, h) = client(e);
    if rich::services(e).is_none() {
        return false;
    }
    let scroll = rich::scroll_y(e);
    // Moved or resized since it was last painted (RichEdit moves what's on screen itself
    // when it scrolls outside `quiet`): all of it. Otherwise whole rows, so each row on
    // screen is known to be one drawn here.
    let moved = SHOWN.with(|m| m.borrow().get(&(e as isize)).is_none_or(|s| s.w != w || s.h != h || s.scroll != scroll));
    unsafe {
        let mut u: RECT = std::mem::zeroed();
        if moved {
            InvalidateRect(e, std::ptr::null(), 0);
        } else if GetUpdateRect(e, &mut u, 0) != 0 {
            InvalidateRect(e, &RECT { left: 0, top: u.top, right: w, bottom: u.bottom }, 0);
        }
    }
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc = unsafe { BeginPaint(e, &mut ps) };
    let (y0, y1) = (ps.rcPaint.top.max(0), ps.rcPaint.bottom.min(h));
    if w > 0 && y1 > y0 {
        let fresh = FRESH.replace(0) == e as isize;
        let mut rows = if fresh { FRESH_ROWS.with(|r| std::mem::take(&mut *r.borrow_mut())) } else { Vec::new() };
        with_canvas(w, h, |c| {
            // Drawn whole even for a part: what goes over the text looks around it (the
            // paper or selection under a circle).
            if !fresh && compose(e, c, 0, w, h, Parts::All) {
                rows = (0..h).map(|y| hash(&c.row(y)[..w as usize])).collect();
            }
            if rows.len() == h as usize {
                unsafe {
                    // RichEdit's class hands out its parent's DC (CS_PARENTDC): keep to the
                    // editor.
                    IntersectClipRect(dc, 0, 0, w, h);
                    BitBlt(dc, 0, y0, w, y1 - y0, c.dc(), 0, y0, SRCCOPY);
                }
            }
        });
        SHOWN.with(|m| {
            let mut m = m.borrow_mut();
            let s = m.entry(e as isize).or_insert(Shown { w, h, scroll, rows: Vec::new() });
            if moved || s.rows.len() != h as usize {
                *s = Shown { w, h, scroll, rows: vec![0; h as usize] };
            }
            for y in y0..y1 {
                if let Some(r) = rows.get(y as usize) {
                    s.rows[y as usize] = *r;
                }
            }
        });
    }
    unsafe { EndPaint(e, &ps) };
    if !crate::win::render::anim::active() {
        CANVAS.with(|c| c.borrow_mut().take());
    }
    true
}

/// Put what changed in editor `e` on screen now, in one paint: it's drawn whole, and the
/// rows that differ from what the window shows are painted. Inside `rich::quiet` it waits
/// for that to finish.
pub fn present(e: HWND) {
    if unsafe { IsWindow(e) == 0 || IsWindowVisible(e) == 0 } || rich::is_quiet(e) {
        return;
    }
    let (w, h) = (client(e).0, client(e).1);
    if w <= 0 || h <= 0 {
        return;
    }
    let scroll = rich::scroll_y(e);
    let Some(rows) = with_canvas(w, h, |c| compose(e, c, 0, w, h, Parts::All).then(|| (0..h).map(|y| hash(&c.row(y)[..w as usize])).collect::<Vec<u64>>())) else {
        // RichEdit paints itself.
        unsafe { InvalidateRect(e, std::ptr::null(), 0) };
        return;
    };
    let changed = SHOWN.with(|m| {
        let m = m.borrow();
        let shown = m.get(&(e as isize)).filter(|s| s.w == w && s.h == h && s.scroll == scroll);
        changed_rows(shown.map(|s| &s.rows[..]), &rows)
    });
    if changed.is_empty() {
        return;
    }
    FRESH.set(e as isize);
    FRESH_ROWS.with(|r| *r.borrow_mut() = rows);
    unsafe {
        for (a, b) in changed {
            InvalidateRect(e, &RECT { left: 0, top: a, right: w, bottom: b }, 0);
        }
        UpdateWindow(e);
    }
    FRESH.set(0);
}

/// The runs of rows (from, to) whose hashes `rows` differ from those `shown` (all of
/// them when what's shown isn't known, or is another size).
fn changed_rows(shown: Option<&[u64]>, rows: &[u64]) -> Vec<(i32, i32)> {
    let shown = shown.filter(|s| s.len() == rows.len());
    let differs = |y: usize| shown.is_none_or(|s| s[y] != rows[y]);
    let mut runs = Vec::new();
    let mut y = 0;
    while y < rows.len() {
        if !differs(y) {
            y += 1;
            continue;
        }
        let a = y;
        while y < rows.len() && differs(y) {
            y += 1;
        }
        runs.push((a as i32, y as i32));
    }
    runs
}

/// Editor `e` is going.
pub fn forget(e: HWND) {
    SHOWN.with(|m| m.borrow_mut().remove(&(e as isize)));
}

/// Let the canvas go (with no note open).
pub fn trim() {
    CANVAS.with(|c| c.borrow_mut().take());
    SHOWN.with(|m| m.borrow_mut().retain(|&e, _| unsafe { IsWindow(e as HWND) } != 0));
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};

    const PAPER: u32 = 0xF3E9C6;

    /// A checklist note, the title with an emoji and every item with one too.
    fn checklist(w: i32, h: i32) -> HWND {
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, w, h, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        let mut s = String::from("Garden \u{1F331}");
        for i in 1..=20 {
            s.push_str(&format!("\r\u{2610} item number {i} \u{1F680}"));
        }
        rich::insert_text(e, &s);
        rich::apply_base_format(e, 0x303030, PAPER);
        rich::style_boxes(e);
        rich::restyle(e);
        checks::set_style(e, 0x303030, PAPER, 1.0);
        e
    }

    fn drawn(e: HWND, top: i32, w: i32, rows: i32) -> Vec<u32> {
        let mut c = Canvas::new(w, rows);
        assert!(compose(e, &mut c, top, w, rows, Parts::All));
        c.copy_out(RECT { left: 0, top: 0, right: w, bottom: rows })
    }

    /// `drawn`, without the circles and the faded edges (the editor's look let go).
    fn bare(e: HWND, w: i32, h: i32) -> Vec<u32> {
        checks::forget(e);
        let px = drawn(e, 0, w, h);
        checks::set_style(e, 0x303030, PAPER, 1.0);
        px
    }

    /// How far rows `a..b` of `px` (`w` wide) are from the paper, on average.
    fn ink(px: &[u32], w: i32, a: i32, b: i32) -> f32 {
        let rows = &px[(a * w) as usize..(b * w) as usize];
        let d = |p: u32| (0..3).map(|k| ((p >> (8 * k)) & 0xFF).abs_diff((PAPER >> (8 * k)) & 0xFF)).sum::<u32>();
        rows.iter().map(|&p| d(p) as f32).sum::<f32>() / rows.len() as f32
    }

    /// Every frame of the editor has all that goes over its text, drawn with it before it
    /// reaches the screen: each checklist circle, the emoji in colour, both edges faded.
    /// Drawn on the window after RichEdit's own frame, they could miss a frame (circles
    /// gone, emoji black, the faded line at full strength).
    #[test]
    fn a_frame_has_its_circles_emoji_and_faded_edges() {
        let (w, h) = (360, 260);
        let e = checklist(w, h);
        rich::set_scroll_y(e, 60);
        let all = drawn(e, 0, w, h);
        let text = bare(e, w, h);
        // A circle on every item wholly in view (its box is drawn in the paper colour).
        let t = rich::text(e);
        let mut circles = 0;
        for (i, _) in t.iter().enumerate().filter(|&(i, &c)| c == rich::BOX && (i == 0 || t[i - 1] == rich::CR)) {
            let (cx, cy, r) = checks::circle(e, i as i32, t.get(i + 1).copied());
            if cy - r < 20.0 || cy + r > (h - 20) as f32 {
                continue;
            }
            let ring = (cx - (r - 4.0)).round() as i32;
            let (x0, x1) = (ring - 1, ring + 2);
            let row = cy.round() as i32;
            let dark = (x0..x1).any(|x| {
                let (a, b) = (all[(row * w + x) as usize], text[(row * w + x) as usize]);
                ink(&[a], 1, 0, 1) > ink(&[b], 1, 0, 1) + 60.0
            });
            assert!(dark, "item at {i}: a circle at {cx},{cy}");
            circles += 1;
        }
        assert!(circles >= 6, "{circles} items checked");
        // The emoji in colour: some of their pixels far from grey.
        let colourful = all.iter().filter(|&&p| {
            let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            r.max(g).max(b) - r.min(g).min(b) > 90
        });
        assert!(colourful.count() > 100);
        // The edges fade into the paper: text on their outermost rows all but gone. (Scrolled
        // so that a line runs across each.)
        let edge = |all: &[u32], text: &[u32], a: i32, b: i32| (ink(all, w, a, b), ink(text, w, a, b));
        let (top, bottom) = (60..90)
            .map(|y| {
                rich::set_scroll_y(e, y);
                let (all, text) = (drawn(e, 0, w, h), bare(e, w, h));
                (edge(&all, &text, 0, 4), edge(&all, &text, h - 4, h))
            })
            .fold(((0.0, 0.0), (0.0, 0.0)), |(t, b), (t2, b2)| (if t2.1 > t.1 { t2 } else { t }, if b2.1 > b.1 { b2 } else { b }));
        assert!(top.1 > 20.0 && top.0 < top.1 * 0.3, "top: {top:?}");
        assert!(bottom.1 > 20.0 && bottom.0 < bottom.1 * 0.3, "bottom: {bottom:?}");
        // Drawn a band at a time (a paint of part of it), each row comes out the same.
        let part = drawn(e, 0, w, h);
        for top in [40, 97, 150] {
            let rows = drawn(e, top, w, 50);
            assert_eq!(rows, part[(top * w) as usize..((top + 50) * w) as usize], "rows {top}..");
        }
        unsafe { DestroyWindow(e) };
    }

    /// What a change puts on screen is the rows that differ from what it shows.
    #[test]
    fn only_the_rows_that_changed_are_shown_again() {
        let shown = [1u64, 2, 3, 4, 5, 6];
        assert_eq!(changed_rows(Some(&shown), &shown), vec![]);
        assert_eq!(changed_rows(Some(&shown), &[1, 9, 9, 4, 5, 7]), vec![(1, 3), (5, 6)]);
        assert_eq!(changed_rows(Some(&shown), &[0, 2, 3, 4, 5, 6]), vec![(0, 1)]);
        // Not known, or another size: all of it.
        assert_eq!(changed_rows(None, &shown), vec![(0, 6)]);
        assert_eq!(changed_rows(Some(&shown[..3]), &shown), vec![(0, 6)]);
    }
}
