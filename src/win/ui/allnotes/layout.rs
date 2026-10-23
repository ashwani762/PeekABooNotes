// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Geometry: where everything sits, and what the pointer is over.

use super::*;

pub(super) fn client_size(h: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    (rc.right.max(1), rc.bottom.max(1))
}

pub(super) fn layout(l: &Lib) -> Layout {
    let (w, h) = client_size(l.hwnd);
    let s = l.s;
    let wide = w as f32 >= 720.0 * s;
    let list_w = (if wide { (w as f32 * 0.4).clamp(330.0 * s, 420.0 * s) } else { w as f32 }).round();
    let pad = 20.0 * s;
    let search = Shape::new(pad, SEARCH_Y * s, list_w - 2.0 * pad, SEARCH_H * s, 10.0 * s);
    let seg = Shape::new(pad, SEG_Y * s, list_w - 2.0 * pad, SEG_H * s, 9.0 * s);
    let detail_x = if wide { list_w } else { w as f32 };
    let card = Shape::new(detail_x + 28.0 * s, CARD_Y * s, (w as f32 - detail_x - 56.0 * s).max(10.0), (h as f32 - CARD_Y * s - 28.0 * s).max(10.0), 18.0 * s);
    Layout { w, h, wide, list_w, search, seg, list_top: LIST_TOP * s, row_h: ROW_H * s, detail_x, card }
}

pub(super) fn caption_rects(lay: &Layout, s: f32) -> [(Hit, RECT); 3] {
    let (bw, bh) = ((46.0 * s) as i32, (34.0 * s) as i32);
    let r = |i: i32| rect(lay.w - bw * (i + 1), 0, bw, bh);
    [(Hit::Close, r(0)), (Hit::Max, r(1)), (Hit::Min, r(2))]
}

/// Toolbar icons beside the large title, centred on its cap height.
pub(super) fn header_buttons(lay: &Layout, s: f32) -> [(Hit, RECT); 4] {
    let size = (30.0 * s) as i32;
    let right = if lay.wide { lay.list_w as i32 - (14.0 * s) as i32 } else { lay.w - (3.0 * 46.0 * s) as i32 - (10.0 * s) as i32 };
    let cy = (TITLE_BASE - 28.0 * 0.7 / 2.0) * s;
    let y = (cy - size as f32 / 2.0).round() as i32;
    let gap = (2.0 * s) as i32;
    let r = |i: i32| rect(right - size * (i + 1) - gap * i, y, size, size);
    [(Hit::Settings, r(0)), (Hit::Export, r(1)), (Hit::Import, r(2)), (Hit::New, r(3))]
}

/// The tooltip for a toolbar icon under the pointer, or none.
pub(super) fn header_tip(l: &Lib, hit: Hit) {
    use crate::win::ui::tooltip;
    let lay = layout(l);
    let Some((_, r)) = header_buttons(&lay, l.s).into_iter().find(|(x, _)| *x == hit) else {
        tooltip::leave(l.hwnd);
        return;
    };
    let (label, keys) = match hit {
        Hit::New => (t!("menu_new_note"), crate::common::i18n::keys(&["ctrl", "N"])),
        Hit::Import => (t!("tip_import"), String::new()),
        Hit::Export => (t!("tip_export"), String::new()),
        _ => (t!("menu_settings"), String::new()),
    };
    tooltip::hover(l.hwnd, r, label, &keys);
}

/// The detail pane's buttons: (hit, rect, label, icon glyph).
pub(super) fn detail_buttons(l: &Lib, lay: &Layout) -> Vec<(Hit, RECT, &'static str, &'static str)> {
    let s = l.s;
    let archived = l.rows.iter().filter(|r| l.sel.contains(&r.id)).all(|r| r.archived);
    let archive = if archived { (t!("button_restore"), "\u{E7A7}") } else { (t!("button_archive"), "\u{E7B8}") };
    let labels: [(Hit, &'static str, &'static str); 2] = [(Hit::Archive, archive.0, archive.1), (Hit::Delete, t!("button_delete"), "\u{E74D}")];
    let mut out = Vec::new();
    let mut x = lay.w - (28.0 * s) as i32;
    for (hit, label, glyph) in labels.iter().rev() {
        let w = text_w(&l.fonts.button, label) + ((28.0 + BUTTON_ICON + BUTTON_ICON_GAP) * s) as i32;
        x -= w;
        out.push((*hit, rect(x, (TOOLBAR_Y * s) as i32, w, (TOOLBAR_H * s) as i32), *label, *glyph));
        x -= (8.0 * s) as i32;
    }
    out.reverse();
    out
}

