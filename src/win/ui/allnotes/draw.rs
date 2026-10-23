// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Painting: the list, the detail card and the window chrome.

use super::*;

pub(super) fn paint_now(h: HWND) {
    unsafe {
        let dc = GetDC(h);
        render(dc);
        ReleaseDC(h, dc);
        ValidateRect(h, std::ptr::null());
    }
}

pub(super) fn render(dc: HDC) {
    with(|l| {
        let lay = layout(l);
        if l.canvas.as_ref().map(|c| (c.w, c.h)) != Some((lay.w, lay.h)) {
            l.canvas = Some(Canvas::new(lay.w, lay.h));
        }
        let mut c = l.canvas.take().unwrap();
        draw(l, &lay, &mut c);
        c.make_opaque();
        c.blit(dc, 0, 0);
        l.canvas = Some(c);
    });
}

pub(super) fn icon(c: &mut Canvas, glyph: &str, f: &Font, r: RECT, rgb: u32) {
    c.text_ct(glyph, f, r, None, rgb, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

pub(super) fn draw(l: &Lib, lay: &Layout, c: &mut Canvas) {
    let dark = app::dark();
    let p = pal(dark);
    let s = l.s;
    let f = &l.fonts;
    let px = |v: f32| (v * s).round() as i32;
    c.clear(0xFF00_0000 | p.window);
    c.begin_batch(); // all text below is drawn in one Direct2D pass at the end
    c.rrect(0.0, 0.0, lay.list_w, lay.h as f32, 0.0, p.list, 1.0);
    if lay.wide {
        c.rrect(lay.list_w, 0.0, 1.0, lay.h as f32, 0.0, p.line, 1.0);
    }

    // Header: large title and count share one baseline.
    let title = match l.mode {
        1 => t!("library_title_archive"),
        2 => t!("library_title_all"),
        _ => t!("library_title_notes"),
    };
    let base = px(TITLE_BASE);
    let tw = c.line_ct(title, &f.big, px(24.0), base, (lay.list_w * 0.6) as i32, None, p.text, DT_LEFT);
    let count = tn!("notes_count", l.rows.len());
    c.line_ct(&count, &f.count, px(24.0) + tw + px(10.0), base, px(120.0), None, p.sub, DT_LEFT);
    for (hit, r) in header_buttons(lay, s) {
        if l.hover == hit {
            let a = p.hover.1 * if l.pressed == hit { 2.4 } else { 1.6 };
            c.shape(&Shape::new(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 8.0 * s), p.hover.0, a, 1.0, 0.0);
        }
        let glyph = match hit {
            Hit::New => "\u{E710}",
            Hit::Import => "\u{E896}",
            Hit::Settings => "\u{E713}",
            _ => "\u{E898}",
        };
        icon(c, glyph, &f.icons, r, p.text);
    }

    // Search field (with focus ring) — the EDIT control sits on top of it.
    let sf = lay.search;
    c.shape(&sf, p.field.0, p.field.1, 1.0, 0.0);
    if l.focus_search {
        let ring = Shape::new(sf.x - 3.0 * s, sf.y - 3.0 * s, sf.w + 6.0 * s, sf.h + 6.0 * s, sf.r + 3.0 * s);
        c.shape(&ring, p.focus, 0.6, 1.0, 3.0 * s);
    }
    let field_rgb = mix(p.list, p.field.0, p.field.1);
    icon(c, "\u{E721}", &f.chrome, rect((sf.x + 10.0 * s) as i32, sf.y as i32, px(20.0), sf.h as i32), mix(field_rgb, p.sub, 0.9));
    if !l.focus_search && l.query.is_empty() {
        let base = (sf.y + sf.h / 2.0) as i32 + f.count.cap / 2;
        c.line_ct(&crate::common::i18n::keys(&["ctrl", "F"]), &f.count, (sf.x + sf.w) as i32 - px(80.0), base, px(68.0), None, p.ter, DT_RIGHT);
    }

    // Segmented control: the pill glides on a spring.
    let sg = lay.seg;
    c.shape(&sg, p.field.0, p.field.1, 1.0, 0.0);
    let seg_w = sg.w / 3.0;
    let pill = Shape::new(sg.x + 2.0 * s + l.seg_x.value * seg_w, sg.y + 2.0 * s, seg_w - 4.0 * s, sg.h - 4.0 * s, sg.r - 2.0 * s);
    c.card(&pill, p.pill, 1.0, 0x000000, 0.05, 0.35, s);
    let seg_base = (sg.y + sg.h / 2.0) as i32 + f.seg.cap / 2;
    for (i, name) in segments().iter().enumerate() {
        let x = (sg.x + i as f32 * seg_w) as i32;
        let on = i == l.mode;
        if !on && l.hover == Hit::Segment(i) {
            let sh = Shape::new(x as f32 + 3.0 * s, sg.y + 3.0 * s, seg_w - 6.0 * s, sg.h - 6.0 * s, sg.r - 3.0 * s);
            c.shape(&sh, p.hover.0, p.hover.1, 1.0, 0.0);
        }
        c.line_ct(name, if on { &f.seg_on } else { &f.seg }, x, seg_base, seg_w as i32, None, if on { p.text } else { p.sub }, DT_CENTER);
    }

    // List.
    let top = lay.list_top;
    let bottom = lay.h as f32;
    let la = clamp01(l.list_a.value);
    let slide = (1.0 - l.list_a.value) * 10.0 * s;
    let clip = Some(rect(0, top as i32, lay.list_w as i32, lay.h - top as i32));
    if l.rows.is_empty() {
        let (headline, sub) = if !l.query.is_empty() {
            (t!("empty_search_title", l.query), t!("empty_search_body").to_string())
        } else if l.mode == 1 {
            (t!("empty_archive_title").to_string(), t!("empty_archive_body").to_string())
        } else {
            (t!("empty_notes_title").to_string(), t!("empty_notes_body", crate::common::i18n::keys(&["ctrl", "alt", "N"])))
        };
        let y = px(LIST_TOP + 64.0);
        c.line_ct(&headline, &f.empty, px(16.0), y, (lay.list_w - 32.0 * s) as i32, None, mix(p.list, p.text, la), DT_CENTER);
        let sr = rect(px(32.0), y + px(10.0), (lay.list_w - 64.0 * s) as i32, px(40.0));
        c.text_ct(&sub, &f.count, sr, None, mix(p.list, p.sub, la), DT_CENTER | DT_WORDBREAK);
    }
    let row_x = 10.0 * s;
    let row_w = lay.list_w - 20.0 * s;
    let row_shape = |y: f32| clip_shape(Shape::new(row_x, y, row_w, lay.row_h - 2.0 * s, 10.0 * s), top);
    if l.hover_a.value > 0.01 {
        let y = l.hover_y.value - l.scroll.value + slide;
        if y + lay.row_h > top && y < bottom {
            c.shape(&row_shape(y), p.hover.0, p.hover.1 * l.hover_a.value * la, 1.0, 0.0);
        }
    }
    let primary = l.sel.last().copied();
    let first = ((l.scroll.value - slide) / lay.row_h).floor().max(0.0) as usize;
    let visible = ((bottom - top) / lay.row_h).ceil() as usize + 2;
    let range = first.min(l.rows.len())..(first + visible).min(l.rows.len());
    for i in range.clone() {
        let r = &l.rows[i];
        if l.sel.contains(&r.id) && Some(r.id) != primary {
            c.shape(&row_shape(row_y(l, lay, i) + slide), theme::paper(r.color, dark), la, 1.0, 0.0);
        }
    }
    if let Some(i) = primary_index(l) {
        let y = l.sel_y.value - l.scroll.value + slide;
        if y + lay.row_h > top && y < bottom {
            c.shape(&row_shape(y), theme::paper(l.rows[i].color, dark), la, 1.0, 0.0);
        }
    }
    let sel_idx: Vec<usize> = range.clone().filter(|i| l.sel.contains(&l.rows[*i].id)).collect();
    let lifted = l.drag.as_ref().filter(|dr| dr.active).map(|dr| dr.from);
    let order: Vec<usize> = range.clone().filter(|i| Some(*i) != lifted).chain(lifted).collect();
    for i in order {
        let r = &l.rows[i];
        let y = row_y(l, lay, i) + slide;
        if y + lay.row_h <= top || y >= bottom {
            continue;
        }
        if Some(i) == lifted {
            // Above everything drawn so far, text included: flush, lift, then draw it.
            c.flush();
            c.begin_batch();
            c.card(&row_shape(y), theme::paper(r.color, dark), 1.0, 0x000000, 0.06, 1.4, s);
        }
        let selected = l.sel.contains(&r.id);
        let bg = if selected { theme::paper(r.color, dark) } else { p.list };
        let fade = |rgb: u32| mix(bg, rgb, la);
        let (tc, mc) = if selected {
            (theme::PAPER_TEXT, theme::paper_subtext(r.color, dark))
        } else {
            (p.text, p.sub)
        };
        let tx = (row_x + 30.0 * s) as i32;
        let right = (row_x + row_w - 14.0 * s) as i32;
        let tb = (y + ROW_TITLE_BASE * s) as i32;
        let mb = (y + ROW_META_BASE * s) as i32;
        // Colour dot, centred on the title's cap height.
        let d = 8.0 * s;
        let dcy = tb as f32 - f.row_title.cap as f32 / 2.0;
        if dcy - d / 2.0 > top {
            c.shape(&Shape::new(row_x + 13.0 * s, dcy - d / 2.0, d, d, d / 2.0), theme::color(r.color).accent, la, 1.0, 0.0);
        }
        c.line_ct(&r.title, &f.row_title, tx, tb, right - tx, clip, fade(tc), DT_LEFT);
        // Second line: when it was edited (and where it lives, when that isn't the deck).
        let time = if r.archived && l.mode == 2 {
            t!("row_archived", r.time)
        } else if r.off_deck && !r.archived {
            t!("row_off_deck", r.time)
        } else {
            r.time.clone()
        };
        c.line_ct(&time, &f.row_time, tx, mb, right - tx, clip, fade(mc), DT_LEFT);
        // Inset hairline separator, hidden next to highlighted rows.
        let next_selected = sel_idx.contains(&(i + 1));
        if !selected && !next_selected && i + 1 < l.rows.len() && lifted.is_none() {
            let sy = y + lay.row_h - 1.0 * s;
            if sy > top {
                c.rrect(tx as f32, sy, (right - tx) as f32 + 14.0 * s, 1.0, 0.0, mix(p.list, p.line, la), 1.0);
            }
        }
    }
    // Overlay scrollbar, shown while scrolling.
    let max = max_scroll(l, lay);
    if max > 0.0 && l.bar_a.value > 0.01 {
        let view = bottom - top;
        let bar_h = (view * view / (view + max)).max(28.0 * s);
        let y = top + (view - bar_h) * clamp01(l.scroll.value / max);
        c.shape(&Shape::new(lay.list_w - 9.0 * s, y + 2.0 * s, 5.0 * s, bar_h - 4.0 * s, 2.5 * s), p.text, 0.32 * l.bar_a.value, 1.0, 0.0);
    }

    if lay.wide {
        draw_detail(l, lay, c, &p, dark);
    }

    // Caption buttons last, above everything.
    let zoomed = unsafe { IsZoomed(l.hwnd) } != 0;
    for (hit, r) in caption_rects(lay, s) {
        let hot = l.hover == hit;
        let under = if (r.left as f32) < lay.list_w { p.list } else { p.window };
        let mut bg = under;
        if hot {
            bg = if hit == Hit::Close { 0xE81123 } else { mix(under, p.hover.0, p.hover.1 * 1.6) };
            c.rrect(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 0.0, bg, 1.0);
        }
        let glyph = match hit {
            Hit::Close => "\u{E8BB}",
            Hit::Max if zoomed => "\u{E923}",
            Hit::Max => "\u{E922}",
            _ => "\u{E921}",
        };
        let ink = if hot && hit == Hit::Close { 0xFFFFFF } else { mix(bg, p.text, 0.9) };
        icon(c, glyph, &f.chrome, r, ink);
    }
    c.flush();
}

/// Clip a row highlight so it never draws over the header above the list.
pub(super) fn clip_shape(mut sh: Shape, top: f32) -> Shape {
    if sh.y < top {
        let cut = top - sh.y;
        sh.y = top;
        sh.h -= cut;
        sh.r = sh.r.min(sh.h / 2.0).max(0.0);
    }
    sh
}

pub(super) fn draw_detail(l: &Lib, lay: &Layout, c: &mut Canvas, p: &Pal, dark: bool) {
    let s = l.s;
    let f = &l.fonts;
    let px = |v: f32| (v * s).round() as i32;
    if l.sel.is_empty() {
        return;
    }
    let a = clamp01(l.detail_a.value);
    let lift = 0.0;
    let primary_row = primary_index(l).map(|i| &l.rows[i]);
    // Toolbar: state chip on the left, actions on the right, one shared baseline.
    let bar_mid = (TOOLBAR_Y + TOOLBAR_H / 2.0) * s;
    let label_base = bar_mid as i32 + f.button.cap / 2;
    if let Some(r) = primary_row {
        let state = if l.sel.len() > 1 {
            tn!("state_selected", l.sel.len())
        } else if r.archived {
            t!("state_archived").to_string()
        } else {
            if r.off_deck { t!("state_off_deck") } else { t!("state_in_deck") }.to_string()
        };
        let x = lay.detail_x + 28.0 * s;
        let d = 8.0 * s;
        c.shape(&Shape::new(x, bar_mid - d / 2.0, d, d, d / 2.0), theme::color(r.color).accent, 1.0, 1.0, 0.0);
        let chip_base = bar_mid as i32 + f.chip.cap / 2;
        c.line_ct(&state, &f.chip, (x + 16.0 * s) as i32, chip_base, px(260.0), None, p.sub, DT_LEFT);
    }
    for (hit, r, label, glyph) in detail_buttons(l, lay) {
        let hot = l.hover == hit;
        let down = hot && l.pressed == hit;
        let sh = Shape::new(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, TOOLBAR_H * s / 2.0);
        let (fill, fa, ink) = match hit {
            // Tinted at rest (deeper red text in light mode for contrast), solid when aimed at.
            Hit::Delete if hot => (p.danger, if down { 0.82 } else { 1.0 }, 0xFFFFFF),
            Hit::Delete => (p.danger, 0.10, if dark { p.danger } else { 0xD70015 }),
            _ => (p.field.0, p.field.1 * if down { 2.2 } else if hot { 1.6 } else { 1.0 }, p.text),
        };
        c.shape(&sh, fill, fa, 1.0, 0.0);
        // Icon and label centred together.
        let (iw, gap, tw) = ((BUTTON_ICON * s) as i32, (BUTTON_ICON_GAP * s) as i32, text_w(&f.button, label));
        let x0 = r.left + ((r.right - r.left) - (iw + gap + tw)) / 2;
        icon(c, glyph, &f.button_icon, rect(x0, r.top, iw, r.bottom - r.top), ink);
        c.line_ct(label, &f.button, x0 + iw + gap, label_base, tw + 2, None, ink, DT_LEFT);
    }
    let mut card = lay.card;
    card.y += lift;
    if l.sel.len() > 1 {
        // A small fanned stack for multi-selection.
        let n = l.sel.len().min(3);
        let ids: Vec<u64> = l.sel.iter().rev().take(n).rev().copied().collect();
        for (k, id) in ids.iter().enumerate() {
            let Some(r) = l.rows.iter().find(|r| r.id == *id) else { continue };
            let off = (n - 1 - k) as f32;
            let w = (card.w * 0.6).min(360.0 * s);
            let h = (card.h * 0.55).min(300.0 * s);
            let sh = Shape::new(card.x + (card.w - w) / 2.0 + off * 14.0 * s, card.y + (card.h - h) / 2.0 - off * 14.0 * s, w, h, 16.0 * s)
                .rotated((off - 1.0) * 0.035, card.x + card.w / 2.0, card.y + card.h / 2.0);
            c.card(&sh, theme::paper(r.color, dark), a, 0x000000, 0.06, 1.2 * a, s);
            if k == n - 1 {
                let bg = theme::paper(r.color, dark);
                c.line_ct(&r.title, &f.card_title, (sh.x + 24.0 * s) as i32, (sh.y + 48.0 * s) as i32, (w - 48.0 * s) as i32, None, mix(bg, theme::PAPER_TEXT, a), DT_LEFT);
            }
        }
        let msg = tn!("notes_selected", l.sel.len());
        c.line_ct(&msg, &f.empty, card.x as i32, (card.y + card.h - 32.0 * s) as i32, card.w as i32, None, mix(p.window, p.sub, a), DT_CENTER);
        return;
    }
    let Some(r) = primary_row else { return };
    if let Some(b) = show_window_rect(l, lay) {
        let hot = l.hover == Hit::OpenHere;
        let down = hot && l.pressed == Hit::OpenHere;
        let sh = Shape::new(b.left as f32, b.top as f32, (b.right - b.left) as f32, (b.bottom - b.top) as f32, (b.bottom - b.top) as f32 / 2.0);
        c.shape(&sh, p.field.0, a * p.field.1 * if down { 2.4 } else if hot { 1.7 } else { 1.0 }, 1.0, 0.0);
        let base = (b.top + b.bottom) / 2 + f.button.cap / 2;
        c.line_ct(t!("button_open_here"), &f.button, b.left, base, b.right - b.left, None, mix(p.window, p.text, a), DT_CENTER);
        return;
    }
    let paper = theme::paper(r.color, dark);
    let ink = |rgb: u32, k: f32| mix(paper, rgb, a * k);
    // The card and its large soft shadow are the most expensive pixels in the window and
    // rarely change, so they're cached and pasted back each frame.
    let region = rect((card.x - 34.0 * s) as i32, (card.y - 16.0 * s) as i32, (card.w + 68.0 * s) as i32, (card.h + 50.0 * s) as i32);
    let key = (region.left, region.top, region.right, region.bottom, paper, dark);
    let hit = CARD_CACHE.with(|cc| {
        let cc = cc.borrow();
        match cc.as_ref() {
            Some((k, px)) if *k == key => {
                c.copy_in(region, px);
                true
            }
            _ => false,
        }
    });
    if !hit {
        c.card(&card, paper, 1.0, 0x000000, 0.05, 1.3, s);
        let px = c.copy_out(region);
        CARD_CACHE.with(|cc| *cc.borrow_mut() = Some((key, px)));
    }
    let x0 = (card.x + 28.0 * s) as i32;
    let x1 = (card.x + card.w - 28.0 * s) as i32;
    let sw = swatch_rects(lay, s);
    let rule_y = sw[0].top as f32 - 18.0 * s;
    let editing_inline = l.editor_id == r.id && !l.editor.is_null();
    if !editing_inline {
        // Static preview while the inline editor is being made.
        c.line_ct(&r.title, &f.card_title, x0, (card.y + 52.0 * s) as i32, x1 - x0, None, ink(theme::PAPER_TEXT, 1.0), DT_LEFT);
        let br = rect(x0, (card.y + 72.0 * s) as i32, x1 - x0, (rule_y - 44.0 * s - card.y - 72.0 * s).max(0.0) as i32);
        let (body, k) = if r.body.is_empty() { (t!("card_no_text"), 0.55) } else { (r.body.as_str(), 0.92) };
        c.text_ct(body, &f.card_body, br, Some(br), ink(theme::PAPER_TEXT, k), DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS);
    }
    // Footer: when it was edited, then a hairline and the colour swatches.
    let meta = t!("card_meta", edited_time(r.updated), relative_time(r.created));
    c.line_ct(&meta, &f.card_meta, x0, (rule_y - 12.0 * s) as i32, x1 - x0, None, ink(theme::paper_subtext(r.color, dark), 1.0), DT_LEFT);
    c.rrect(x0 as f32, rule_y, (x1 - x0) as f32, 1.0, 0.0, 0x000000, 0.08 * a);
    for (k, rc) in sw.iter().enumerate() {
        let ci = theme::ORDER[k];
        let hot = l.hover == Hit::Swatch(k);
        let on = ci == r.color % theme::COLORS.len() as u8;
        let grow = if hot { 2.0 * s } else { 0.0 };
        let sh = Shape::new(rc.left as f32 - grow, rc.top as f32 - grow + lift, (rc.right - rc.left) as f32 + 2.0 * grow, (rc.bottom - rc.top) as f32 + 2.0 * grow, 100.0);
        c.shape(&sh, theme::color(ci).accent, a, 1.0, 0.0);
        c.shape(&sh, 0x000000, 0.15 * a, 1.0, 1.0);
        if on {
            let ring = Shape::new(sh.x - 4.0 * s, sh.y - 4.0 * s, sh.w + 8.0 * s, sh.h + 8.0 * s, 100.0);
            c.shape(&ring, theme::PAPER_TEXT, 0.55 * a, 1.0, 1.6 * s);
        }
    }
}
