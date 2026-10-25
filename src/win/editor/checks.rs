// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checklist boxes, drawn over RichEdit.
//!
//! The ☐ / ☑ characters stay in the text — they carry the state, the caret, undo and the
//! saved RTF — but are painted in the paper colour. After RichEdit draws, each visible one
//! is covered by an anti-aliased circle in the note's hue, optically centred on the line's
//! capitals. Ticking fills the circle from the centre with a small spring pop and fades
//! a white check in; unticking drains it the same way.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetDC, IntersectClipRect, ReleaseDC};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, HideCaret, ShowCaret};

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::render::gfx::{Canvas, Shape};
use crate::win::editor::rich;
use crate::win::util::mix;

#[derive(Clone, Copy)]
struct Style {
    ink: u32,
    bg: u32,
    s: f32,
}

thread_local! {
    static STYLES: RefCell<HashMap<isize, Style>> = RefCell::new(HashMap::new());
    /// Boxes mid-animation, by (editor, character position).
    static ANIMS: RefCell<HashMap<(isize, i32), Spring>> = RefCell::new(HashMap::new());
    static HOT: RefCell<Option<(isize, i32)>> = const { RefCell::new(None) };
    /// What the boxes are drawn on, kept between paints.
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

/// Colours and scale for an editor's boxes: the note's ink and its paper.
pub fn set_style(e: HWND, ink: u32, bg: u32, s: f32) {
    STYLES.with(|m| m.borrow_mut().insert(e as isize, Style { ink, bg, s }));
}

/// Editor `e`'s paper colour and scale, as set by `set_style`.
pub fn paper(e: HWND) -> Option<(u32, f32)> {
    STYLES.with(|m| m.borrow().get(&(e as isize)).map(|st| (st.bg, st.s)))
}

pub fn forget(e: HWND) {
    STYLES.with(|m| m.borrow_mut().remove(&(e as isize)));
    ANIMS.with(|a| a.borrow_mut().retain(|k, _| k.0 != e as isize));
}

/// Start the tick (or untick) animation for the box at `cp`.
pub fn animate(e: HWND, cp: i32, checked: bool) {
    let from = ANIMS.with(|a| a.borrow().get(&(e as isize, cp)).map(|sp| sp.value)).unwrap_or(if checked { 0.0 } else { 1.0 });
    let mut sp = Spring::unit(from, (0.62, 0.34));
    sp.to(if checked { 1.0 } else { 0.0 });
    ANIMS.with(|a| a.borrow_mut().insert((e as isize, cp), sp));
    anim::wake(Client::Notes);
}

/// The box under the pointer gets a darker ring.
pub fn set_hot(e: HWND, cp: Option<i32>) {
    let key = cp.map(|c| (e as isize, c));
    let changed = HOT.with(|h| {
        let mut h = h.borrow_mut();
        let was = *h;
        *h = key;
        was != key
    });
    if changed {
        paint(e);
    }
}

/// Step running animations and redraw just their boxes. Returns true while any move.
pub fn frame(dt: f32) -> bool {
    let mut done = Vec::new();
    let live: Vec<(isize, i32)> = ANIMS.with(|a| {
        let mut a = a.borrow_mut();
        for (k, sp) in a.iter_mut() {
            if !sp.step(dt) {
                done.push(*k);
            }
        }
        a.keys().copied().collect()
    });
    let mut by_editor: HashMap<isize, ()> = HashMap::new();
    for (e, _) in &live {
        by_editor.insert(*e, ());
    }
    for e in by_editor.keys() {
        paint(*e as HWND);
    }
    ANIMS.with(|a| {
        let mut a = a.borrow_mut();
        for k in &done {
            a.remove(k);
        }
        !a.is_empty()
    })
}

/// Draw every visible box of editor `e` straight onto its window: all of them on one
/// canvas, taken from the window and put back in a single copy. One box at a time, the
/// screen could show RichEdit's fresh text with only some of its circles back (while the
/// wheel glides, rows came into view bare for a frame), and each box cost its own reads
/// of the window.
pub fn paint(e: HWND) {
    let Some(st) = STYLES.with(|m| m.borrow().get(&(e as isize)).copied()) else { return };
    let t = rich::text(e);
    // Only the boxes on screen (a long checklist has thousands).
    let (a0, b0) = rich::on_screen(e, &t);
    let boxes: Vec<i32> = t
        .iter()
        .enumerate()
        .take(b0)
        .skip(a0)
        .filter(|&(i, &c)| (c == rich::BOX || c == rich::CHECKED) && (i == 0 || t[i - 1] == rich::CR))
        .map(|(i, _)| i as i32)
        .collect();
    if boxes.is_empty() {
        return;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let hot = HOT.with(|h| *h.borrow());
    let s = st.s;
    let mut items = Vec::with_capacity(boxes.len());
    // The canvas: from the leftmost circle to just short of the rightmost item's text, and
    // from the top circle to the bottom one.
    let (mut ux0, mut uy0, mut ux1, mut uy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for cp in boxes {
        let p0 = rich::pos_from_char(e, cp);
        if p0.y > rc.bottom || p0.y < rc.top - (40.0 * s) as i32 {
            continue;
        }
        let (text_x, cy, d, gap) = place(e, cp, p0, &t, s);
        let h = (d * 1.2 + 4.0 * s).ceil() as i32;
        let y0 = (cy - h as f32 / 2.0).round() as i32;
        ux0 = ux0.min((text_x as f32 - gap - d - 3.0 * s).floor() as i32);
        ux1 = ux1.max(text_x - 2);
        uy0 = uy0.min(y0);
        uy1 = uy1.max(y0 + h);
        let checked = t[cp as usize] == rich::CHECKED;
        let v = ANIMS.with(|a| a.borrow().get(&(e as isize, cp)).map(|sp| sp.value)).unwrap_or(if checked { 1.0 } else { 0.0 });
        items.push((text_x, cy, d, gap, v, hot == Some((e as isize, cp))));
    }
    let (ux0, uy0, ux1, uy1) = (ux0.max(rc.left), uy0.max(rc.top), ux1.min(rc.right), uy1.min(rc.bottom));
    if items.is_empty() || ux1 <= ux0 || uy1 <= uy0 {
        return;
    }
    let (w, h) = (ux1 - ux0, uy1 - uy0);
    CANVAS.with(|cell| {
        let mut slot = cell.borrow_mut();
        // Kept between paints (a glide paints every frame); grown, never shrunk, so a
        // canvas the size of the column is made once.
        if slot.as_ref().is_none_or(|c| c.w < w || c.h < h) {
            let (cw, ch) = slot.as_ref().map_or((w, h), |c| (c.w.max(w), c.h.max(h)));
            *slot = Some(Canvas::new(cw, ch));
        }
        let c = slot.as_mut().unwrap();
        unsafe {
            let dc = GetDC(e);
            // RichEdit's class hands out its parent's DC (CS_PARENTDC), which isn't clipped to
            // the editor: without this, the circle of a line scrolled up under the header
            // would still be drawn there. The canvas may be larger than this paint needs:
            // only its `w` x `h` corner goes back.
            IntersectClipRect(dc, ux0, uy0, ux1, uy1);
            HideCaret(e);
            c.grab(dc, ux0, uy0);
            // Paper or selection colour, sampled just left of each item's text, before any
            // circle goes on.
            let unders: Vec<u32> = items
                .iter()
                .map(|&(text_x, cy, ..)| {
                    let (x, y) = (text_x - 3 - ux0, cy as i32 - uy0);
                    if x >= 0 && y >= 0 && x < w && y < h {
                        c.copy_out(RECT { left: x, top: y, right: x + 1, bottom: y + 1 }).first().map_or(st.bg, |p| p & 0xFF_FFFF)
                    } else {
                        st.bg
                    }
                })
                .collect();
            for (&(text_x, cy, d, gap, v, hot), &under) in items.iter().zip(&unders) {
                draw_box(c, ux0, uy0, text_x, cy, d, gap, under, st, v, hot);
            }
            c.make_opaque();
            c.blit(dc, ux0, uy0);
            ShowCaret(e);
            ReleaseDC(e, dc);
        }
    });
}

/// The (invisible) marker glyph's size in points, as set by rich::style_boxes.
const BOX_PT: f32 = 9.0;

/// Where the circle for the box at `cp` goes: (where the item's text starts, the circle's
/// centre height, its diameter, the gap before the text). Drawing and clicking share it.
fn place(e: HWND, cp: i32, p0: POINT, t: &[u16], s: f32) -> (i32, f32, f32, f32) {
    let pt = |p: f32| p * 96.0 / 72.0 * s;
    // Where the item's text starts (after the space, if there is one): the list column.
    let after = if t.get(cp as usize + 1) == Some(&(b' ' as u16)) { cp + 2 } else { cp + 1 };
    let p1 = rich::pos_from_char(e, after);
    let d = (pt(rich::body_size() as f32 / 20.0) * 1.04).round();
    let gap = 6.0 * s;
    let text_x = if p1.y == p0.y && p1.x > p0.x { p1.x } else { p0.x + (d + gap) as i32 };
    // Centre on the item's text: the line's own baseline, as RichEdit laid it out (failing
    // that, the first letter's top plus the body ascent; the marker glyph's for an empty
    // item).
    let (sym, body) = fonts(s);
    let baseline = rich::baseline_at(e, cp).map(|b| b as f32).unwrap_or_else(|| if p1.y >= p0.y && p1.y - p0.y < (d * 2.0) as i32 && p1.x > p0.x { p1.y as f32 + body.0 } else { p0.y as f32 + sym.0 });
    // The circle's centre on the text's middle as the eye takes it: between the capitals'
    // middle and the lowercase letters' (most of a line), a little below half the cap
    // height.
    (text_x, baseline - body.1 * 0.42, d, gap)
}

/// Whether point (`x`, `y`) in editor `e` is on the circle of the box at `cp`: the drawn
/// circle and a little around it, not the invisible glyph under it.
pub fn hits(e: HWND, cp: i32, x: i32, y: i32) -> bool {
    let s = STYLES.with(|m| m.borrow().get(&(e as isize)).map(|st| st.s)).unwrap_or(1.0);
    let t = rich::text(e);
    let p0 = rich::pos_from_char(e, cp);
    let (text_x, cy, d, gap) = place(e, cp, p0, &t, s);
    let cx = text_x as f32 - gap - d / 2.0;
    let r = d / 2.0 + 4.0 * s;
    (x as f32 - cx).abs() <= r && (y as f32 - cy).abs() <= r
}

/// One box onto canvas `c`, whose top-left is at (`ox`, `oy`) in the editor: a disc around
/// the circle cleared to the paper (the last frame's circle goes, while a table's border
/// or a line just above stays as RichEdit drew it), then the circle.
#[allow(clippy::too_many_arguments)]
fn draw_box(c: &mut Canvas, ox: i32, oy: i32, text_x: i32, cy: f32, d: f32, gap: f32, under: u32, st: Style, v: f32, hot: bool) {
    let s = st.s;
    let k = clamp01(v);
    // A brief pop while the state changes (either way), springing back to size.
    let pop = 1.0 + 0.10 * (std::f32::consts::PI * k).sin() + (v - 1.0).max(0.0) * 0.6;
    let dd = d * pop;
    let cx = text_x as f32 - gap - d / 2.0 - ox as f32;
    let ly = cy - oy as f32;
    let clear = d * 1.16 + 2.0 * s;
    c.shape(&Shape::new(cx - clear / 2.0, ly - clear / 2.0, clear, clear, clear / 2.0), under, 1.0, 1.0, 0.0);
    let circle = Shape::new(cx - dd / 2.0, ly - dd / 2.0, dd, dd, dd / 2.0);
    let ring = if hot { mix(st.ink, under, 0.15) } else { mix(st.ink, under, 0.42) };
    c.shape(&circle, ring, 1.0 - k * 0.6, 1.0, 1.5 * s);
    if k > 0.001 {
        // The fill grows from the centre.
        let fd = dd * k;
        c.shape(&Shape::new(cx - fd / 2.0, ly - fd / 2.0, fd, fd, fd / 2.0), st.ink, 1.0, 1.0, 0.0);
        let ca = clamp01((k - 0.35) / 0.65);
        if ca > 0.0 {
            check(c, cx, ly, dd, 0xFFFFFF, ca, s);
        }
    }
}

/// A rounded checkmark from two strokes.
pub fn check(c: &mut Canvas, cx: f32, cy: f32, d: f32, rgb: u32, a: f32, s: f32) {
    let sw = (d * 0.12).max(1.4 * s);
    let pts = [(-0.22, 0.02), (-0.06, 0.18), (0.24, -0.14)];
    for i in 0..2 {
        let (ax, ay) = (cx + pts[i].0 * d, cy + pts[i].1 * d);
        let (bx, by) = (cx + pts[i + 1].0 * d, cy + pts[i + 1].1 * d);
        let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
        let len = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt() + sw;
        let leg = Shape::new(mx - len / 2.0, my - sw / 2.0, len, sw, sw / 2.0).rotated((by - ay).atan2(bx - ax), mx, my);
        c.shape(&leg, rgb, a, 1.0, 0.0);
    }
}

type Metrics = ((f32, f32), (f32, f32));

thread_local! {
    static METRICS: RefCell<Option<(f32, i32, &'static str, Metrics)>> = const { RefCell::new(None) };
}

/// (marker ascent, marker cap) and (body ascent, body cap) in px at scale `s`, for the
/// current note typography.
fn fonts(s: f32) -> Metrics {
    let (size, face) = (rich::body_size(), rich::body_face());
    if let Some((ms, mz, mf, m)) = METRICS.with(|m| *m.borrow())
        && ms == s && mz == size && mf == face {
            return m;
        }
    let px = |p: f32| p * 96.0 / 72.0 * s;
    let sym = crate::win::render::dw::Font::new(px(BOX_PT), 400, "Segoe UI Symbol");
    let body = crate::win::render::dw::Font::new(px(size as f32 / 20.0), 400, face);
    let r = ((sym.ascent as f32, sym.cap as f32), (body.ascent as f32, body.cap as f32));
    METRICS.with(|m| *m.borrow_mut() = Some((s, size, face, r)));
    r
}
