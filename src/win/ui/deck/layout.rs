// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deck geometry and motion: where every card is, how it moves, what the pointer is over.

use super::*;

pub(super) fn compute_geo(n_items: usize) -> Geo {
    let (left, pos, max_tabs) = app::store(|s| (s.d.settings.edge_left, s.d.settings.deck_pos, s.d.settings.deck_max.clamp(3, 16) as usize));
    let (work, s) = monitor_geo();
    let wh = (work.bottom - work.top) as f32;
    let mut g = Geo { s, left, work, ..Default::default() };
    let gap = -6.0 * s; // shingled: each card tucks slightly under the next
    // Compact cards keep the fanned-out stack centred on the pill instead of filling the
    // whole screen height.
    let (min_h, max_h) = (96.0 * s, 124.0 * s);
    let fixed = 2.0 * 14.0 * s + 34.0 * s + 40.0 * s + 36.0 * s;
    let room = wh - fixed;
    let fit = ((room / (min_h + gap)).floor() as usize).max(1);
    g.n = n_items.min(max_tabs).min(fit);
    g.more = n_items - g.n;
    g.tab_h = if g.n == 0 { min_h } else { ((room - if g.more > 0 { 36.0 * s } else { 0.0 }) / g.n as f32 - gap).clamp(min_h, max_h) };
    g.step = g.tab_h + gap;
    g.top = 14.0 * s;
    g.tail = if g.more > 0 { 36.0 * s } else { 0.0 } + 8.0 * s + 34.0 * s + 16.0 * s;
    let stack = g.top + g.n as f32 * g.step - gap + g.tail;
    g.centre = work.top + (pos.clamp(0.05, 0.95) * wh) as i32;
    // With only a few cards the stack is short, but a pulled-out card can be as tall as
    // its preview: the window always has room for the tallest one with the plus button
    // below it (see `footer_drop`), or it gets cut off.
    g.eh = stack.max(8.0 * s + PREVIEW_MAX * s + gap + g.tail).min(wh).round() as i32;
    g.ew = ((CARD_W + PULL_GAP + 28.0) * s).round() as i32;
    g.ey = (g.centre - g.eh / 2).clamp(work.top, (work.bottom - g.eh).max(work.top));
    // The stack stays centred on the pill within the taller window.
    g.top += ((g.centre - g.ey) as f32 - stack / 2.0).clamp(0.0, (g.eh as f32 - stack).max(0.0));
    g.ex = if left { work.left } else { work.right - g.ew };
    g.cw = ((PILL_W + PILL_INSET + 12.0) * s).round() as i32;
    g.ch = (g.pill_h() + 36.0 * s).round() as i32;
    g.cy = (g.centre - g.ch / 2).clamp(work.top, (work.bottom - g.ch).max(work.top));
    g.cx = if left { work.left } else { work.right - g.cw };
    g
}

