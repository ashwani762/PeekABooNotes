// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note's scroll bar: a slim thumb in the right margin, as on macOS. It shows up while
//! the text scrolls (and briefly when a long note opens), widens under the pointer, can be
//! dragged or clicked to jump, and isn't there at all when everything fits.

use super::*;

pub(super) const T_BAR: usize = 2;

/// How long the bar stays after the text stops moving.
const LINGER_MS: u32 = 1100;
const EM_GETLINECOUNT: u32 = 0x00BA;
const EM_LINEINDEX: u32 = 0x00BB;

/// (scrolled, content, view) heights in px, or None when the text fits.
fn metrics(edit: HWND) -> Option<(f32, f32, f32)> {
    let m = measure(edit);
    (m.1 > m.2 + 1.0).then_some(m)
}

fn measure(edit: HWND) -> (f32, f32, f32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(edit, &mut rc) };
    let scroll = rich::scroll_y(edit) as f32;
    // Where the text really ends; failing that, the last line taken as tall as the one
    // before (which a picture at the end isn't).
    let bottom = rich::content_bottom(edit).unwrap_or_else(|| {
        let (last, line_h) = last_line(edit);
        last + line_h.unwrap_or(rc.bottom)
    });
    (scroll, scroll + bottom as f32, rc.bottom as f32)
}

/// The last line's top (in the editor) and its height, taken as the line before it's:
/// both body text, nearly always. No height for a one-line note.
fn last_line(edit: HWND) -> (i32, Option<i32>) {
    let n = rich::send(edit, EM_GETLINECOUNT, 0, 0) as i32;
    let top_of = |line: i32| rich::pos_from_char(edit, rich::send(edit, EM_LINEINDEX, line.max(0) as usize, 0) as i32).y;
    let last = top_of(n - 1);
    (last, (n > 1).then(|| (last - top_of(n - 2)).max(1)))
}

/// When the caret is out of view: the start of the first line wholly in view, to put it
/// at instead.
pub(super) fn caret_out_of_view(edit: HWND) -> Option<i32> {
    const EM_EXLINEFROMCHAR: u32 = 0x0400 + 54;
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(edit, &mut rc) };
    let caret = rich::sel(edit).1;
    let y = rich::pos_from_char(edit, caret).y;
    if y >= 0 && y < rc.bottom - 4 {
        return None;
    }
    let first = rich::char_from_pos(edit, 1, 1);
    let line = rich::send(edit, EM_EXLINEFROMCHAR, 0, first as isize) as i32;
    let start = rich::send(edit, EM_LINEINDEX, line.max(0) as usize, 0) as i32;
    // That line partly scrolled out at the top: the next one.
    Some(if rich::pos_from_char(edit, start).y < 0 { rich::send(edit, EM_LINEINDEX, (line + 1) as usize, 0) as i32 } else { start }.max(0))
}

/// Nudge the scroll so the caret's line is wholly in view. RichEdit insists on that the
/// moment it next paints or takes focus: a view restored with that line half under the top
/// (or bottom) would jump by the hidden part right after the note appears.
pub(super) fn keep_caret_in_view(edit: HWND) {
    const EM_EXLINEFROMCHAR: u32 = 0x0400 + 54;
    let caret = rich::sel(edit).1;
    let line = rich::send(edit, EM_EXLINEFROMCHAR, 0, caret as isize) as i32;
    let top = rich::pos_from_char(edit, caret).y;
    let n = rich::send(edit, EM_GETLINECOUNT, 0, 0) as i32;
    let line_h = if line + 1 < n {
        rich::pos_from_char(edit, rich::send(edit, EM_LINEINDEX, (line + 1) as usize, 0) as i32).y - top
    } else {
        last_line(edit).1.unwrap_or(0)
    };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(edit, &mut rc) };
    let scroll = rich::scroll_y(edit);
    if top < 0 {
        rich::set_scroll_y(edit, scroll + top);
    } else if top + line_h > rc.bottom {
        rich::set_scroll_y(edit, scroll + (top + line_h - rc.bottom).min(top));
    }
}

/// Scrolled as far as the text goes: its last line at the bottom (0 when it all fits).
pub(super) fn max_scroll(edit: HWND) -> i32 {
    let (_, content, view) = measure(edit);
    (content - view).max(0.0).round() as i32
}

