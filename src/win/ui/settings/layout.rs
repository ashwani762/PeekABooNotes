// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings geometry: rows, controls and hit testing.

use super::*;

pub(super) fn client_size(h: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    (rc.right.max(1), rc.bottom.max(1))
}

pub(super) fn close_rect(w: &Win) -> RECT {
    let (cw, _) = client_size(w.hwnd);
    let (bw, bh) = ((46.0 * w.s) as i32, (34.0 * w.s) as i32);
    rect(cw - bw, 0, bw, bh)
}

pub(super) fn side_item(w: &Win, i: usize) -> Shape {
    let s = w.s;
    Shape::new(10.0 * s, SIDE_TOP * s + i as f32 * SIDE_ROW * s, (SIDE_W - 20.0) * s, 32.0 * s, 8.0 * s)
}

pub(super) fn content_x(w: &Win) -> (f32, f32) {
    let (cw, _) = client_size(w.hwnd);
    let x0 = (SIDE_W + PAD) * w.s;
    (x0, cw as f32 - x0 - PAD * w.s)
}

pub(super) fn row_h(r: &Row) -> f32 {
    match (&r.ctl, &r.sub) {
        (Ctl::Preview, _) => PREVIEW_H,
        (_, Some(_)) => ROW_SUB_H,
        (Ctl::Keys(_) | Ctl::Hotkey(_), _) => KEY_ROW_H,
        _ => ROW_H,
    }
}

/// The room row `row` (laid out at `r`) leaves its subtitle: from the inset to its control.
pub(super) fn sub_width(w: &Win, row: &Row, r: &RECT) -> i32 {
    let lx = r.left + (INSET * w.s) as i32;
    ((r.right - lx) - px_ctl(w, row, r) - (12.0 * w.s) as i32).max(10)
}

/// Whether row `row`'s subtitle wraps onto more lines when it's too long for one. The
/// folder's path is shortened in the middle instead, and the colour row's name is short.
pub(super) fn sub_wraps(row: &Row) -> bool {
    row.sub.is_some() && row.key != Some(Key::Folder) && !matches!(row.ctl, Ctl::Swatches)
}

/// How much taller than one line row `row`'s subtitle is at `r`'s width (0 for one line).
pub(super) fn sub_extra(w: &Win, row: &Row, r: &RECT) -> i32 {
    match &row.sub {
        Some(sub) if sub_wraps(row) => {
            let line = w.fonts.sub.ascent + w.fonts.sub.descent;
            (w.fonts.sub.wrapped_height(sub, sub_width(w, row, r)) - line).max(0)
        }
        _ => 0,
    }
}

pub(super) fn place<'a>(w: &Win, groups: &'a [Group]) -> Vec<Item<'a>> {
    place_to(w, groups).0
}

/// The top of the page, where its title stays while the rest scrolls under it.
pub(super) fn header_band(w: &Win) -> f32 {
    (GROUP_TOP - 8.0) * w.s
}

/// How far the current page can scroll: its height past the window's.
pub(super) fn max_scroll(w: &Win) -> f32 {
    let groups = groups(w);
    let end = place_to(w, &groups).1 + w.scroll.value;
    let (_, ch) = client_size(w.hwnd);
    (end + PAD * w.s - ch as f32).max(0.0)
}