/// Rebuild the tab list from the store, keeping the springs of tabs that still exist.
pub(super) fn sync(d: &mut Deck) {
    let rows: Vec<(u64, String, String, u8)> = app::store(|s| {
        s.active_ids().iter().filter_map(|id| s.note(*id)).filter(|n| !n.off_deck).map(|n| (n.id, title_of(&n.text), body_of(&n.text), n.color)).collect()
    });
    let open: Vec<u64> = app::ui(|u| u.notes.keys().copied().collect());
    let in_deck = rows.iter().filter(|r| !open.contains(&r.0)).count();
    let mut g = compute_geo(in_deck);
    let mut old: HashMap<u64, Tab> = d.tabs.drain(..).map(|t| (t.id, t)).collect();
    let mut slot = 0usize;
    for (id, title, body, color) in rows.into_iter() {
        let is_open = open.contains(&id);
        // Only cards in the deck count toward its size: open notes take no slot (they
        // keep a hollow capsule in the pill), so they must never crowd real cards out.
        if !is_open && slot >= g.n {
            continue;
        }
        let i = slot;
        let mut t = old.remove(&id).unwrap_or_else(|| {
            let mut t = Tab::new(id, g.slot_y(i));
            if d.open && !is_open {
                t.reveal.snap(1.0);
                t.reveal_goal = 1.0;
            }
            t
        });
        t.slot = slot;
        if !is_open {
            slot += 1;
        }
        let label = title.to_uppercase();
        if label != t.label {
            t.fitted = None;
        }
        t.label = label;
        t.title = title;
        t.body = body;
        t.color = color;
        if is_open != t.open {
            t.open = is_open;
            // Out: the ghost carried the card away. Back: it slides in from its capsule.
            t.reveal_goal = if is_open || !d.open { 0.0 } else { 1.0 };
            if is_open {
                t.reveal.snap(0.0);
                t.pull.snap(0.0);
            }
            t.delay = 0.0;
        }
        if d.drag.as_ref().is_none_or(|dr| !dr.active) {
            t.y.to(g.slot_y(i));
        }
        d.tabs.push(t);
    }
    g.caps = d.tabs.len();
    g.ch = (g.pill_h() + 36.0 * g.s).round() as i32;
    g.cy = (g.centre - g.ch / 2).clamp(g.work.top, (g.work.bottom - g.ch).max(g.work.top));
    d.geo = g;
    if d.font_scale != g.s {
        d.fonts.clear();
        d.font_scale = g.s;
        for t in d.tabs.iter_mut() {
            t.fitted = None;
        }
    }
    // Text, scale or typography may have changed: measure again when next pulled out,
    // and right away for a card that is out now.
    for t in d.tabs.iter_mut() {
        t.body_h = None;
    }
    for i in 0..d.tabs.len() {
        if d.tabs[i].pull.target > 0.0 || d.tabs[i].pull.value > 0.0 {
            measure_body(d, i);
        }
    }
    anim::wake(Client::Deck);
}

/// Measure card `i`'s preview (not estimate it), so the card is exactly as tall as its
/// text, up to PREVIEW_MAX, past which the text fades out. Done only when a card is about
/// to be pulled out: laying text out loads DirectWrite, which an untouched deck never needs.
pub(super) fn measure_body(d: &mut Deck, i: usize) {
    if d.tabs.get(i).is_none_or(|t| t.body_h.is_some()) {
        return;
    }
    let s = d.geo.s;
    let bf = super::draw::font(d, 2, 13.0, FW_NORMAL as i32);
    let w = ((CARD_W - SPINE_W - 2.0 * 14.0) * s) as i32;
    let t = &mut d.tabs[i];
    t.body_h = Some(if t.body.is_empty() { 0.0 } else { crate::win::ui::body_height(&t.body, &bf, w, s) as f32 });
}

pub(super) fn set_open(d: &mut Deck, open: bool, instant: bool) {
    if d.open == open && !instant {
        return;
    }
    d.open = open;
    let n = d.tabs.len();
    if open {
        d.wide = true;
    } else {
        d.previewing = false;
    }
    let reopening = open && d.tabs.iter().any(|t| t.reveal.value > 0.05);
    for t in d.tabs.iter_mut() {
        let i = t.slot;
        t.reveal_goal = if open && !t.open { 1.0 } else { 0.0 };
        t.delay = if instant || reopening {
            0.0
        } else if open {
            i as f32 * motion::STAGGER_S
        } else {
            (n.saturating_sub(1 + i)) as f32 * motion::STAGGER_S * 0.45
        };
        if !open {
            t.pull.to(0.0);
            t.bulge.to(0.0);
            t.lean.to(LEAN);
        }
        if instant {
            t.reveal.snap(t.reveal_goal);
        }
    }
    d.extras_goal = if open { 1.0 } else { 0.0 };
    d.extras_delay = if open && !instant && !reopening { n as f32 * motion::STAGGER_S } else { 0.0 };
    d.pill.to(if open { 0.0 } else { 1.0 });
    if instant {
        d.extras.snap(d.extras_goal);
        d.pill.snap(if open { 0.0 } else { 1.0 });
        if !open {
            d.wide = false;
        }
    }
    anim::wake(Client::Deck);
}

