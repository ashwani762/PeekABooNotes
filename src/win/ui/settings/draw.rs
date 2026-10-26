// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Painting Settings: sidebar, groups, controls and the note preview.

use super::*;
use crate::win::ui::focus;

thread_local! {
    /// The preview's fonts for the current typography and scale. Made on every paint, each
    /// new text format left fresh layouts in DirectWrite's cache, frame after frame.
    static PREVIEW: RefCell<Option<(PreviewKey, Font, Font)>> = const { RefCell::new(None) };
}

type PreviewKey = (&'static str, &'static str, i32, bool, u32);

pub(super) fn forget_preview() {
    PREVIEW.with(|p| p.borrow_mut().take());
}

pub(super) fn paint_now(h: HWND) {
    unsafe {
        let dc = GetDC(h);
        with(|w| {
            let (cw, ch) = client_size(w.hwnd);
            if w.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
                w.canvas = Some(Canvas::new(cw, ch));
            }
            let mut c = w.canvas.take().unwrap();
            draw(w, &mut c);
            c.make_opaque();
            c.blit(dc, 0, 0);
            w.canvas = Some(c);
        });
        ReleaseDC(h, dc);
        ValidateRect(h, std::ptr::null());
    }
}

pub(super) fn draw(w: &Win, c: &mut Canvas) {
    let dark = app::dark();
    let p = pal(dark);
    let s = w.s;
    let f = &w.fonts;
    let px = |v: f32| (v * s).round() as i32;
    let (cw, ch) = (c.w, c.h);
    c.clear(0xFF00_0000 | p.bg);

    // Content first, so the page fade can blend it (text included) toward the background.
    let a = clamp01(w.page_a.value);
    let rise = (1.0 - a) * 8.0 * s;
    c.begin_batch();
    let (x0, _) = content_x(w);
    let groups = groups(w);
    for it in place(w, &groups) {
        match it {
            Item::Header(t, base) => {
                c.line_ct(t, &f.header, (x0 + 2.0 * s) as i32, base + rise as i32, px(300.0), None, p.sub, DT_LEFT);
            }
            Item::Footer(t, base) => {
                let (_, cwid) = content_x(w);
                let (x, top, wid) = ((x0 + 2.0 * s) as i32, base + rise as i32 - f.sub.ascent, cwid as i32 - (4.0 * s) as i32);
                let h = f.sub.wrapped_height(t, wid);
                c.text_ct(t, &f.sub, rect(x, top, wid, h), None, p.sub, DT_LEFT | DT_WORDBREAK);
            }
            Item::Box(mut sh) => {
                sh.y += rise;
                c.shape(&sh, p.group, 1.0, 1.0, 0.0);
                if !dark {
                    c.shape(&sh, 0x000000, 0.06, 1.0, 1.0 * s);
                }
            }
            Item::Row(pl) => draw_row(w, c, &p, &pl, rise),
        }
    }
    c.flush();
    // A long page (Shortcuts) scrolls under its title, which stays put with the close
    // button; the content fades out at whichever edge has more of it, by a slim thumb.
    let max = max_scroll(w);
    if max > 0.0 {
        let left = (SIDE_W * s) as i32;
        let band = header_band(w) as i32;
        let edge = 20.0 * s;
        let fade = |c: &mut Canvas, past: f32, top: bool| {
            let k = (past / (12.0 * s)).clamp(0.0, 1.0);
            for i in 0..edge as i32 {
                let keep = 1.0 - k * (1.0 - i as f32 / edge).powi(2);
                let y = if top { band + i } else { ch - 1 - i };
                c.fade_rect(rect(left, y, cw - left, 1), p.bg, keep);
            }
        };
        c.fade_rect(rect(left, 0, cw - left, band), p.bg, 0.0);
        fade(c, w.scroll.value, true);
        fade(c, max - w.scroll.value, false);
        let (top, track) = (band as f32 + 4.0 * s, ch as f32 - band as f32 - 12.0 * s);
        let thumb = (track * ch as f32 / (ch as f32 + max)).max(28.0 * s);
        let ty = top + (track - thumb) * (w.scroll.value / max).clamp(0.0, 1.0);
        c.shape(&Shape::new(cw as f32 - 7.0 * s, ty, 3.0 * s, thumb, 1.5 * s), p.sub, 0.45, 1.0, 0.0);
    }
    c.begin_batch();
    c.line_ct(crate::common::i18n::get(PAGES[w.page].0), &f.title, x0 as i32, (PAGE_BASE * s + rise) as i32, px(300.0), None, p.text, DT_LEFT);
    c.flush();
    c.fade_rect(rect((SIDE_W * s) as i32, 0, cw, ch), p.bg, a);

    // Sidebar.
    c.begin_batch();
    c.rrect(0.0, 0.0, SIDE_W * s, ch as f32, 0.0, p.side, 1.0);
    c.rrect(SIDE_W * s - 1.0, 0.0, 1.0, ch as f32, 0.0, p.line, 1.0);
    // The sidebar's own title, on the page title's baseline.
    let tw = (SIDE_W * s) as i32 - px(32.0);
    c.line_ct(&f.side_title.fit(t!("settings_title"), tw), &f.side_title, px(16.0), (PAGE_BASE * s) as i32, tw, None, p.text, DT_LEFT);
    let hover_a = clamp01(w.hover_a.value);
    if hover_a > 0.01 {
        let mut sh = side_item(w, 0);
        sh.y = SIDE_TOP * s + w.hover_y.value;
        c.shape(&sh, p.hover.0, p.hover.1 * hover_a, 1.0, 0.0);
    }
    let mut sel = side_item(w, 0);
    sel.y = SIDE_TOP * s + w.side_y.value;
    c.shape(&sel, if dark { 0xFFFFFF } else { 0x000000 }, if dark { 0.10 } else { 0.075 }, 1.0, 0.0);
    if ring_on(w) == Some(Focus::Pages) {
        focus::ring(c, &sel, p.accent, 1.0, s);
    }
    for (i, (name, glyph, tint)) in PAGES.iter().enumerate() {
        let it = side_item(w, i);
        let tile = Shape::new(it.x + 6.0 * s, it.y + (it.h - 22.0 * s) / 2.0, 22.0 * s, 22.0 * s, 6.0 * s);
        c.shape(&tile, *tint, 1.0, 1.0, 0.0);
        let tr = rect(tile.x as i32, tile.y as i32, tile.w as i32, tile.h as i32);
        c.text_ct(glyph, &f.tile, tr, None, 0xFFFFFF, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        let base = (it.y + it.h / 2.0) as i32 + f.side.cap / 2;
        let lx = tile.x + tile.w + 10.0 * s;
        let label = f.side.fit(crate::common::i18n::get(name), (it.x + it.w - lx - 8.0 * s) as i32);
        c.line_ct(&label, &f.side, lx as i32, base, (it.x + it.w - lx) as i32, None, p.text, DT_LEFT);
    }
    // About, at the foot of the sidebar.
    let ab = ch - px(38.0);
    c.line_ct(t!("app_name"), &f.about, px(20.0), ab, px(160.0), None, p.text, DT_LEFT);
    c.line_ct(&t!("settings_version", env!("CARGO_PKG_VERSION")), &f.about_sub, px(20.0), ab + px(17.0), px(160.0), None, p.sub, DT_LEFT);

    // Close button.
    let r = close_rect(w);
    let hot = w.hover == Hit::Close;
    let mut bg = p.bg;
    if hot {
        bg = 0xE81123;
        c.rrect(r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32, 0.0, bg, 1.0);
    }
    c.text_ct("\u{E8BB}", &f.chrome, r, None, if hot { 0xFFFFFF } else { mix(bg, p.text, 0.9) }, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    c.flush();
}

pub(super) fn draw_row(w: &Win, c: &mut Canvas, p: &Pal, pl: &Placed, rise: f32) {
    let s = w.s;
    let f = &w.fonts;
    let mut r = pl.r;
    r.top += rise as i32;
    r.bottom += rise as i32;
    let lx = r.left + (INSET * s) as i32;
    if !pl.first {
        c.rrect(lx as f32, r.top as f32, (r.right - lx) as f32, 1.0, 0.0, p.line, 1.0);
    }
    let row = pl.row;
    let mid = (r.top + r.bottom) / 2;
    let label_ink = if row.off || row.disabled { p.sub } else { p.text };
    match &row.sub {
        Some(sub) => {
            // As on a row of one subtitle line, however many lines it wraps onto.
            let mid1 = r.top + (ROW_SUB_H * s / 2.0) as i32;
            c.line_ct(row.label, &f.label, lx, mid1 - (4.0 * s) as i32, (r.right - lx) - px_ctl(w, row, &r), None, label_ink, DT_LEFT);
            let sw = sub_width(w, row, &r);
            if sub_wraps(row) {
                let top = mid1 + (15.0 * s) as i32 - f.sub.ascent;
                let h = f.sub.wrapped_height(sub, sw);
                c.text_ct(sub, &f.sub, rect(lx, top, sw, h), None, p.sub, DT_LEFT | DT_WORDBREAK);
                return draw_ctl(w, c, p, pl, &r, mid);
            }
            // The colour row names its choice, previewing whichever swatch is under the pointer.
            let named;
            let sub = match (&row.ctl, row.key) {
                (Ctl::Swatches, Some(k)) => {
                    let i = match w.hover {
                        Hit::Ctl(hk, i) if hk == k => i,
                        _ => value(w, k),
                    };
                    named = if i == 0 { t!("color_automatic").to_string() } else { crate::common::i18n::get(&format!("color_{}", theme::color(theme::ORDER[i - 1]).name.to_lowercase())).to_string() };
                    named.as_str()
                }
                _ => sub.as_str(),
            };
            let fitted = if row.key == Some(Key::Folder) { f.sub.fit_path(sub, sw.max(10)) } else { f.sub.fit(sub, sw.max(10)) };
            c.line_ct(&fitted, &f.sub, lx, mid1 + (15.0 * s) as i32, sw, None, p.sub, DT_LEFT);
        }
        None => {
            let lw = (r.right - lx) - px_ctl(w, row, &r) - (12.0 * s) as i32;
            let label = f.label.fit(row.label, lw.max(10));
            c.line_ct(&label, &f.label, lx, mid + f.label.cap / 2, lw.max(10), None, label_ink, DT_LEFT);
            // What the row says for a moment, between its label and its control.
            if let Some(hint) = &row.hint {
                let x = lx + f.label.measure(&label).0 + (16.0 * s) as i32;
                let room = lx + lw - x;
                if room > (24.0 * s) as i32 {
                    let fitted = f.sub.fit(hint, room);
                    let hw = f.sub.measure(&fitted).0;
                    c.line_ct(&fitted, &f.sub, lx + lw - hw, mid + f.sub.cap / 2, hw + 2, None, p.sub, DT_LEFT);
                }
            }
        }
    }
    draw_ctl(w, c, p, pl, &r, mid);
}

/// Row `pl`'s control, drawn at `r` (risen with the page) around its middle `mid`.
fn draw_ctl(w: &Win, c: &mut Canvas, p: &Pal, pl: &Placed, r: &RECT, mid: i32) {
    let (s, f, row) = (w.s, &w.fonts, pl.row);
    let r = *r;
    let knob = row.key.and_then(|k| w.knobs.get(&k)).map(|sp| sp.value).unwrap_or(0.0);
    let pressed = |sub: usize| row.key.is_some_and(|k| w.pressed == Hit::Ctl(k, sub) && w.hover == w.pressed);
    let hovered = |sub: usize| row.key.is_some_and(|k| w.hover == Hit::Ctl(k, sub));
    let focused = row.key.is_some_and(|k| ring_on(w) == Some(Focus::Ctl(k)));
    // A greyed-out setting's control, faded into its group.
    let fade = if row.disabled { 0.4 } else { 1.0 };
    let dim = |ink: u32| mix(p.group, ink, fade);
    let ring = |c: &mut Canvas, sh: &Shape| {
        if focused {
            focus::ring(c, sh, p.accent, 1.0, s);
        }
    };
    match &row.ctl {
        Ctl::Toggle => {
            let t = clamp01(knob);
            let sh = toggle_shape(w, &r);
            c.shape(&sh, mix(p.track, p.accent, t), fade, 1.0, 0.0);
            ring(c, &sh);
            // The knob stretches while pressed, like a finger squashing it.
            let d = sh.h - 4.0 * s;
            let kw = d + if pressed(0) { 6.0 * s * clamp01(w.press.value) } else { 0.0 };
            let kx = sh.x + 2.0 * s + t * (sh.w - 4.0 * s - kw);
            let k = Shape::new(kx, sh.y + 2.0 * s, kw, d, d / 2.0);
            c.card(&k, dim(0xFFFFFF), 1.0, 0x000000, 0.04 * fade, 0.5 * fade, s);
        }
        Ctl::Seg(labels) => {
            let (sh, item) = seg_metrics(w, labels, &r);
            c.shape(&sh, p.field.0, p.field.1 * fade, 1.0, 0.0);
            ring(c, &sh);
            let pos = knob.clamp(0.0, labels.len() as f32 - 1.0);
            let pill = Shape::new(sh.x + 2.0 * s + pos * item, sh.y + 2.0 * s, item, sh.h - 4.0 * s, 6.5 * s);
            c.card(&pill, dim(p.pill), 1.0, 0x000000, 0.04 * fade, 0.35 * fade, s);
            let on = row.key.map(|k| value(w, k)).unwrap_or(0);
            for (i, l) in labels.iter().enumerate() {
                let x = (sh.x + 2.0 * s + i as f32 * item) as i32;
                let font = if i == on { &f.seg_on } else { &f.seg };
                let ink = if i == on || hovered(i) { p.text } else { mix(p.group, p.text, 0.78) };
                c.line_ct(l, font, x, mid + font.cap / 2, item as i32, None, dim(ink), DT_CENTER);
            }
        }
        Ctl::Swatches => {
            for i in 0..=theme::ORDER.len() {
                let (x, y) = swatch_x(w, &r, i);
                let d = SWATCH * s;
                let grow = if hovered(i) { 1.0 * s } else { 0.0 };
                let sh = Shape::new(x - grow, y - grow, d + 2.0 * grow, d + 2.0 * grow, d / 2.0 + grow);
                if i == 0 {
                    // Automatic: a colour wheel of every paper, in display order.
                    let wheel: Vec<u32> = theme::ORDER.iter().map(|&o| theme::color(o).accent).collect();
                    c.conic_disc(sh.x + sh.w / 2.0, sh.y + sh.h / 2.0, sh.w / 2.0, &wheel);
                } else {
                    c.shape(&sh, theme::color(theme::ORDER[i - 1]).accent, 1.0, 1.0, 0.0);
                }
            }
            // The selection ring glides between swatches.
            let n = theme::ORDER.len() as f32;
            let pos = knob.clamp(0.0, n);
            let (x, y) = swatch_x(w, &r, 0);
            let step = (SWATCH + SWATCH_GAP) * s;
            let ring = Shape::new(x + pos * step - 3.0 * s, y - 3.0 * s, SWATCH * s + 6.0 * s, SWATCH * s + 6.0 * s, SWATCH * s / 2.0 + 3.0 * s);
            c.shape(&ring, p.text, 0.55, 1.0, 1.6 * s);
            if focused {
                let (x, y) = swatch_x(w, &r, w.swatch_k);
                focus::ring(c, &Shape::new(x, y, SWATCH * s, SWATCH * s, SWATCH * s / 2.0), p.accent, 1.0, s);
            }
        }
        Ctl::Button(label) => {
            let mut sh = button_shape(w, label, &r);
            let k = if pressed(0) { 2.4 } else if hovered(0) { 1.6 } else { 1.0 };
            // After an import / export the button briefly answers with a tick (or a
            // cross) and a count, then turns back. It grows when the answer needs the
            // room, but never shrinks: a button that jumps narrower looks like a glitch.
            let result = match &w.result {
                Some((rk, ok, text)) if row.key == Some(*rk) && w.result_a.value > 0.001 => Some((*ok, text.as_str(), clamp01(w.result_a.value))),
                _ => None,
            };
            let glyph_w = 16.0 * s;
            if let Some((_, text, t)) = result {
                let want = f.button.measure(text).0 as f32 + glyph_w + 30.0 * s;
                let grow = (want - sh.w).max(0.0) * t;
                sh.x -= grow;
                sh.w += grow;
            }
            // Delete All is red: tinted at rest, solid when aimed at, and dimmed with
            // nothing left to delete.
            let danger = row.key == Some(Key::DeleteAll);
            let empty = danger && app::store(|s| s.d.notes.is_empty());
            // Reset to Defaults is dimmed while every shortcut has its default keys.
            let idle = row.key == Some(Key::ResetKeys) && app::hotkeys_default();
            let (fill, fa, ink) = match (danger, empty) {
                _ if idle => (p.field.0, p.field.1, p.sub),
                (true, true) => (p.field.0, p.field.1, p.sub),
                (true, false) if hovered(0) => (p.danger.0, if pressed(0) { 0.82 } else { 1.0 }, 0xFFFFFF),
                (true, false) => (p.danger.0, 0.10, p.danger.1),
                _ if row.disabled => (p.field.0, p.field.1 * fade, dim(p.text)),
                _ => (p.field.0, p.field.1 * k, p.text),
            };
            c.shape(&sh, fill, fa, 1.0, 0.0);
            ring(c, &sh);
            // Cross-fade by mixing toward the button's own colour (the canvas is opaque).
            let face = mix(p.group, fill, fa.min(1.0));
            // One after the other, never overlapping: the label is gone by the time the
            // result appears (and the result is gone before the label returns), so no
            // text ever spills past the pill while it resizes.
            let t = result.map_or(0.0, |r| r.2);
            let label_t = clamp01(1.0 - t / 0.4);
            if label_t > 0.001 {
                c.line_ct(label, &f.button, sh.x as i32, mid + f.button.cap / 2, sh.w as i32, None, mix(face, ink, label_t), DT_CENTER);
            }
            if let Some((ok, text, t)) = result.map(|(ok, text, t)| (ok, text, clamp01((t - 0.6) / 0.4))).filter(|r| r.2 > 0.001) {
                let tw = f.button.measure(text).0 as f32;
                let x = sh.x + (sh.w - tw - glyph_w) / 2.0;
                let (glyph, tint) = if ok { ("\u{E73E}", 0x34C759) } else { ("\u{E711}", 0xFF3B30) };
                let gr = rect(x as i32, sh.y as i32, glyph_w as i32, sh.h as i32);
                c.text_ct(glyph, &f.result_glyph, gr, None, mix(face, tint, t), DT_LEFT | DT_VCENTER | DT_SINGLELINE);
                c.line_ct(text, &f.button, (x + glyph_w) as i32, mid + f.button.cap / 2, tw as i32 + 4, None, mix(face, p.text, t), DT_LEFT);
            }
        }
        Ctl::Preview => draw_preview(w, c, p, &r),
        Ctl::Choice => {
            let key = row.key.unwrap_or(Key::Language);
            let sh = choice_shape(w, key, &r);
            let k = if pressed(0) { 2.4 } else if hovered(0) { 1.6 } else { 1.0 };
            c.shape(&sh, p.field.0, p.field.1 * k * fade, 1.0, 0.0);
            ring(c, &sh);
            let label = choice_label(key);
            // A chosen display that isn't connected reads as out of reach.
            let ink = if key == Key::DeckDisplay && display_away() { p.sub } else { p.text };
            c.line_ct(&label, &f.button, (sh.x + 12.0 * s) as i32, mid + f.button.cap / 2, (sh.w - 36.0 * s) as i32, None, dim(ink), DT_LEFT);
            let chev = rect((sh.x + sh.w - 26.0 * s) as i32, sh.y as i32, (16.0 * s) as i32, sh.h as i32);
            c.text_ct("\u{E70D}", &f.chrome, chev, None, dim(p.sub), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
        Ctl::Keys(ks) => draw_caps(w, c, p, ks, row.off, r.right as f32 - INSET * s, mid),
        Ctl::Hotkey(i) => {
            let sh = hotkey_shape(w, *i, &r);
            let waiting = w.edit.rec == Some(*i);
            // A field round the keys, as the pointer or the keys come to it; while it waits
            // for keys, tinted and outlined in the accent.
            if waiting {
                c.shape(&sh, p.accent, 0.10, 1.0, 0.0);
                c.shape(&sh, p.accent, 0.85, 1.0, 1.5 * s);
            } else if hovered(0) || focused {
                c.shape(&sh, p.field.0, p.field.1 * if pressed(0) { 2.4 } else { 1.6 }, 1.0, 0.0);
            }
            ring(c, &sh);
            let right = r.right as f32 - INSET * s;
            let words = |c: &mut Canvas, text: &str, ink: u32| {
                let tw = f.seg.measure(text).0 as f32;
                c.line_ct(text, &f.seg, (right - tw) as i32, mid + f.seg.cap / 2, tw as i32 + 2, None, ink, DT_LEFT);
                tw
            };
            if waiting {
                // The modifiers held so far, then the prompt.
                let tw = words(c, t!("shortcut_press"), p.accent);
                let held = hotkey::modifier_parts(w.held);
                if !held.is_empty() {
                    draw_caps(w, c, p, &held, false, right - tw - HELD_GAP * s, mid);
                }
            } else {
                match app::hotkey_keys(*i) {
                    Some(k) => draw_caps(w, c, p, &k.parts(), row.off, right, mid),
                    None => {
                        words(c, t!("shortcut_none"), p.sub);
                    }
                }
            }
        }
    }
}

/// Keycaps `ks`, ending at `right` around the row's middle `mid`; "/" between two
/// alternatives is plain text. Flat and grey when the shortcut is `off`.
fn draw_caps(w: &Win, c: &mut Canvas, p: &Pal, ks: &[&str], off: bool, right: f32, mid: i32) {
    let (s, f) = (w.s, &w.fonts);
    let mut x = right;
    let h = 22.0 * s;
    let y = mid as f32 - h / 2.0;
    for k in ks.iter().rev() {
        if *k == "/" {
            // "or" between two alternatives: plain text, not a keycap.
            let kw = f.key.measure("/").0 as f32 + 8.0 * s;
            x -= kw;
            c.line_ct("/", &f.key, x as i32, mid + f.key.cap / 2, kw as i32, None, p.sub, DT_CENTER);
            x -= 5.0 * s;
            continue;
        }
        let k = key_name(k);
        let kw = (f.key.measure(k).0 as f32 + 14.0 * s).max(h);
        x -= kw;
        let cap = Shape::new(x, y, kw, h, 5.0 * s);
        // A keycap: a darker lip under a lighter face; flat and grey when it's off.
        if !off {
            c.shape(&Shape::new(cap.x, cap.y + 1.0 * s, cap.w, cap.h, cap.r), p.text, 0.10, 1.0, 0.0);
        }
        c.shape(&cap, mix(p.group, p.text, if off { 0.04 } else { 0.06 }), 1.0, 1.0, 0.0);
        c.line_ct(k, &f.key, x as i32, mid + f.key.cap / 2, kw as i32, None, if off { p.sub } else { p.text }, DT_CENTER);
        x -= 5.0 * s;
    }
}

/// A small note in the chosen typeface, size and colour, so choices can be judged
/// before opening a real note.
pub(super) fn draw_preview(w: &Win, c: &mut Canvas, p: &Pal, r: &RECT) {
    let s = w.s;
    let color = app::store(|st| st.d.settings.new_color).unwrap_or(theme::ORDER[0]);
    let paper = theme::paper(color, app::dark());
    let card = Shape::new(r.left as f32 + INSET * s, r.top as f32 + 14.0 * s, (r.right - r.left) as f32 - 2.0 * INSET * s, (r.bottom - r.top) as f32 - 28.0 * s, 12.0 * s);
    c.card(&card, paper, 1.0, 0x000000, 0.06, 0.8, s);
    let px = |twips: i32| twips as f32 / 20.0 * 96.0 / 72.0 * s;
    let (body_face, body) = (crate::win::editor::rich::body_face(), crate::win::editor::rich::body_size());
    let bold_title = !matches!(app::store(|st| st.d.settings.note_font), 0 | 3);
    let title_face = if app::store(|st| st.d.settings.note_font) == 0 { theme::FACE_DISPLAY } else { body_face };
    let key = (title_face, body_face, body, bold_title, s.to_bits());
    let (title, text) = PREVIEW.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().map(|(k, ..)| *k) != Some(key) {
            let title = Font::new(px(body * 13 / 10), if bold_title { FW_BOLD } else { FW_SEMIBOLD }, title_face);
            *p = Some((key, title, Font::new(px(body), FW_NORMAL, body_face)));
        }
        let (_, t, b) = p.as_ref().unwrap();
        (t.clone(), b.clone())
    });
    let x = (card.x + 20.0 * s) as i32;
    let tw = (card.w - 40.0 * s) as i32;
    let mut y = card.y as i32 + (18.0 * s) as i32 + title.ascent;
    c.line_ct(t!("preview_title"), &title, x, y, tw, None, theme::PAPER_TEXT, DT_LEFT);
    let step = ((text.ascent + text.descent) as f32 * 1.25) as i32;
    y += (title.descent as f32 + 8.0 * s) as i32 + text.ascent;
    c.line_ct(t!("preview_line"), &text, x, y, tw, None, theme::PAPER_TEXT, DT_LEFT);
    y += step;
    let ink = theme::paper_ink(color, app::dark());
    let d = (px(body) * 1.04).round();
    let ring = Shape::new(x as f32, y as f32 - text.cap as f32 * 0.44 - d / 2.0, d, d, d / 2.0);
    c.shape(&ring, mix(ink, paper, 0.42), 1.0, 1.0, 1.5 * s);
    let list_x = x + px(345) as i32;
    c.line_ct(t!("preview_task"), &text, list_x, y, tw - (list_x - x), None, theme::PAPER_TEXT, DT_LEFT);
    let _ = p;
}

/// A key's caption: lowercase names ("ctrl", "enter") are localised.
fn key_name(k: &str) -> &str {
    if k.chars().all(|c| c.is_ascii_lowercase()) { crate::common::i18n::get(&format!("key_{k}")) } else { k }
}

/// Width of a row of keycaps as drawn (caps, "/" separators and the gaps between).
pub(super) fn keys_width(w: &Win, ks: &[&str]) -> f32 {
    caps_width(&w.fonts, w.s, ks)
}

/// `keys_width` in fonts `f` at scale `s`.
pub(super) fn caps_width(f: &Fonts, s: f32, ks: &[&str]) -> f32 {
    let h = 22.0 * s;
    let caps: f32 = ks
        .iter()
        .map(|k| if *k == "/" { f.key.measure("/").0 as f32 + 8.0 * s } else { (f.key.measure(key_name(k)).0 as f32 + 14.0 * s).max(h) })
        .sum();
    caps + 5.0 * s * ks.len().saturating_sub(1) as f32
}

/// Room the row's control takes on the right (so labels never run under it).
pub(super) fn px_ctl(w: &Win, row: &Row, r: &RECT) -> i32 {
    let s = w.s;
    let left = match &row.ctl {
        Ctl::Toggle => toggle_shape(w, r).x,
        Ctl::Seg(l) => seg_metrics(w, l, r).0.x,
        Ctl::Swatches => swatch_x(w, r, 0).0,
        Ctl::Button(l) => button_shape(w, l, r).x,
        Ctl::Choice => choice_shape(w, row.key.unwrap_or(Key::Language), r).x,
        Ctl::Keys(ks) => r.right as f32 - INSET * s - keys_width(w, ks),
        Ctl::Hotkey(i) => hotkey_shape(w, *i, r).x,
        Ctl::Preview => r.right as f32,
    };
    r.right - left as i32
}