/// The page laid out, scrolled; and where it ends.
fn place_to<'a>(w: &Win, groups: &'a [Group]) -> (Vec<Item<'a>>, f32) {
    let s = w.s;
    let (x0, cw) = content_x(w);
    let mut y = GROUP_TOP * s - w.scroll.value;
    let mut out = Vec::new();
    for g in groups {
        if let Some(h) = g.header {
            out.push(Item::Header(h, (y + 17.0 * s) as i32));
            y += HEADER_H * s;
        }
        // A subtitle too long for one line wraps, and its row grows by the extra lines.
        let at = |r: &Row, y: f32| {
            let one = rect(x0 as i32, y as i32, cw as i32, (row_h(r) * s) as i32);
            row_h(r) * s + sub_extra(w, r, &one) as f32
        };
        let (mut ty, mut total) = (y, 0.0);
        for r in &g.rows {
            let h = at(r, ty);
            ty += h;
            total += h;
        }
        out.push(Item::Box(Shape::new(x0, y, cw, total, 10.0 * s)));
        for (i, r) in g.rows.iter().enumerate() {
            let h = at(r, y);
            out.push(Item::Row(Placed { row: r, r: rect(x0 as i32, y as i32, cw as i32, h as i32), first: i == 0 }));
            y += h;
        }
        if let Some(f) = &g.footer {
            out.push(Item::Footer(f, (y + 19.0 * s) as i32));
            // Longer footers wrap: each extra line adds its height.
            let line = w.fonts.sub.ascent + w.fonts.sub.descent;
            let extra = (w.fonts.sub.wrapped_height(f, (cw - 4.0 * s) as i32) - line).max(0);
            y += FOOTER_H * s + extra as f32;
        }
        y += GROUP_GAP * s;
    }
    (out, y - GROUP_GAP * s)
}

pub(super) fn seg_metrics(w: &Win, labels: &[&str], r: &RECT) -> (Shape, f32) {
    let s = w.s;
    let widest = labels.iter().map(|l| w.fonts.seg.measure(l).0).max().unwrap_or(0) as f32;
    let item = seg_item(widest, labels.len(), (r.right - r.left) as f32, s);
    let (sw, sh) = (item * labels.len() as f32 + 4.0 * s, 28.0 * s);
    let x = r.right as f32 - INSET * s - sw;
    let y = (r.top + r.bottom) as f32 / 2.0 - sh / 2.0;
    (Shape::new(x, y, sw, sh, 8.0 * s), item)
}

/// Each segment's width for `n` labels the widest of which is `widest` px, in a row `row_w`
/// px wide at scale `s`: the label with room on either side, but closer in where long names
/// (German, French) would leave the row's own label no room, the control taking at most
/// seven tenths of the row.
pub(super) fn seg_item(widest: f32, n: usize, row_w: f32, s: f32) -> f32 {
    let roomy = (widest + 28.0 * s).max(46.0 * s);
    let most = (row_w * 0.7 - 4.0 * s) / n.max(1) as f32;
    roomy.min(most.max(widest + 12.0 * s))
}

pub(super) fn toggle_shape(w: &Win, r: &RECT) -> Shape {
    let s = w.s;
    let (tw, th) = (TOGGLE.0 * s, TOGGLE.1 * s);
    Shape::new(r.right as f32 - INSET * s - tw, (r.top + r.bottom) as f32 / 2.0 - th / 2.0, tw, th, th / 2.0)
}

pub(super) fn swatch_x(w: &Win, r: &RECT, i: usize) -> (f32, f32) {
    let s = w.s;
    let n = theme::ORDER.len() + 1;
    let total = n as f32 * SWATCH * s + (n - 1) as f32 * SWATCH_GAP * s;
    let x = r.right as f32 - INSET * s - total + i as f32 * (SWATCH + SWATCH_GAP) * s;
    (x, (r.top + r.bottom) as f32 / 2.0 - SWATCH * s / 2.0)
}

pub(super) fn button_shape(w: &Win, label: &str, r: &RECT) -> Shape {
    let s = w.s;
    let bw = w.fonts.button.measure(label).0 as f32 + 28.0 * s;
    let bh = 28.0 * s;
    Shape::new(r.right as f32 - INSET * s - bw, (r.top + r.bottom) as f32 / 2.0 - bh / 2.0, bw, bh, bh / 2.0)
}

/// Setting `k`'s pop-up button: its current choice, a gap and a chevron.
pub(super) fn choice_shape(w: &Win, k: Key, r: &RECT) -> Shape {
    let s = w.s;
    let bw = w.fonts.seg.measure(&choice_label(k)).0 as f32 + 44.0 * s;
    let bh = 28.0 * s;
    Shape::new(r.right as f32 - INSET * s - bw, (r.top + r.bottom) as f32 / 2.0 - bh / 2.0, bw, bh, 8.0 * s)
}