pub(super) fn step(d: &mut Deck, dt: f32) -> bool {
    let mut moving = false;
    let s = d.geo.s;
    let hover = d.hover;
    let drag_idx = d.drag.as_ref().filter(|dr| dr.active).map(|dr| dr.idx);
    for (i, t) in d.tabs.iter_mut().enumerate() {
        if t.delay > 0.0 {
            t.delay -= dt;
            moving = true;
        }
        if t.delay <= 0.0 && t.reveal.target != t.reveal_goal {
            t.reveal.to(t.reveal_goal);
        }
        if d.open {
            let hot = hover == Hit::Tab(i) || drag_idx == Some(i);
            let near = matches!(hover, Hit::Tab(h) if h.abs_diff(i) == 1) && t.pull.target < 0.5;
            t.bulge.to(if hot { BULGE * s } else if near { 4.0 * s } else { 0.0 });
            t.lean.to(if hot || t.pull.target > 0.5 { 0.0 } else { LEAN });
        }
        for sp in t.springs() {
            moving |= sp.step(dt);
        }
    }
    if d.extras_delay > 0.0 {
        d.extras_delay -= dt;
        moving = true;
    }
    if d.extras_delay <= 0.0 {
        d.extras.to(d.extras_goal);
    }
    d.plus_hot.to(if d.hover == Hit::Plus { 1.0 } else { 0.0 });
    d.more_hot.to(if d.hover == Hit::More { 1.0 } else { 0.0 });
    for sp in [&mut d.pill, &mut d.extras, &mut d.plus_hot, &mut d.more_hot] {
        moving |= sp.step(dt);
    }
    moving
}

/// Map an edge-space horizontal span (distance from the screen edge, `near` < `far`) to
/// window-local x and width.
pub(super) fn span(g: &Geo, w: i32, near: f32, far: f32) -> (f32, f32) {
    if g.left { (near, far - near) } else { (w as f32 - far, far - near) }
}

pub(super) fn pose(d: &Deck, i: usize, w: i32, origin_y: f32) -> CardPose {
    let g = &d.geo;
    let s = g.s;
    let t = &d.tabs[i];
    let p = t.reveal.value.max(0.0);
    let pull = clamp01(t.pull.value);
    // Spine target: inner edge `out` from the screen edge, card spans [out - CARD_W, out].
    let tucked_out = TAB_W * s + t.bulge.value;
    let pulled_out = (CARD_W + PULL_GAP) * s;
    let out = lerp(tucked_out, pulled_out, t.pull.value);
    let card_h = lerp(g.tab_h, preview_height(d, i), pull);
    let mut top = t.y.value;
    if pull > 0.0 {
        // Never so low that the plus button, dropping below it, would leave the window.
        let max_top = (g.eh as f32 + 6.0 * s - g.tail - card_h).max(8.0 * s);
        top = lerp(top, top.min(max_top), pull);
    }
    // Capsule (p = 0) in the same coordinates: the pill sits PILL_INSET from the edge.
    let cap_near = (PILL_INSET + (PILL_W - CAP_W) / 2.0) * s;
    let cap_top = g.pill_top() - origin_y + ((PILL_W - CAP_W) / 2.0) * s + i as f32 * (CAP_H + CAP_GAP) * s;
    let near = lerp(cap_near, out - CARD_W * s, p);
    let far = lerp(cap_near + CAP_W * s, out, p);
    let (x, wd) = span(g, w, near, far);
    let y = lerp(cap_top, top, p);
    let h = lerp(CAP_H * s, card_h, p);
    let r = lerp(CAP_W * s / 2.0, CARD_R * s, clamp01(p));
    let edge_x = if g.left { 0.0 } else { w as f32 };
    let angle = t.lean.value * clamp01(p) * if g.left { -1.0 } else { 1.0 };
    let mut shape = Shape::new(x, y, wd, h, r).rotated(angle, edge_x, y + h / 2.0);
    let press = t.press.value;
    if press < 0.999 {
        shape = shape.inset((1.0 - press) * 40.0 * s);
    }
    let spine_x = if g.left { x + wd } else { x };
    CardPose { shape, spine_x, pull, reveal: p }
}

