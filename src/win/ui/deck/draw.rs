// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Painting the deck: the pill, the cards and their spines, the + and "+N" buttons.

use super::*;

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
    if !d.visible {
        return;
    }
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
    let dark = app::dark();
    let mat = theme::chrome(dark);
    let mut c = d.canvas.take().unwrap();
    c.clear(0);
    // Keep the tab column hoverable (alpha 1 is invisible but hit-testable).
    let col = ((TAB_W + BULGE + 10.0) * s) as i32;
    if d.wide {
        let (x0, x1) = if g.left { (0, col) } else { (ww - col, ww) };
        c.fill_raw(x0, 0, x1, wh, 0x0100_0000);
    } else {
        c.fill_raw(0, 0, ww, wh, 0x0100_0000);
    }

    // The pill.
    let pa = clamp01(d.pill.value);
    if pa > 0.01 {
        let (px, pw) = span(&g, ww, PILL_INSET * s, (PILL_INSET + PILL_W) * s);
        let pt = g.pill_top() - wy as f32;
        let shape = Shape::new(px, pt, pw, g.pill_h(), pw / 2.0);
        c.soft_shadow(&shape, s, 0.7 * pa);
        c.shape(&shape, mat.fill, mat.alpha * pa, 1.0, 0.0);
        c.shape(&shape, mat.edge, mat.edge_alpha * pa, 1.0, 1.0);
        if d.tabs.is_empty() {
            let cap = Shape::new(px + (pw - CAP_W * s) / 2.0, pt + (PILL_W - CAP_W) / 2.0 * s, CAP_W * s, CAP_H * s, CAP_W * s / 2.0);
            c.shape(&cap, mat.subtext, 0.4 * pa, 1.0, 0.0);
        }
    }

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

    // "+N" tile and the plus button.
    let ea = clamp01(d.extras.value);
    if ea > 0.01 && d.wide {
        let slide = (1.0 - d.extras.value) * 30.0 * s;
        let centre_near = (TAB_W / 2.0 + 2.0) * s - slide;
        let (cx, _) = span(&g, ww, centre_near, centre_near);
        let drop = footer_drop(d);
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
    }

    c.present(d.hwnd, wx, wy);
    d.canvas = Some(c);
    unsafe {
        if IsWindowVisible(d.hwnd) == 0 {
            ShowWindow(d.hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(d.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
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
        // At rest: just the capsule inside the pill.
        let a = clamp01(d.pill.value);
        if d.tabs[i].open {
            c.shape(&cp.shape, col.accent, a, 1.0, 1.6 * s);
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
    let reserve = if open { 16.0 * s } else { 0.0 };
    // Quantise so a card growing/shrinking doesn't re-measure its label every frame.
    let max_len = ((sh.h - 28.0 * s - reserve).max(10.0) as i32 / 8) * 8;
    let (label, len, cell_h) = match &d.tabs[i].fitted {
        Some((m, l, len, ch)) if *m == max_len => (l.clone(), *len, *ch),
        _ => {
            let l = lf.fit(&d.tabs[i].label, max_len);
            let (len, ch) = lf.measure(&l);
            d.tabs[i].fitted = Some((max_len, l.clone(), len, ch));
            (l, len, ch)
        }
    };
    // The hovered tab's label, if it was shortened, scrolls along the spine to show it all.
    let scroll = d.hover == Hit::Tab(i) && d.drag.is_none() && label_a >= 0.999 && label != d.tabs[i].label && !crate::win::render::anim::reduced();
    let (label, len, offset) = if scroll {
        let id = d.tabs[i].id;
        let t0 = match d.marquee {
            Some((m, t0)) if m == id => t0,
            _ => {
                let now = std::time::Instant::now();
                d.marquee = Some((id, now));
                now
            }
        };
        let span = (lf.measure(&d.tabs[i].label).0 - max_len).max(0) as f32;
        let (offset, moving) = marquee_offset(t0.elapsed().as_secs_f32(), span, s);
        d.marquee_live |= moving;
        if moving { (d.tabs[i].label.clone(), max_len, offset) } else { (label, len, 0.0) }
    } else {
        // Off this tab: the next time it's pointed at, its label scrolls again.
        if d.hover != Hit::Tab(i) && d.marquee.is_some_and(|(m, _)| m == d.tabs[i].id) {
            d.marquee = None;
        }
        (label, len, 0.0)
    };
    let spine_cx = cp.spine_x + dir * spine_w / 2.0;
    let (ox, oy) = rot(spine_cx - cell_h as f32 / 2.0, sh.y + 14.0 * s + len as f32 + offset);
    let clip = if scroll {
        rect(sh.x.min(sh.x + sh.w) as i32 - 4, (sh.y + 14.0 * s) as i32, sh.w as i32 + 8, max_len)
    } else {
        rect(sh.x.min(sh.x + sh.w) as i32 - 4, sh.y as i32 - 4, sh.w as i32 + 8, sh.h as i32 + 8)
    };
    // Reads bottom-to-top along the spine: rotated -90 degrees, plus the card's lean.
    let angle = -90.0 + sh.angle.to_degrees();
    c.rotated(&label, &lf, (ox, oy), angle, (ox, oy), clip, ink, 0.9 * label_a);
    if scroll {
        // Soften the edge(s) where the moving label is cut, instead of slicing letters.
        c.flush();
        let span = (sh.y + 14.0 * s) as i32;
        let fade = (12.0 * s) as i32;
        let (x0, xw) = ((spine_cx - cell_h as f32 / 2.0) as i32 - 3, cell_h + 6);
        let cut_end = offset < (lf.measure(&label).0 - max_len) as f32 - 0.5;
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