/// The wheel anywhere on the note. RichEdit only takes it with the pointer over its own
/// text, so the window scrolls it by the user's lines per notch, never past either end.
pub(super) fn wheel(h: HWND, delta: i16) {
    let Some(st) = state(h) else { return };
    let (scroll, content, view) = measure(st.edit);
    let line_h = last_line(st.edit).1.unwrap_or((20.0 * st.s) as i32) as f32;
    let mut lines: u32 = 3;
    unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, &mut lines as *mut _ as _, 0) };
    // "One screen per notch" comes back as u32::MAX.
    let step = if lines == u32::MAX { view } else { lines as f32 * line_h };
    // Notches add up while the text is still gliding, so a fast spin goes further.
    let from = if st.glide.settled() { scroll } else { st.glide.target };
    // RichEdit's own scroll-to-caret can rest a pixel past our measured end: that's the
    // end too, or every notch there would nudge the text by that pixel and back.
    let mut max = (content - view).max(0.0);
    if scroll > max && scroll - max <= 2.0 {
        max = scroll;
    }
    let to = (from - delta as f32 / 120.0 * step).clamp(0.0, max);
    if crate::win::render::anim::reduced() {
        rich::set_scroll_y(st.edit, to.round() as i32);
        return;
    }
    if st.glide.settled() {
        st.glide.snap(scroll);
    }
    st.glide.to(to);
    crate::win::render::anim::wake(crate::win::render::anim::Client::Notes);
}

/// Advance a wheel glide. True while it's still moving.
pub(super) fn step(st: &mut NoteWin, dt: f32) -> bool {
    if st.glide.settled() {
        return false;
    }
    let moving = st.glide.step(dt);
    let y = if moving { st.glide.value } else { st.glide.target };
    rich::set_scroll_y(st.edit, y.round() as i32);
    moving
}

/// Typing (which scrolls to the caret) takes over from a glide.
pub(super) fn interrupt(h: HWND) {
    if let Some(st) = state(h) {
        stop_glide(st);
    }
}

/// Anything else that moves the text (a drag, typing) takes over from a glide.
fn stop_glide(st: &mut NoteWin) {
    let at = st.glide.value;
    st.glide.snap(at);
}

/// The track (top, height) beside the editor, in the note's client coordinates.
fn track(h: HWND, st: &NoteWin) -> (f32, f32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetWindowRect(st.edit, &mut r);
        let mut p = POINT { x: r.left, y: r.top };
        ScreenToClient(h, &mut p);
        let pad = 2.0 * st.s;
        (p.y as f32 + pad, (r.bottom - r.top) as f32 - 2.0 * pad)
    }
}

/// The thumb as (x, y, w, h), when there is anything to scroll.
fn thumb(h: HWND, st: &NoteWin) -> Option<(f32, f32, f32, f32)> {
    let (scroll, content, view) = metrics(st.edit)?;
    let (top, len) = track(h, st);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let s = st.s;
    let th = (len * view / content).max(28.0 * s).min(len);
    let y = top + (len - th) * (scroll / (content - view)).clamp(0.0, 1.0);
    let w = crate::win::render::anim::lerp(4.0 * s, 7.0 * s, st.bar_wide.value);
    Some((rc.right as f32 - 3.0 * s - w, y, w, th))
}