/// How far the "+N" tile and the plus button move down to clear a pulled-out card that
/// reaches past the bottom of the stack. It follows the card's pull spring, so they slide
/// out of the way and back with it.
pub(super) fn footer_drop(d: &Deck) -> f32 {
    let g = &d.geo;
    let rest = g.slot_y(g.n) + 6.0 * g.s; // bottom of the last tucked card
    let bottoms = d.tabs.iter().enumerate().filter(|(_, t)| !t.open && t.pull.value > 0.0).map(|(i, _)| {
        let sh = pose(d, i, g.ew, g.ey as f32).shape;
        sh.y + sh.h - rest
    });
    bottoms.fold(0.0, f32::max)
}

/// The "+N" tile or the plus button as drawn now (window coordinates), for its tooltip.
pub(super) fn footer_rect(d: &Deck, h: Hit) -> Option<RECT> {
    let g = &d.geo;
    let s = g.s;
    let centre_near = (TAB_W / 2.0 + 2.0) * s;
    let (cx, _) = span(g, g.ew, centre_near, centre_near);
    let drop = footer_drop(d);
    let (w, top, ht) = match h {
        Hit::More if g.more > 0 => ((TAB_W - 10.0) * s, g.more_y() + drop, 26.0 * s),
        Hit::Plus => (34.0 * s, g.plus_cy() + drop - 17.0 * s, 34.0 * s),
        _ => return None,
    };
    Some(rect((cx - w / 2.0) as i32, top as i32, w as i32, ht as i32))
}

pub(super) fn preview_height(d: &Deck, i: usize) -> f32 {
    let s = d.geo.s;
    // Title band above the body, padding below it (as `draw_card` lays them out).
    let h = 40.0 * s + d.tabs[i].body_h.unwrap_or(0.0) + 14.0 * s;
    h.clamp(d.geo.tab_h, PREVIEW_MAX * s)
}

pub(super) fn hit_test(d: &Deck, x: i32, y: i32) -> Hit {
    if !d.wide || !d.open {
        return Hit::None;
    }
    let g = &d.geo;
    let s = g.s;
    let (fx, fy) = (x as f32, y as f32);
    // Topmost first: pulled-out cards, then tabs from the bottom of the stack up.
    let mut order: Vec<usize> = (0..d.tabs.len()).collect();
    order.sort_by(|&a, &b| d.tabs[b].pull.value.partial_cmp(&d.tabs[a].pull.value).unwrap_or(std::cmp::Ordering::Equal).then(b.cmp(&a)));
    for i in order {
        if d.tabs[i].open || d.tabs[i].reveal.value < 0.3 {
            continue;
        }
        let cp = pose(d, i, g.ew, g.ey as f32);
        let sh = cp.shape;
        let (x0, x1) = (sh.x.max(0.0), (sh.x + sh.w).min(g.ew as f32));
        // A pulled-out card also owns the strip between it and the screen edge, across
        // its own slot: the pointer rests at the edge and mustn't fall "off" the card it
        // pulled out (it would tuck back under the pointer and pull out again, forever).
        // Elsewhere along the edge the neighbouring cards stay reachable for scrubbing.
        let t = &d.tabs[i];
        if t.pull.value > 0.05 && fy >= t.y.value && fy < t.y.value + g.tab_h {
            let (s0, s1) = if g.left { (0.0, x0) } else { (x1, g.ew as f32) };
            if fx >= s0 && fx <= s1 + 2.0 {
                return Hit::Tab(i);
            }
        }
        if fx >= x0 - 2.0 && fx < x1 + 2.0 && fy >= sh.y && fy < sh.y + sh.h {
            return Hit::Tab(i);
        }
    }
    let centre_near = (TAB_W / 2.0 + 2.0) * s;
    let (cx, _) = span(g, g.ew, centre_near, centre_near);
    let drop = footer_drop(d);
    if g.more > 0 {
        let my = g.more_y() + drop;
        if fy >= my && fy < my + 26.0 * s && (fx - cx).abs() < TAB_W * s / 2.0 {
            return Hit::More;
        }
    }
    let (dx, dy) = (fx - cx, fy - g.plus_cy() - drop);
    if dx * dx + dy * dy <= (21.0 * s).powi(2) {
        return Hit::Plus;
    }
    Hit::None
}