/// The width of what shortcut `i`'s field shows: its keycaps; "Off"; or, while it waits
/// for keys, the modifiers held so far and the prompt.
fn hotkey_content_w(w: &Win, i: usize) -> f32 {
    let f = &w.fonts;
    if w.edit.rec == Some(i) {
        let held = hotkey::modifier_parts(w.held);
        let prompt = f.seg.measure(t!("shortcut_press")).0 as f32;
        return if held.is_empty() { prompt } else { keys_width(w, &held) + HELD_GAP * w.s + prompt };
    }
    match app::hotkey_keys(i) {
        Some(k) => keys_width(w, &k.parts()),
        None => f.seg.measure(t!("shortcut_none")).0 as f32,
    }
}

/// Between the modifiers held and the prompt.
pub(super) const HELD_GAP: f32 = 6.0;

/// Shortcut `i`'s field: round its keycaps, which still end where other rows' do. Shown
/// when the pointer or the keys are on it, and while it waits for keys.
pub(super) fn hotkey_shape(w: &Win, i: usize, r: &RECT) -> Shape {
    let s = w.s;
    let (pad, h) = (7.0 * s, 28.0 * s);
    let right = r.right as f32 - INSET * s + pad;
    let fw = hotkey_content_w(w, i) + 2.0 * pad;
    Shape::new(right - fw, (r.top + r.bottom) as f32 / 2.0 - h / 2.0, fw, h, 7.0 * s)
}

pub(super) fn inside(sh: &Shape, x: i32, y: i32, pad: f32) -> bool {
    let (x, y) = (x as f32, y as f32);
    x >= sh.x - pad && x < sh.x + sh.w + pad && y >= sh.y - pad && y < sh.y + sh.h + pad
}

