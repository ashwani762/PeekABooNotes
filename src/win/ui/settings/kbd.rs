// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard focus: Tab through the page list and each row's control, and what keys do there.

use super::*;
use crate::win::ui::focus;

/// The control on this page for setting `k`.
fn control(groups: &[Group], k: Key) -> Option<&Ctl> {
    groups.iter().flat_map(|g| g.rows.iter()).find(|r| r.key == Some(k)).map(|r| &r.ctl)
}

/// The page list, then each control on the page, top to bottom. Keycaps and the preview
/// only show things, so Tab passes them by.
fn tab_order(w: &Win) -> Vec<Focus> {
    let groups = groups(w);
    let ctls = groups.iter().flat_map(|g| g.rows.iter()).filter(|r| !matches!(r.ctl, Ctl::Keys(_) | Ctl::Preview));
    std::iter::once(Focus::Pages).chain(ctls.filter_map(|r| r.key).map(Focus::Ctl)).collect()
}

/// The control wearing the focus ring: only once the keyboard has moved focus.
pub(super) fn ring_on(w: &Win) -> Option<Focus> {
    (w.cues && unsafe { GetFocus() } == w.hwnd).then_some(w.kbd)
}

/// Tab or Shift+Tab (`wrap`, coming round through the page list), or Up and Down among
/// the page's controls (stopping at the ends).
pub(super) fn move_focus(w: &mut Win, back: bool, wrap: bool) {
    let order = tab_order(w);
    let order = if wrap { &order[..] } else { &order[1..] };
    if let Some(f) = focus::step(order, Some(w.kbd), back, wrap, |_| true) {
        w.kbd = f;
        if let Focus::Ctl(k) = f {
            // Onto the chosen swatch, as a radio group starts at its choice.
            w.swatch_k = value(w, k);
            reveal(w, k);
        }
    }
    w.cues = true;
    anim::wake(Client::Settings);
}

/// Scroll a long page so the row of setting `k` is in full view.
fn reveal(w: &mut Win, k: Key) {
    let groups = groups(w);
    let Some(r) = place(w, &groups).into_iter().find_map(|it| match it {
        Item::Row(pl) if pl.row.key == Some(k) => Some(pl.r),
        _ => None,
    }) else {
        return;
    };
    let (s, (_, ch)) = (w.s, client_size(w.hwnd));
    // In page coordinates: `place` lays rows out where they're scrolled to now.
    let (top, bottom) = (r.top as f32 + w.scroll.value, r.bottom as f32 + w.scroll.value);
    let mut to = w.scroll.target;
    if top - to < header_band(w) + 8.0 * s {
        to = top - header_band(w) - 8.0 * s;
    } else if bottom - to > ch as f32 - PAD * s {
        to = bottom - ch as f32 + PAD * s;
    }
    w.scroll.to(to.clamp(0.0, max_scroll(w)));
}

/// Left or Right: a segmented control changes (the change to make), or focus moves
/// along the swatches.
pub(super) fn nudge(w: &mut Win, by: i32) -> Option<(Key, usize)> {
    let Focus::Ctl(k) = w.kbd else { return None };
    let groups = groups(w);
    w.cues = true;
    anim::wake(Client::Settings);
    match control(&groups, k)? {
        Ctl::Seg(labels) => {
            let v = value(w, k);
            let to = (v as i32 + by).clamp(0, labels.len() as i32 - 1) as usize;
            (to != v).then_some((k, to))
        }
        Ctl::Swatches => {
            w.swatch_k = (w.swatch_k as i32 + by).clamp(0, theme::ORDER.len() as i32) as usize;
            None
        }
        _ => None,
    }
}

/// Space or Enter: a switch flips, a button or the language choice is pressed, the
/// swatch focus is on is chosen.
pub(super) fn activate(w: &mut Win) -> Option<(Key, usize)> {
    let Focus::Ctl(k) = w.kbd else { return None };
    let groups = groups(w);
    match control(&groups, k)? {
        Ctl::Toggle => click(w, Hit::Ctl(k, 0)),
        Ctl::Button(_) | Ctl::Choice => Some((k, 0)),
        Ctl::Swatches => Some((k, w.swatch_k)),
        _ => None,
    }
}

/// Where a menu from setting `k`'s button drops, when the keyboard pressed it: under the
/// button, not at the pointer, which may be anywhere.
pub(super) fn menu_anchor(w: &Win, k: Key) -> Option<POINT> {
    if !w.cues {
        return None;
    }
    let groups = groups(w);
    let sh = place(w, &groups).into_iter().find_map(|it| match it {
        Item::Row(pl) if pl.row.key == Some(k) => match pl.row.ctl {
            Ctl::Button(label) => Some(button_shape(w, label, &pl.r)),
            _ => None,
        },
        _ => None,
    })?;
    let mut p = POINT { x: sh.x as i32, y: (sh.y + sh.h + 4.0 * w.s) as i32 };
    unsafe { ClientToScreen(w.hwnd, &mut p) };
    Some(p)
}