/// The pointer is over the bar's column (beside the text, along the track).
pub(super) fn on_bar(h: HWND, x: i32, y: i32) -> bool {
    let Some(st) = state(h) else { return false };
    if metrics(st.edit).is_none() {
        return false;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let (top, len) = track(h, st);
    x >= rc.right - (12.0 * st.s) as i32 && (y as f32) >= top && (y as f32) < top + len
}

/// Show the bar now and hide it again `LINGER_MS` after the last call.
fn show(h: HWND, st: &mut NoteWin) {
    st.bar_hold = true;
    st.bar.to(1.0);
    unsafe { SetTimer(h, T_BAR, LINGER_MS, None) };
    crate::win::render::anim::wake(crate::win::render::anim::Client::Notes);
}

fn settle(st: &mut NoteWin) {
    st.bar.to(if st.bar_hold || st.bar_hot || st.bar_drag.is_some() { 1.0 } else { 0.0 });
    st.bar_wide.to(if st.bar_hot || st.bar_drag.is_some() { 1.0 } else { 0.0 });
    crate::win::render::anim::wake(crate::win::render::anim::Client::Notes);
}

/// After anything that may have scrolled or resized the text: show the bar while it moves
/// and keep the thumb in step.
pub(super) fn sync(h: HWND) {
    let Some(st) = state(h) else { return };
    // RichEdit lets the wheel (and deleting at the end) leave the text scrolled past its
    // last line, over blank paper: never further than the last line at the bottom.
    let (scroll, content, view) = measure(st.edit);
    let max = (content - view).max(0.0);
    if scroll > max + 8.0 * st.s {
        rich::set_scroll_y(st.edit, max as i32);
        return; // it repaints, and that brings us back here
    }
    let m = metrics(st.edit).map(|(s, c, _)| (s as i32, c as i32));
    if m == st.bar_seen {
        return;
    }
    let scrolled = matches!((st.bar_seen, m), (Some(a), Some(b)) if a.0 != b.0);
    st.bar_seen = m;
    if scrolled {
        show(h, st);
    }
    unsafe { InvalidateRect(h, std::ptr::null(), 0) };
}

/// A long note just opened: flash the bar so it's clear there's more above.
pub(super) fn hint(h: HWND) {
    if let Some(st) = state(h)
        && metrics(st.edit).is_some()
    {
        st.bar_seen = metrics(st.edit).map(|(s, c, _)| (s as i32, c as i32));
        show(h, st);
    }
}

pub(super) fn timer(h: HWND) {
    unsafe { KillTimer(h, T_BAR) };
    if let Some(st) = state(h) {
        st.bar_hold = false;
        settle(st);
    }
}

/// Pointer moved over the note: hover and drag. True when the bar handled it.
pub(super) fn mouse_move(h: HWND, x: i32, y: i32) -> bool {
    let Some(st) = state(h) else { return false };
    if let Some((grab_y, from)) = st.bar_drag {
        if let (Some((_, content, view)), Some((_, _, _, th))) = (metrics(st.edit), thumb(h, st)) {
            let (_, len) = track(h, st);
            let max = content - view;
            let per_px = max / (len - th).max(1.0);
            // Held at either end when the pointer runs past it: asking for more each move
            // made the over-scroll guard pull it back every time, so the text shook.
            let end = if from as f32 > max && from as f32 - max <= 2.0 { from as f32 } else { max };
            let to = (from as f32 + (y - grab_y) as f32 * per_px).clamp(0.0, end).round() as i32;
            if to != rich::scroll_y(st.edit) {
                rich::set_scroll_y(st.edit, to);
            }
        }
        return true;
    }
    let hot = on_bar(h, x, y);
    if hot != st.bar_hot {
        st.bar_hot = hot;
        settle(st);
    }
    hot
}

pub(super) fn mouse_leave(h: HWND) {
    if let Some(st) = state(h)
        && st.bar_hot
    {
        st.bar_hot = false;
        settle(st);
    }
}

/// Press on the bar: grab the thumb, or jump so the thumb centres on the pointer first.
pub(super) fn mouse_down(h: HWND, x: i32, y: i32) -> bool {
    if !on_bar(h, x, y) {
        return false;
    }
    let Some(st) = state(h) else { return false };
    if let (Some((_, content, view)), Some((_, ty, _, th))) = (metrics(st.edit), thumb(h, st))
        && ((y as f32) < ty || (y as f32) >= ty + th)
    {
        let (top, len) = track(h, st);
        let k = ((y as f32 - top - th / 2.0) / (len - th).max(1.0)).clamp(0.0, 1.0);
        rich::set_scroll_y(st.edit, (k * (content - view)).round() as i32);
    }
    stop_glide(st);
    st.bar_drag = Some((y, rich::scroll_y(st.edit)));
    unsafe { SetCapture(h) };
    settle(st);
    true
}

pub(super) fn mouse_up(h: HWND) -> bool {
    let Some(st) = state(h) else { return false };
    if st.bar_drag.take().is_none() {
        return false;
    }
    unsafe { ReleaseCapture() };
    show(h, st);
    settle(st);
    true
}

/// Capture went elsewhere mid-drag.
pub(super) fn cancel(h: HWND) {
    if let Some(st) = state(h)
        && st.bar_drag.take().is_some()
    {
        settle(st);
    }
}

pub(super) fn paint(h: HWND, st: &NoteWin, c: &mut Canvas, ink: u32) {
    let a = crate::win::render::anim::clamp01(st.bar.value);
    if a <= 0.01 {
        return;
    }
    if let Some((x, y, w, th)) = thumb(h, st) {
        let strength = crate::win::render::anim::lerp(0.30, 0.48, st.bar_wide.value);
        c.shape(&Shape::new(x, y, w, th, w / 2.0), ink, strength * a, 1.0, 0.0);
    }
}