pub(super) fn hit_test(w: &Win, x: i32, y: i32) -> Hit {
    if pt_in(&close_rect(w), x, y) {
        return Hit::Close;
    }
    let s = w.s;
    if (x as f32) < SIDE_W * s {
        for i in 0..PAGES.len() {
            if inside(&side_item(w, i), x, y, 1.0 * s) {
                return Hit::Page(i);
            }
        }
        return Hit::None;
    }
    if (y as f32) < header_band(w) {
        return Hit::None;
    }
    let groups = groups(w);
    for it in place(w, &groups) {
        let Item::Row(p) = it else { continue };
        let Some(k) = p.row.key else { continue };
        if !pt_in(&p.r, x, y) {
            continue;
        }
        if p.row.disabled {
            return Hit::None;
        }
        return match &p.row.ctl {
            // The whole row flips a switch: a big, forgiving target.
            Ctl::Toggle => Hit::Ctl(k, 0),
            Ctl::Seg(labels) => {
                let (sh, item) = seg_metrics(w, labels, &p.r);
                if !inside(&sh, x, y, 2.0 * s) {
                    return Hit::None;
                }
                let i = ((x as f32 - sh.x - 2.0 * s) / item).floor().clamp(0.0, labels.len() as f32 - 1.0) as usize;
                Hit::Ctl(k, i)
            }
            Ctl::Swatches => {
                for i in 0..=theme::ORDER.len() {
                    let (sx, sy) = swatch_x(w, &p.r, i);
                    if inside(&Shape::new(sx, sy, SWATCH * s, SWATCH * s, 0.0), x, y, SWATCH_GAP * s / 2.0) {
                        return Hit::Ctl(k, i);
                    }
                }
                Hit::None
            }
            Ctl::Button(label) => {
                if inside(&button_shape(w, label, &p.r), x, y, 2.0 * s) {
                    Hit::Ctl(k, 0)
                } else {
                    Hit::None
                }
            }
            Ctl::Choice => {
                if inside(&choice_shape(w, k, &p.r), x, y, 2.0 * s) {
                    Hit::Ctl(k, 0)
                } else {
                    Hit::None
                }
            }
            Ctl::Hotkey(i) => {
                if inside(&hotkey_shape(w, *i, &p.r), x, y, 2.0 * s) {
                    Hit::Ctl(k, 0)
                } else {
                    Hit::None
                }
            }
            Ctl::Keys(_) | Ctl::Preview => Hit::None,
        };
    }
    Hit::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_close_in_for_long_names() {
        // Short names keep the roomy segments.
        assert_eq!(seg_item(46.0, 4, 452.0, 1.0), 74.0);
        assert_eq!(seg_item(14.0, 3, 452.0, 1.0), 46.0, "5, 8, 12: the smallest segment");
        // Long ones close in, to seven tenths of the row, but never past their label.
        let item = seg_item(60.0, 4, 452.0, 1.0);
        assert!(item < 88.0 && item * 4.0 + 4.0 <= 452.0 * 0.7 + 0.01);
        assert_eq!(seg_item(75.0, 4, 452.0, 1.0), 87.0, "no closer than 6 px either side of the label");
        assert_eq!(seg_item(100.0, 4, 452.0, 1.0), 112.0);
        // The same at twice the scale.
        assert_eq!(seg_item(150.0, 4, 904.0, 2.0), seg_item(75.0, 4, 452.0, 1.0) * 2.0);
    }

    /// The Notes page's font row, in every language and at 100% to 200%: the font names fit
    /// their segments and the row's own label fits beside them.
    #[test]
    fn the_font_row_fits_in_every_language() {
        use crate::common::i18n;
        for lang in i18n::languages() {
            i18n::init(&lang.tag);
            for s in [1.0f32, 1.25, 1.5, 2.0] {
                let seg = Font::new(12.5 * s, FW_NORMAL, theme::FACE_TEXT);
                let label = Font::new(13.5 * s, FW_NORMAL, theme::FACE_TEXT);
                let names = [t!("font_system"), t!("font_serif"), t!("font_mono"), t!("font_hand")];
                let widest = names.iter().map(|n| seg.measure(n).0).max().unwrap() as f32;
                let row_w = (W - SIDE_W - 2.0 * PAD) * s;
                let item = seg_item(widest, names.len(), row_w, s);
                assert!(item >= widest, "{} at {s}: a font name is cut", lang.tag);
                let control = item * names.len() as f32 + 4.0 * s;
                let room = row_w - 2.0 * INSET * s - control - 12.0 * s;
                assert!(label.measure(t!("settings_font")).0 as f32 <= room, "{} at {s}: \"{}\" has {room} px", lang.tag, t!("settings_font"));
            }
        }
        i18n::init("en");
    }

    /// Every row of keycaps on the Shortcuts page, in every language and at 100% and
    /// 150%: its name fits beside its keys, never cut short ("Alinear a la izquierda, al
    /// centro o a la dere…").
    #[test]
    fn shortcut_names_fit_beside_their_keys() {
        use crate::common::i18n;
        let mut cut = Vec::new();
        for lang in i18n::languages() {
            i18n::init(&lang.tag);
            for s in [1.0f32, 1.5] {
                let fonts = Fonts::new(s);
                let row_w = (W - SIDE_W - 2.0 * PAD) * s;
                for g in page(3, &Edit::default()) {
                    for row in &g.rows {
                        let Ctl::Keys(ks) = &row.ctl else { continue };
                        let room = row_w - 2.0 * INSET * s - caps_width(&fonts, s, ks) - 12.0 * s;
                        let need = fonts.label.measure(row.label).0 as f32;
                        if need > room {
                            cut.push(format!("{} at {s}: \"{}\" needs {need} px, has {room}", lang.tag, row.label));
                        }
                    }
                }
            }
        }
        i18n::init("en");
        assert!(cut.is_empty(), "{cut:#?}");
    }
}