/// While the selected note is open in its own window, the detail pane doesn't repeat
/// it: there's only a button to bring it back here (its window closes, the inline
/// editor takes over).
pub(super) fn in_window(l: &Lib) -> Option<u64> {
    let id = *l.sel.last()?;
    (l.sel.len() == 1 && app::ui(|u| u.notes.contains_key(&id))).then_some(id)
}

pub(super) fn show_window_rect(l: &Lib, lay: &Layout) -> Option<RECT> {
    in_window(l)?;
    let s = l.s;
    let w = (l.fonts.button.measure(t!("button_open_here")).0 as f32 + 36.0 * s) as i32;
    let h = (32.0 * s) as i32;
    let cx = (lay.card.x + lay.card.w / 2.0) as i32;
    let cy = (lay.card.y + lay.card.h / 2.0) as i32;
    Some(rect(cx - w / 2, cy - h / 2, w, h))
}

/// One swatch per colour, in display (hue) order; `Hit::Swatch(k)` is `theme::ORDER[k]`.
pub(super) fn swatch_rects(lay: &Layout, s: f32) -> Vec<RECT> {
    let n = theme::ORDER.len() as f32;
    let d = 18.0 * s;
    let room = lay.card.w - 56.0 * s;
    let gap = ((room - n * d) / (n - 1.0)).clamp(4.0 * s, 10.0 * s);
    let y = lay.card.y + lay.card.h - 26.0 * s - d;
    (0..theme::ORDER.len()).map(|i| rect((lay.card.x + 28.0 * s + i as f32 * (d + gap)) as i32, y as i32, d as i32, d as i32)).collect()
}

pub(super) fn hit_test(l: &Lib, x: i32, y: i32) -> Hit {
    let lay = layout(l);
    let s = l.s;
    let pt = |r: &RECT| crate::win::util::pt_in(r, x, y);
    for (h, r) in caption_rects(&lay, s) {
        if pt(&r) {
            return h;
        }
    }
    for (h, r) in header_buttons(&lay, s) {
        if pt(&r) {
            return h;
        }
    }
    let (fx, fy) = (x as f32, y as f32);
    let inside = |sh: &Shape| fx >= sh.x && fx < sh.x + sh.w && fy >= sh.y && fy < sh.y + sh.h;
    if inside(&lay.search) {
        return Hit::Search;
    }
    if inside(&lay.seg) {
        let i = ((fx - lay.seg.x) / (lay.seg.w / 3.0)).floor().clamp(0.0, 2.0) as usize;
        return Hit::Segment(i);
    }
    if fx < lay.list_w && fy >= lay.list_top {
        // Rows are uniform: compute the index directly (fast with thousands of notes).
        let i = ((fy - lay.list_top + l.scroll.value) / lay.row_h).floor();
        if i >= 0.0 && (i as usize) < l.rows.len() && fx >= 10.0 * s && fx < lay.list_w - 10.0 * s {
            return Hit::Row(i as usize);
        }
        return Hit::None;
    }
    if lay.wide && !l.sel.is_empty() {
        for (h, r, ..) in detail_buttons(l, &lay) {
            if pt(&r) {
                return h;
            }
        }
        if let Some(r) = show_window_rect(l, &lay) {
            if pt(&r) {
                return Hit::OpenHere;
            }
            return Hit::None;
        }
        if l.sel.len() == 1 {
            for (i, r) in swatch_rects(&lay, s).iter().enumerate() {
                if pt(r) {
                    return Hit::Swatch(i);
                }
            }
            if inside(&lay.card) {
                return Hit::Card;
            }
        }
    }
    Hit::None
}
