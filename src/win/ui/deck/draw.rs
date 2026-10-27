// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Painting the deck: the pill, the cards and their spines, the + and "+N" buttons.

use super::*;
use crate::win::render::gfx::Image;
use crate::win::ui::focus;

pub(super) fn font(d: &mut Deck, face: u8, px: f32, weight: i32) -> Font {
    let s = d.geo.s;
    let key = (px * s * 2.0).round() as i32; // half-pixel buckets
    d.fonts
        .entry((face, key, weight, crate::win::editor::rich::font_index()))
        .or_insert_with(|| {
            // Note text in a pulled-out card (face 2) follows the note typography; chrome keeps the system face.
            let name = match face {
                0 => theme::FACE_SMALL,
                1 => theme::FACE_DISPLAY,
                _ => crate::win::editor::rich::body_face(),
            };
            Font::new(px * s, weight as u32, name)
        })
        .clone()
}

pub(super) fn render(d: &mut Deck) {
    if !d.visible && !d.hiding {
        return;
    }
    let (wx, wy) = paint(d, app::dark());
    if let Some(c) = d.canvas.as_ref() {
        c.present(d.hwnd, wx, wy);
    }
    unsafe {
        if d.visible && IsWindowVisible(d.hwnd) == 0 {
            ShowWindow(d.hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(d.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Draw the deck as it is now on its canvas; returns where the window goes on screen.
pub(super) fn paint(d: &mut Deck, dark: bool) -> (i32, i32) {
    // Only as wide as what is visible: the full width is needed only while a card is out.
    if d.wide {
        let s = d.geo.s;
        let mut need = (TAB_W + BULGE + 40.0) * s;
        for t in &d.tabs {
            if t.pull.value > 0.001 {
                let out = lerp(TAB_W * s + t.bulge.value, (CARD_W + PULL_GAP) * s, t.pull.value);
                need = need.max(out + 28.0 * s);
            }
        }
        let full = ((CARD_W + PULL_GAP + 28.0) * s).round() as i32;
        d.geo.ew = (need.ceil() as i32).min(full);
        d.geo.ex = if d.geo.left { d.geo.work.left } else { d.geo.work.right - d.geo.ew };
    }
    let g = d.geo;
    let s = g.s;
    let (wx, wy, ww, wh) = if d.wide { (g.ex, g.ey, g.ew, g.eh) } else { (g.cx, g.cy, g.cw, g.ch) };
    if d.canvas.as_ref().map(|c| (c.w, c.h)) != Some((ww, wh)) {
        d.canvas = Some(Canvas::new(ww, wh));
    }
    let mat = theme::chrome(dark);
    let ring = ring_hit(d);
    let mut c = d.canvas.take().unwrap();
    c.clear(0);
    // Keep the tab column hoverable (alpha 1 is invisible but hit-testable). Folded, only
    // the pill catches the pointer, with the sliver between it and the screen edge (where a
    // pointer thrown at the edge lands): the rest of the small window lets it through, to
    // the scroll bar of a maximized window, say.
    let col = ((TAB_W + BULGE + 10.0) * s) as i32;
    let pill_zone = {
        let (zx, zw) = span(&g, ww, 0.0, (PILL_INSET + PILL_W + 3.0) * s);
        let top = g.pill_top() - wy as f32 - 3.0 * s;
        rect(zx.floor() as i32, top.floor() as i32, zw.ceil() as i32, (g.pill_h() + 6.0 * s).ceil() as i32)
    };
    if d.wide {
        let (x0, x1) = if g.left { (0, col) } else { (ww - col, ww) };
        c.fill_raw(x0, 0, x1, wh, 0x0100_0000);
    } else {
        c.fill_raw(pill_zone.left, pill_zone.top, pill_zone.right, pill_zone.bottom, 0x0100_0000);
    }

    // The pill, with a shadow close about it (a wide soft one would reach past where it
    // takes the pointer).
    let pa = clamp01(d.pill.value);
    if pa > 0.01 {
        let (px, pw) = span(&g, ww, PILL_INSET * s, (PILL_INSET + PILL_W) * s);
        let n = d.caps.value;
        let pt = g.pill_top_for(n) - wy as f32;
        let empty = d.tabs.is_empty();
        let pill = |c: &mut Canvas, px: f32, pt: f32, a: f32| {
            let shape = Shape::new(px, pt, pw, g.pill_h_for(n), pw / 2.0);
            let mut drop = shape;
            drop.y += 1.0 * s;
            c.shape(&drop, 0x000000, 0.12 * a, 3.0 * s, 0.0);
            c.shape(&shape, mat.fill, mat.alpha * a, 1.0, 0.0);
            c.shape(&shape, mat.edge, mat.edge_alpha * a, 1.0, 1.0);
            if empty {
                let cap = Shape::new(px + (pw - CAP_W * s) / 2.0, pt + (PILL_W - CAP_W) / 2.0 * s, CAP_W * s, CAP_H * s, CAP_W * s / 2.0);
                c.shape(&cap, mat.subtext, 0.4 * a, 1.0, 0.0);
            }
        };
        if d.wide && d.pill.target < 0.5 {
            // Fanning out, the pill dissolves from the top down as the capsules leave it
            // in turn, rather than fading all at once around the ones still waiting.
            let m = (6.0 * s).ceil() as i32;
            let (x0, y0) = (px.floor() as i32 - m, pt.floor() as i32 - m);
            let (w, h) = (pw.ceil() as i32 + 2 * m + 1, g.pill_h_for(n).ceil() as i32 + 2 * m + 1);
            let mut pc = Canvas::new(w, h);
            pc.clear(0);
            pill(&mut pc, px - x0 as f32, pt - y0 as f32, 1.0);
            let mut v = pc.copy_out(rect(0, 0, w, h));
            for (row, line) in v.chunks_mut(w as usize).enumerate() {
                let k = sweep(pa, (row as f32 + y0 as f32 - pt) / g.pill_h_for(n));
                let k = (k * 256.0) as u32;
                for p in line.iter_mut() {
                    let ch = |sh: u32| ((((*p >> sh) & 0xFF) * k) >> 8) << sh;
                    *p = ch(24) | ch(16) | ch(8) | ch(0);
                }
            }
            c.over(&Image::from_pixels(w, h, v), x0, y0);
        } else {
            pill(&mut c, px, pt, pa);
        }
    }

    // The "+N" tile and the plus button come out from under the last card, so they're
    // drawn before the cards.
    draw_extras(d, &mut c, ww, ring, dark);

    // Cards: tucked ones first, then the pulled-out / dragged one on top.
    let mut order: Vec<usize> = (0..d.tabs.len()).collect();
    let drag_idx = d.drag.as_ref().filter(|dr| dr.active).map(|dr| dr.idx);
    order.sort_by(|&a, &b| {
        let ka = (Some(a) == drag_idx, d.tabs[a].pull.value);
        let kb = (Some(b) == drag_idx, d.tabs[b].pull.value);
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    });
    for i in order {
        draw_card(d, &mut c, i, ww, wy as f32, dark);
    }
    // The focus ring, over the cards so the one shingled below never covers its edge. It
    // comes in with the card's spine as the deck fans out.
    if let Some(Hit::Tab(i)) = ring {
        let cp = pose(d, i, ww, wy as f32);
        focus::ring_haloed(&mut c, &cp.shape, theme::FOCUS, mat.fill, clamp01((cp.reveal - 0.55) / 0.45), s);
    }

    if !d.wide {
        // Nothing outside the pill's zone is left hit-testable, not even a shadow's edge.
        let z = pill_zone;
        c.fill_raw(0, 0, ww, z.top, 0);
        c.fill_raw(0, z.bottom, ww, wh, 0);
        c.fill_raw(0, z.top, z.left, z.bottom, 0);
        c.fill_raw(z.right, z.top, ww, z.bottom, 0);
    }
    d.canvas = Some(c);
    (wx, wy)
}

/// How much of a row `y` of the pill (0 at its top, 1 at its foot) is left as it dissolves
/// from the top down, at `pa` (1 whole, 0 gone): a soft front moving down its length.
fn sweep(pa: f32, y: f32) -> f32 {
    const SOFT: f32 = 0.6;
    let front = (1.0 - pa) * (1.0 + SOFT) - SOFT;
    clamp01((y - front) / SOFT)
}

/// The "+N" tile and the plus button, which slide down out from under the last card as
/// the deck fans out, and back under it as it folds.
fn draw_extras(d: &mut Deck, c: &mut Canvas, ww: i32, ring: Option<Hit>, dark: bool) {
    let g = d.geo;
    let s = g.s;
    let mat = theme::chrome(dark);
    let ea = clamp01(d.extras.value);
    if ea > 0.01 && d.wide {
        let centre_near = (TAB_W / 2.0 + 2.0) * s;
        let (cx, _) = span(&g, ww, centre_near, centre_near);
        // From the foot of the stack (its last card's bottom) to the plus button's. They
        // keep below the foot as it moves (the stack folding up into the pill, its last
        // card still on its way down), rather than waiting where they go.
        let rest = g.slot_y(g.n) + 6.0 * s;
        let tuck = (g.plus_cy() + 17.0 * s - rest).max(0.0);
        let foot = (0..d.tabs.len()).filter(|&i| !d.tabs[i].open).map(|i| {
            let sh = pose(d, i, ww, g.ey as f32).shape;
            sh.y + sh.h
        });
        let follow = foot.reduce(f32::max).map_or(0.0, |f| (f - rest).min(0.0));
        let drop = footer_drop(d) + follow - (1.0 - d.extras.value) * tuck;
        if g.more > 0 {
            let (w, h) = ((TAB_W - 10.0) * s, 26.0 * s);
            let sh = Shape::new(cx - w / 2.0, g.more_y() + drop, w, h, h / 2.0);
            let hot = d.more_hot.value;
            c.soft_shadow(&sh, s, 0.6 * ea);
            c.shape(&sh, mat.fill, mat.alpha * ea, 1.0, 0.0);
            c.shape(&sh, mat.edge, mat.edge_alpha * ea, 1.0, 1.0);
            if hot > 0.01 {
                c.shape(&sh, if dark { 0xFFFFFF } else { 0x000000 }, 0.06 * hot * ea, 1.0, 0.0);
            }
            if ring == Some(Hit::More) {
                focus::ring_haloed(c, &sh, theme::FOCUS, mat.fill, ea, s);
            }
            let f = font(d, 0, 11.0, FW_SEMIBOLD as i32);
            let r = rect(sh.x as i32, sh.y as i32, sh.w as i32, sh.h as i32);
            c.text(&t!("deck_more", g.more), &f, r, mat.text, ea, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        let hot = d.plus_hot.value;
        let pressed = if d.pressed == Hit::Plus { 0.92 } else { 1.0 };
        let r = 17.0 * s * (1.0 + 0.08 * hot) * pressed;
        let cy = g.plus_cy() + drop;
        let sh = Shape::new(cx - r, cy - r, 2.0 * r, 2.0 * r, r);
        c.soft_shadow(&sh, s, 0.7 * ea);
        c.shape(&sh, mat.fill, mat.alpha * ea, 1.0, 0.0);
        c.shape(&sh, mat.edge, mat.edge_alpha * ea, 1.0, 1.0);
        let ink = mat.text;
        let (lw, lt) = (11.0 * s, 1.8 * s);
        c.rrect(cx - lw / 2.0, cy - lt / 2.0, lw, lt, lt / 2.0, ink, ea);
        c.rrect(cx - lt / 2.0, cy - lw / 2.0, lt, lw, lt / 2.0, ink, ea);
        if ring == Some(Hit::Plus) {
            focus::ring_haloed(c, &sh, theme::FOCUS, mat.fill, ea, s);
        }
    }
}

/// How far a scrolling spine label has moved after `t` seconds, and whether it's still
/// moving: rest at the start, glide to the end, rest, glide back, once. A pointer resting
/// on the card then costs nothing: the label settles at its start (moving the pointer
/// off the tab and back shows it again).
fn marquee_offset(t: f32, span: f32, s: f32) -> (f32, bool) {
    const REST: f32 = 1.2;
    let travel = (span / (34.0 * s)).max(0.3);
    if t >= 2.0 * (REST + travel) {
        return (0.0, false);
    }
    let ease = |x: f32| x * x * (3.0 - 2.0 * x);
    let f = if t < REST {
        0.0
    } else if t < REST + travel {
        ease((t - REST) / travel)
    } else if t < 2.0 * REST + travel {
        1.0
    } else {
        1.0 - ease((t - 2.0 * REST - travel) / travel)
    };
    (f * span, true)
}

/// The pointed-at card's label, too long for its spine, scrolling along it.
pub(super) struct Marquee {
    pub(super) id: u64,
    since: std::time::Instant,
    /// How much longer the label is than the spine's room.
    span: f32,
    /// What the last frame drew (see `now`).
    pub(super) drawn: i32,
}

impl Marquee {
    /// How far the label is along now, in whole pixels (it's put down on whole pixels),
    /// or -1 while it rests at its start, shortened; and whether it still moves.
    pub(super) fn now(&self, s: f32) -> (i32, bool) {
        let (offset, moving) = marquee_offset(self.since.elapsed().as_secs_f32(), self.span, s);
        (if moving && offset > 0.0 { offset.round() as i32 } else { -1 }, moving)
    }
}

/// A spine label as Direct2D drew it, upright on a clear canvas: its text and ink, the
/// picture, the room left around the text for glyphs that reach past their box, and the
/// text's length.
pub(super) struct Sprite {
    text: String,
    ink: u32,
    img: Image,
    pad: i32,
    len: i32,
}

/// Card `t`'s label `text` (`len` long, `cell_h` across) as a picture, drawn the first time
/// it's wanted and kept: a card shows its shortened label, and while pointed at the whole
/// one, so it keeps two. Drawing text costs a Direct2D pass each time; putting a picture
/// down costs a few thousand pixels.
fn label_sprite<'a>(t: &'a mut Tab, lf: &Font, text: &str, len: i32, cell_h: i32, ink: u32) -> &'a Sprite {
    if let Some(k) = t.sprites.iter().position(|s| s.ink == ink && s.text == text) {
        return &t.sprites[k];
    }
    let pad = 3;
    let mut c = Canvas::new(cell_h.max(lf.ascent + lf.descent) + 2 * pad, len.max(1) + 2 * pad);
    c.clear(0);
    let o = (pad as f32, (pad + len) as f32);
    c.rotated(text, lf, o, -90.0, o, rect(0, 0, c.w, c.h), ink, 1.0);
    let img = c.snapshot(rect(0, 0, c.w, c.h)).unwrap_or_else(|| Image::from_pixels(1, 1, vec![0]));
    if t.sprites.len() >= 2 {
        t.sprites.remove(0);
    }
    t.sprites.push(Sprite { text: text.to_string(), ink, img, pad, len });
    t.sprites.last().unwrap()
}

/// How long card `i`'s spine label may run, pulled out by `pull`: fitted to the card as it
/// is once fanned out, so the label doesn't change as the card grows out of its capsule
/// (the card's edge cuts it meanwhile). Quantised so a card being pulled out doesn't
/// re-measure its label every frame.
fn label_room(d: &Deck, i: usize, pull: f32) -> i32 {
    let s = d.geo.s;
    let reserve = if d.tabs[i].open { 16.0 * s } else { 0.0 };
    let tall = lerp(d.geo.tab_h, preview_height(d, i), pull);
    ((tall - 28.0 * s - reserve).max(10.0) as i32 / 8) * 8
}

/// Card `i`'s label shortened to `max_len`: (text, length, cell height), measured once.
fn fitted(d: &mut Deck, i: usize, lf: &Font, max_len: i32) -> (String, i32, i32) {
    match &d.tabs[i].fitted {
        Some((m, l, len, ch)) if *m == max_len => (l.clone(), *len, *ch),
        _ => {
            let l = lf.fit(&d.tabs[i].label, max_len);
            let (len, ch) = lf.measure(&l);
            d.tabs[i].fitted = Some((max_len, l.clone(), len, ch));
            (l, len, ch)
        }
    }
}

/// Make ahead what the fan-out's first label would make partway through: the spine font
/// (12 ms the first time, while DirectWrite loads the system fonts; 2 ms after a trim) and
/// the text engine (1 ms). Made as that label first showed, with it, they cost one frame
/// 20 ms, and the motion leapt the refreshes it missed. The labels themselves are made as
/// each card shows, a frame apart in the cascade (1 to 6 ms each, with emoji).
pub(super) fn warm(d: &mut Deck) {
    let lf = font(d, 0, 10.5, FW_SEMIBOLD as i32);
    let mut c = Canvas::new(8, 8);
    c.text("A", &lf, rect(0, 0, 8, 8), 0, 1.0, DT_LEFT | DT_SINGLELINE);
}

pub(super) fn draw_card(d: &mut Deck, c: &mut Canvas, i: usize, w: i32, origin_y: f32, dark: bool) {
    c.begin_batch();
    draw_card_inner(d, c, i, w, origin_y, dark);
    c.flush();
}

pub(super) fn draw_card_inner(d: &mut Deck, c: &mut Canvas, i: usize, w: i32, origin_y: f32, dark: bool) {
    let g = d.geo;
    let s = g.s;
    let cp = pose(d, i, w, origin_y);
    let (color, open) = (d.tabs[i].color, d.tabs[i].open);
    let col = theme::color(color);
    let p = clamp01(cp.reveal);
    if p <= 0.003 {
        // At rest: just the capsule inside the pill. One waiting its turn to come out, or
        // just back in, stays solid as the pill fades out or in around it: it would
        // otherwise have faded with the pill and come back whole as its card set off.
        let a = if d.tabs[i].open || !d.wide { clamp01(d.pill.value) } else { 1.0 };
        // One that just came grows in from its middle.
        let b = clamp01(d.tabs[i].born.value);
        let a = a * b;
        let mut cp = cp;
        if b < 0.999 {
            let sh = &mut cp.shape;
            let (cx, cy) = (sh.x + sh.w / 2.0, sh.y + sh.h / 2.0);
            let k = 0.4 + 0.6 * d.tabs[i].born.value.max(0.0);
            (sh.w, sh.h, sh.r) = (sh.w * k, sh.h * k, sh.r * k);
            (sh.x, sh.y) = (cx - sh.w / 2.0, cy - sh.h / 2.0);
        }
        if d.tabs[i].open {
            c.shape(&cp.shape, col.accent, a, 1.0, 1.6 * s);
            if d.tabs[i].pinned {
                // Pinned on top of everything: a pin's head in the hollow capsule.
                let sh = cp.shape;
                let r = 1.4 * s;
                let (x, y) = (sh.x + sh.w / 2.0, sh.y + sh.h / 2.0);
                c.shape(&Shape::new(x - r, y - r, 2.0 * r, 2.0 * r, r), col.accent, a, 1.0, 0.0);
            }
        } else {
            c.shape(&cp.shape, col.accent, a, 1.0, 0.0);
        }
        return;
    }
    let fill = crate::win::util::mix(col.accent, theme::paper(color, dark), clamp01((p - 0.15) / 0.6));
    let lifted = d.drag.as_ref().is_some_and(|dr| dr.active && dr.idx == i);
    let shadow = p * if lifted { 1.8 } else { 1.0 + cp.pull * 0.8 };
    c.card(&cp.shape, fill, 1.0, 0x000000, 0.06 * p, shadow, s);
    if d.hover == Hit::Tab(i) && cp.pull < 0.5 {
        c.shape(&cp.shape, 0xFFFFFF, 0.16 * p, 1.0, 0.0);
    }
    let label_a = clamp01((p - 0.55) / 0.45);
    if label_a <= 0.0 {
        return;
    }
    let sh = cp.shape;
    let ink = theme::paper_ink(color, dark);
    // Spine: perforation, label, open dot. Positions are rotated with the card.
    let spine_w = SPINE_W * s;
    let rot = |x: f32, y: f32| -> (f32, f32) {
        let (sn, cs) = sh.angle.sin_cos();
        let (dx, dy) = (x - sh.px, y - sh.py);
        (sh.px + dx * cs - dy * sn, sh.py + dx * sn + dy * cs)
    };
    let dir = if g.left { -1.0 } else { 1.0 };
    let perf_x = cp.spine_x + dir * spine_w;
    let mut y = sh.y + 12.0 * s;
    while y < sh.y + sh.h - 12.0 * s {
        let len = (4.0 * s).min(sh.y + sh.h - 12.0 * s - y);
        let (x0, y0) = rot(perf_x, y);
        let dash = Shape::new(x0 - 0.5, y0, 1.0, len, 0.0).rotated(sh.angle, x0, y0);
        c.shape(&dash, ink, 0.28 * label_a, 1.0, 0.0);
        y += 8.0 * s;
    }
    let lf = font(d, 0, 10.5, FW_SEMIBOLD as i32);
    let max_len = label_room(d, i, cp.pull);
    let (label, len, cell_h) = fitted(d, i, &lf, max_len);
    // The hovered tab's label, if it was shortened, scrolls along the spine to show it all.
    let scroll = d.hover == Hit::Tab(i) && d.drag.is_none() && label_a >= 0.999 && label != d.tabs[i].label && !crate::win::render::anim::reduced();
    let (label, len, offset, whole) = if scroll {
        let id = d.tabs[i].id;
        let whole_len = lf.measure(&d.tabs[i].label).0;
        let span = (whole_len - max_len).max(0) as f32;
        let m = match d.marquee.take() {
            Some(m) if m.id == id => Marquee { span, ..m },
            _ => Marquee { id, since: std::time::Instant::now(), span, drawn: -1 },
        };
        let (at, moving) = m.now(s);
        d.marquee = Some(Marquee { drawn: at, ..m });
        d.marquee_live |= moving;
        // Resting at its start it reads as it does unpointed at, ending in "…"; the whole
        // label, cut where the spine ends, only once it moves.
        if at >= 0 { (d.tabs[i].label.clone(), whole_len, at as f32, true) } else { (label, len, 0.0, false) }
    } else {
        // Off this tab: the next time it's pointed at, its label scrolls again.
        if d.hover != Hit::Tab(i) && d.marquee.as_ref().is_some_and(|m| m.id == d.tabs[i].id) {
            d.marquee = None;
        }
        (label, len, 0.0, false)
    };
    let spine_cx = cp.spine_x + dir * spine_w / 2.0;
    // While the whole label runs the spine's length, its start sits at the foot of the
    // spine's room and moves down past it by `offset`.
    let foot = sh.y + 14.0 * s + if whole { max_len as f32 } else { len as f32 };
    let (ox, oy) = rot(spine_cx - cell_h as f32 / 2.0, foot + offset);
    let clip = if scroll {
        rect(sh.x.min(sh.x + sh.w) as i32 - 4, (sh.y + 14.0 * s) as i32, sh.w as i32 + 8, max_len)
    } else {
        rect(sh.x.min(sh.x + sh.w) as i32 - 4, sh.y as i32 - 4, sh.w as i32 + 8, sh.h as i32 + 8)
    };
    // Reads bottom-to-top along the spine: turned -90 degrees, plus the card's lean. Drawn
    // once, and put down turned with the card from then on.
    let lab = label_sprite(&mut d.tabs[i], &lf, &label, len, cell_h, ink);
    c.over_turned(&lab.img, (lab.pad as f32, (lab.pad + lab.len) as f32), (ox, oy), sh.angle, clip, 0.9 * label_a);
    if scroll {
        // Soften the edge(s) where the moving label is cut, instead of slicing letters.
        c.flush();
        let span = (sh.y + 14.0 * s) as i32;
        let fade = (12.0 * s) as i32;
        let (x0, xw) = ((spine_cx - cell_h as f32 / 2.0) as i32 - 3, cell_h + 6);
        let cut_end = whole && offset < (len - max_len) as f32 - 0.5;
        for k in 0..fade {
            // Fully paper at the very edge, so nothing of a cut glyph shows there.
            let a = k as f32 / fade as f32;
            if cut_end {
                c.fade_rect(rect(x0, span + k, xw, 1), fill, a);
            }
            if offset > 0.5 {
                c.fade_rect(rect(x0, span + max_len - 1 - k, xw, 1), fill, a);
            }
        }
        c.begin_batch();
    }
    if open {
        let (dx, dy) = rot(spine_cx, sh.y + sh.h - 16.0 * s);
        let r = 3.0 * s;
        c.shape(&Shape::new(dx - r, dy - r, 2.0 * r, 2.0 * r, r), col.accent, label_a, 1.0, 0.0);
    }
    // Card content, revealed as it is pulled out of the deck.
    let ca = clamp01((cp.pull - 0.35) / 0.65);
    if ca > 0.01 {
        let pad = 14.0 * s;
        let (cx0, cx1) = if g.left { (sh.x + pad, cp.spine_x - spine_w - pad) } else { (perf_x + pad, sh.x + sh.w - pad) };
        let tf = font(d, 1, 15.0, FW_SEMIBOLD as i32);
        let bf = font(d, 2, 13.0, FW_NORMAL as i32);
        let t = &d.tabs[i];
        let tr = rect(cx0 as i32, (sh.y + 12.0 * s) as i32, (cx1 - cx0) as i32, (22.0 * s) as i32);
        c.text(&t.title, &tf, tr, theme::PAPER_TEXT, ca, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        if !t.body.is_empty() {
            let top = sh.y + 40.0 * s;
            let br = rect(cx0 as i32, top as i32, (cx1 - cx0) as i32, (sh.y + sh.h - 12.0 * s - top) as i32);
            crate::win::ui::draw_body(c, &t.body, &bf, br, color, dark, ca, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marquee_runs_once_then_rests() {
        let (span, s) = (68.0, 1.0);
        assert_eq!(marquee_offset(0.0, span, s), (0.0, true));
        let (mid, moving) = marquee_offset(2.2, span, s);
        assert!(moving && mid > 0.0 && mid <= span);
        assert_eq!(marquee_offset(3.5, span, s), (span, true));
        // There and back once, then still at the start for as long as the pointer stays.
        assert_eq!(marquee_offset(7.0, span, s), (0.0, false));
        assert_eq!(marquee_offset(600.0, span, s), (0.0, false));
    }
}
