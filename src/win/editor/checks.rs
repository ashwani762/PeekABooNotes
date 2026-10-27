// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checklist boxes, drawn over RichEdit.
//!
//! The ☐ / ☑ characters stay in the text — they carry the state, the caret, undo and the
//! saved RTF — but are painted in the paper colour. Over what RichEdit draws (`view`), each
//! visible one is covered by an anti-aliased circle in the note's hue, optically centred on
//! the line's capitals. Ticking fills the circle from the centre with a small spring pop
//! and fades a white check in, while the item's text fades to grey and struck through;
//! unticking drains it the same way, the text fading back.

use std::cell::RefCell;
use std::collections::HashMap;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::render::gfx::{Canvas, Image, Shape};
use crate::win::editor::rich;
use crate::win::util::mix;

#[derive(Clone, Copy)]
struct Style {
    ink: u32,
    bg: u32,
    s: f32,
}

/// A ticking item's text as it read before (`keep_look`).
struct Look {
    /// Its rows that showed, the full width of the editor.
    img: Image,
    /// How far below the item's top they start.
    dy: i32,
    /// How tall the whole item was.
    rows: i32,
    /// How far the text has faded to its new look (0 to 1).
    done: f32,
}

thread_local! {
    static STYLES: RefCell<HashMap<isize, Style>> = RefCell::new(HashMap::new());
    /// Boxes mid-animation, by (editor, character position).
    static ANIMS: RefCell<HashMap<(isize, i32), Spring>> = RefCell::new(HashMap::new());
    static HOT: RefCell<Option<(isize, i32)>> = const { RefCell::new(None) };
    /// What ticking items' text fades from, by (editor, character position).
    static LOOKS: RefCell<HashMap<(isize, i32), Look>> = RefCell::new(HashMap::new());
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
    LOOKS.with(|l| l.borrow_mut().retain(|k, _| k.0 != e as isize));
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
        crate::win::editor::view::present(e);
    }
}

/// Step running animations and redraw their editors. Returns true while any move.
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
    // Settled ones (at rest on their target) are drawn once more, then let go.
    let mut editors: Vec<isize> = live.iter().map(|k| k.0).collect();
    editors.sort_unstable();
    editors.dedup();
    for e in editors {
        crate::win::editor::view::present(e as HWND);
    }
    ANIMS.with(|a| {
        let mut a = a.borrow_mut();
        for k in &done {
            a.remove(k);
        }
    });
    LOOKS.with(|l| {
        let mut l = l.borrow_mut();
        for k in &done {
            l.remove(k);
        }
    });
    ANIMS.with(|a| !a.borrow().is_empty())
}

/// The rows of the checklist item whose box is at `cp`, from the top of its first line to
/// where the next paragraph starts (to the end of the text for the last), in client px.
fn item_rows(e: HWND, cp: i32) -> (i32, i32) {
    let t = rich::paras_around(e, cp as usize, cp as usize);
    let end = (t.from + rich::para_end(&t.t, cp - t.from as i32) as usize) as i32;
    let top = rich::pos_from_char(e, cp).y;
    let bottom = if (end as usize) < t.len { rich::pos_from_char(e, end + 1).y } else { rich::content_bottom(e).unwrap_or(top) };
    (top, bottom.max(top))
}

/// The item whose box at `cp` is about to be ticked or unticked, as it reads now: its
/// text fades from this to its new look (struck through and grey, or back) as its circle
/// fills or empties, instead of changing at once.
pub fn keep_look(e: HWND, cp: i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let (top, bottom) = item_rows(e, cp);
    // Kept to what shows (a long item can run off either edge).
    let (a, b) = (top.max(rc.top), bottom.min(rc.bottom));
    LOOKS.with(|l| l.borrow_mut().remove(&(e as isize, cp)));
    if b <= a {
        return;
    }
    if let Some(img) = crate::win::editor::view::look(e, a, b - a) {
        LOOKS.with(|l| l.borrow_mut().insert((e as isize, cp), Look { img, dy: a - top, rows: bottom - top, done: 0.0 }));
    }
}

/// Draw every visible box of editor `e` onto `c`, which holds what RichEdit drew of its
/// client rows from `top` on (`view::compose`), `w` of them wide; each ticking item's old
/// look fading out first. `t` holds the text on screen, `a0..b0` is what shows (only the
/// boxes there: a long checklist has thousands).
pub fn draw(e: HWND, c: &mut Canvas, top: i32, w: i32, t: &rich::Span, a0: usize, b0: usize) {
    let Some(st) = STYLES.with(|m| m.borrow().get(&(e as isize)).copied()) else { return };
    let boxes: Vec<i32> = (a0..b0)
        .filter(|&i| t.get(i).is_some_and(|c| c == rich::BOX || c == rich::CHECKED) && (i == 0 || t.get(i - 1) == Some(rich::CR)))
        .map(|i| i as i32)
        .collect();
    if boxes.is_empty() {
        return;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    let hot = HOT.with(|h| *h.borrow());
    let s = st.s;
    let bottom = (top + c.h).min(rc.bottom);
    let mut items = Vec::with_capacity(boxes.len());
    for cp in boxes {
        let p0 = rich::pos_from_char(e, cp);
        if p0.y > bottom || p0.y < top.max(rc.top) - (40.0 * s) as i32 {
            continue;
        }
        let (text_x, cy, d, gap) = place(e, cp, p0, t.get(cp as usize + 1), s);
        let checked = t.get(cp as usize) == Some(rich::CHECKED);
        let v = ANIMS.with(|a| a.borrow().get(&(e as isize, cp)).map(|sp| sp.value)).unwrap_or(if checked { 1.0 } else { 0.0 });
        // The text's old look over its new one, as much as the circle still has to go.
        LOOKS.with(|l| {
            let mut l = l.borrow_mut();
            let Some(look) = l.get_mut(&(e as isize, cp)) else { return };
            // As far as the circle has come, never back: its spring springs past and back,
            // and the text mustn't flicker with it.
            let k = clamp01(v);
            look.done = look.done.max(if checked { k } else { 1.0 - k });
            let left = 1.0 - look.done;
            let (rt, rb) = item_rows(e, cp);
            // Only while the item is laid out as it was (typing in it changes that).
            if rb - rt == look.rows && left > 0.0 {
                let y = (rt + look.dy - top) as f32;
                c.image(&look.img, 0.0, y, &Shape::new(0.0, y, w.min(look.img.w) as f32, look.img.h as f32, 0.0), left);
            }
        });
        items.push((text_x, cy, d, gap, v, hot == Some((e as isize, cp))));
    }
    for (text_x, cy, d, gap, v, hot) in items {
        // Paper or selection colour, sampled just left of the item's text, before its
        // circle goes on.
        let (x, y) = (text_x - 3, cy as i32 - top);
        let under = c.row(y).get(x.max(0) as usize).filter(|_| x >= 0 && x < rc.right).map_or(st.bg, |p| p & 0xFF_FFFF);
        draw_box(c, 0, top, text_x, cy, d, gap, under, st, v, hot);
    }
}

/// The (invisible) marker glyph's size in points, as set by rich::style_boxes.
const BOX_PT: f32 = 9.0;

/// Where the circle for the box at `cp` goes: (where the item's text starts, the circle's
/// centre height, its diameter, the gap before the text). Drawing and clicking share it.
/// `next`: the character after the box.
fn place(e: HWND, cp: i32, p0: POINT, next: Option<u16>, s: f32) -> (i32, f32, f32, f32) {
    let pt = |p: f32| p * 96.0 / 72.0 * s;
    // Where the item's text starts (after the space, if there is one): the list column.
    let after = if next == Some(b' ' as u16) { cp + 2 } else { cp + 1 };
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

/// Where a point in editor `e` is on the circle of the box at `cp`: (its centre x, y, and
/// how far from it either way), the drawn circle and a little around it, not the
/// invisible glyph under it. `next`: the character after the box.
pub fn circle(e: HWND, cp: i32, next: Option<u16>) -> (f32, f32, f32) {
    let s = STYLES.with(|m| m.borrow().get(&(e as isize)).map(|st| st.s)).unwrap_or(1.0);
    let p0 = rich::pos_from_char(e, cp);
    let (text_x, cy, d, gap) = place(e, cp, p0, next, s);
    (text_x as f32 - gap - d / 2.0, cy, d / 2.0 + 4.0 * s)
}

/// One box onto canvas `c`, whose top-left is at (`ox`, `oy`) in the editor: a disc around
/// the circle cleared to the paper (the last frame's circle goes, while a table's border
/// or a line just above stays as RichEdit drew it), then the circle. In a selection
/// (`under` its colour), the circle is drawn in the selection's text colour, as the
/// item's text is: in the note's ink, an open circle vanished into the blue.
#[allow(clippy::too_many_arguments)]
fn draw_box(c: &mut Canvas, ox: i32, oy: i32, text_x: i32, cy: f32, d: f32, gap: f32, under: u32, st: Style, v: f32, hot: bool) {
    let s = st.s;
    let selected = under == highlight().0;
    let (ink, tick) = if selected { (highlight().1, under) } else { (st.ink, 0xFFFFFF) };
    let k = clamp01(v);
    // A brief pop while the state changes (either way), springing back to size.
    let pop = 1.0 + 0.10 * (std::f32::consts::PI * k).sin() + (v - 1.0).max(0.0) * 0.6;
    let dd = d * pop;
    let cx = text_x as f32 - gap - d / 2.0 - ox as f32;
    let ly = cy - oy as f32;
    let clear = d * 1.16 + 2.0 * s;
    c.shape(&Shape::new(cx - clear / 2.0, ly - clear / 2.0, clear, clear, clear / 2.0), under, 1.0, 1.0, 0.0);
    let circle = Shape::new(cx - dd / 2.0, ly - dd / 2.0, dd, dd, dd / 2.0);
    let ring = if hot { mix(ink, under, 0.15) } else { mix(ink, under, 0.42) };
    c.shape(&circle, ring, 1.0 - k * 0.6, 1.0, 1.5 * s);
    if k > 0.001 {
        // The fill grows from the centre.
        let fd = dd * k;
        c.shape(&Shape::new(cx - fd / 2.0, ly - fd / 2.0, fd, fd, fd / 2.0), ink, 1.0, 1.0, 0.0);
        let ca = clamp01((k - 0.35) / 0.65);
        if ca > 0.0 {
            check(c, cx, ly, dd, tick, ca, s);
        }
    }
}

/// The selection's colour and its text's, as the system has them (RGB).
fn highlight() -> (u32, u32) {
    use windows_sys::Win32::Graphics::Gdi::GetSysColor;
    const COLOR_HIGHLIGHT: i32 = 13;
    const COLOR_HIGHLIGHTTEXT: i32 = 14;
    let rgb = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
    unsafe { (rgb(GetSysColor(COLOR_HIGHLIGHT)), rgb(GetSysColor(COLOR_HIGHLIGHTTEXT))) }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An open circle in a selection is drawn in the selection's text colour, so it shows
    /// on the selection's blue; on the paper, in the note's ink.
    #[test]
    fn a_selected_circle_shows_on_the_selection() {
        let st = Style { ink: 0x404040, bg: 0xF0E6C8, s: 1.0 };
        let (hl, hl_text) = highlight();
        let light = |c: u32| ((c >> 16) & 0xFF) + ((c >> 8) & 0xFF) + (c & 0xFF);
        for (under, want) in [(hl, hl_text), (st.bg, st.ink)] {
            let mut c = Canvas::new(60, 40);
            c.clear(0xFF00_0000 | under);
            // The circle's centre at x = 40 - 6 - 7.5, its left edge 7.5 left of that.
            draw_box(&mut c, 0, 0, 40, 20.0, 15.0, 6.0, under, st, 0.0, false);
            c.make_opaque();
            let ring = c.copy_out(RECT { left: 19, top: 20, right: 20, bottom: 21 })[0] & 0xFF_FFFF;
            let (r, u, w) = (light(ring), light(under), light(want));
            assert!(r.abs_diff(w) < r.abs_diff(u) + 1, "ring {ring:06x} on {under:06x}: towards {want:06x}");
            assert!(r.abs_diff(u) > 60, "ring {ring:06x} stands out of {under:06x}");
        }
    }

    /// A tick's text goes grey and struck through as its circle fills, frame by frame,
    /// from just as it was (it changed in one frame, and the circle followed), and an
    /// untick fades it back the same way.
    #[test]
    fn a_ticked_items_text_fades_as_its_circle_fills() {
        use crate::win::editor::view::{Parts, compose};
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};
        let (w, h) = (360, 200);
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, w, h, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        rich::setup(e);
        rich::insert_text(e, "Tasks\r\u{2610} water the basil\r\u{2610} repot the fern");
        rich::apply_base_format(e, 0x303030, 0xF3E9C6);
        rich::style_boxes(e);
        set_style(e, 0x303030, 0xF3E9C6, 1.0);
        let cp = 6;
        // The item's text, right of its circle.
        let (top, bottom) = item_rows(e, cp);
        let x0 = rich::pos_from_char(e, cp + 2).x;
        let text = || {
            let mut c = Canvas::new(w, h);
            compose(e, &mut c, 0, w, h, Parts::All);
            (top..bottom).flat_map(|y| c.row(y)[x0 as usize..w as usize].to_vec()).collect::<Vec<u32>>()
        };
        let apart = |a: &[u32], b: &[u32]| a.iter().zip(b).map(|(p, q)| (0..3).map(|k| ((p >> (8 * k)) & 0xFF).abs_diff((q >> (8 * k)) & 0xFF)).sum::<u32>() as u64).sum::<u64>();
        for tick in [true, false] {
            let before = text();
            keep_look(e, cp);
            animate(e, cp, tick);
            // As `toggle_check` does it.
            let end = rich::para_end(&rich::text(e), cp);
            rich::still(e, || {
                rich::set_sel(e, cp, cp + 1);
                rich::replace_sel(e, if tick { "\u{2611}" } else { "\u{2610}" });
                rich::set_sel(e, cp + 1, end);
                rich::set_effect(e, rich::CFM_STRIKEOUT, tick);
                rich::set_sel(e, 0, 0);
                rich::style_boxes_in(e, cp, cp);
            });
            let mut frames = vec![text()];
            while frame(1.0 / 60.0) {
                frames.push(text());
            }
            let after = text();
            assert_eq!(apart(&frames[0], &before), 0, "tick {tick}: the first frame is the item as it was");
            assert!(apart(&before, &after) > 10_000, "tick {tick}: it changed");
            let left: Vec<u64> = frames.iter().map(|f| apart(f, &after)).collect();
            assert!(left.windows(2).all(|d| d[1] <= d[0]), "tick {tick}: towards its new look, never back: {left:?}");
            let between = left.iter().filter(|&&d| d > 0 && d < left[0]).count();
            assert!(between >= 4, "tick {tick}: over several frames: {left:?}");
        }
        unsafe { DestroyWindow(e) };
    }
}
