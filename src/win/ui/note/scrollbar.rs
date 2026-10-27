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

/// The caret's line in `edit`: its top, its height, and the editor's client area.
fn caret_line(edit: HWND) -> (i32, i32, RECT) {
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
    (top, line_h, rc)
}

/// Whether the caret's line is wholly in view.
fn caret_line_in_view(edit: HWND) -> bool {
    let (top, line_h, rc) = caret_line(edit);
    top >= 0 && top + line_h <= rc.bottom
}

/// Before a key that may type: the selection, whether the caret's line is wholly in view,
/// the scroll position, and the caret's line's top in the text (px from its top).
pub(super) type Typing = ((i32, i32), bool, i32, i32);

pub(super) fn before_typing(edit: HWND) -> Typing {
    let scroll = rich::scroll_y(edit);
    (rich::sel(edit), caret_line_in_view(edit), scroll, caret_line(edit).0 + scroll)
}

/// After a key that may have typed (`before` it): typed on to a new line at the bottom
/// edge (an item that wraps), RichEdit left the caret's line below the view, and the text
/// 3 px lower, until the next key scrolled it into view (the note jumped down, then 27 px
/// up), or scrolled it in itself, as far as the note's end (35 px for a 20 px line, the
/// bottom's faded edge left over nothing). The caret's line comes into view at once, as
/// Enter's does: the text goes up as far as that line went down, one line, so the new line
/// is where the one it came out of was.
pub(super) fn follow_typing(edit: HWND, before: Typing) {
    let (sel, in_view, scroll, top) = before;
    if sel.0 != sel.1 || !in_view || rich::sel(edit) == sel {
        return;
    }
    let down = caret_line(edit).0 + rich::scroll_y(edit) - top;
    if down > 0 {
        if rich::scroll_y(edit) != scroll {
            rich::set_scroll_y(edit, scroll);
        }
        if !caret_line_in_view(edit) {
            rich::set_scroll_y(edit, (scroll + down).min(max_scroll(edit)));
        }
    }
    if !caret_line_in_view(edit) {
        keep_caret_in_view(edit);
    }
}

/// Nudge the scroll so the caret's line is wholly in view. RichEdit insists on that the
/// moment it next paints or takes focus: a view restored with that line half under the top
/// (or bottom) would jump by the hidden part right after the note appears.
pub(super) fn keep_caret_in_view(edit: HWND) {
    let (top, line_h, rc) = caret_line(edit);
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

/// The wheel's glide: each notch sets the text off at once, at its fastest, slowing
/// smoothly to land on a whole pixel at a set time, without a slow creep at the end. A
/// notch while it's still moving carries on from the speed the text has, and picks up
/// from there, sagging only a little between quick notches (one the other way starts
/// afresh).
///
/// It was a spring from rest: each notch started slow (its first step a third of its
/// second), so a spin surged and dipped at every notch (12, 32, 64, 32, 65 px a frame),
/// and the spring's long tail crept the last pixels in, the final one half a second late.
#[derive(Clone, Copy, Debug)]
pub(super) struct Glide {
    pub value: f32,
    /// Where it ends up (a whole pixel).
    pub target: f32,
    /// The curve under way (`curve`): where it started, its starting speed (as a share of
    /// the distance over the time), how long it takes and how far along it is (s).
    from: f32,
    a: f32,
    dur: f32,
    t: f32,
}

/// How long one notch's glide takes, from rest (s).
const GLIDE: f32 = 0.22;
/// The shortest and longest a notch caught up in a glide can take (s).
const GLIDE_MIN: f32 = 0.12;
const GLIDE_MAX: f32 = 0.36;
/// The starting speed from rest, and the fastest start a curve can take without
/// overshooting its end.
const FRESH: f32 = 2.0;

/// How far along (0 to 1) a curve that starts at speed `a` is at `u` of its time, and its
/// speed there: `a u + (4 - 3a) u³ + (2a - 3) u⁴`, which starts at speed `a` without
/// slowing at once (no u² term), and comes to rest at 1. At `a` = 2 it's
/// `2(1 - u)²(1 + 2u)` fast: slowing all the way, gently at first; below that it picks up
/// speed before slowing.
fn curve(a: f32, u: f32) -> (f32, f32) {
    let (b, c) = (4.0 - 3.0 * a, 2.0 * a - 3.0);
    (a * u + b * u * u * u + c * u * u * u * u, a + 3.0 * b * u * u + 4.0 * c * u * u * u)
}

impl Glide {
    pub(super) fn new() -> Glide {
        Glide { value: 0.0, target: 0.0, from: 0.0, a: 0.0, dur: 0.0, t: 0.0 }
    }

    pub(super) fn settled(&self) -> bool {
        self.t >= self.dur
    }

    /// At `v`, still.
    pub(super) fn snap(&mut self, v: f32) {
        *self = Glide { value: v, target: v, from: v, a: 0.0, dur: 0.0, t: 0.0 };
    }

    /// The position and speed (px/s) `t` seconds into the curve under way.
    fn at(&self, t: f32) -> (f32, f32) {
        if self.dur <= 0.0 {
            return (self.target, 0.0);
        }
        let d = self.target - self.from;
        let (p, v) = curve(self.a, (t / self.dur).clamp(0.0, 1.0));
        (self.from + d * p, d * v / self.dur)
    }

    /// Head for `target` (rounded to a whole pixel) from where it is, at the speed it has.
    pub(super) fn to(&mut self, target: f32) {
        let (p, v) = if self.settled() { (self.value, 0.0) } else { self.at(self.t) };
        let target = target.round();
        let d = target - p;
        if d.abs() < 0.5 {
            self.snap(target);
            return;
        }
        // Moving its way already: on at the speed it has, over as long as a curve from rest
        // at that speed would take, within reason. From rest, or turning back, off at once
        // at the fresh speed.
        let (dur, a) = if v * d > 0.0 {
            let dur = (FRESH * d / v).clamp(GLIDE_MIN, GLIDE_MAX);
            (dur, (v * dur / d).min(FRESH))
        } else {
            (GLIDE, FRESH)
        };
        *self = Glide { value: p, target, from: p, a, dur, t: 0.0 };
    }

    /// Advance by `dt` seconds. True while still moving (its value short of the target).
    pub(super) fn step(&mut self, dt: f32) -> bool {
        if self.settled() || crate::win::render::anim::reduced() {
            self.snap(self.target);
            return false;
        }
        // Its first step is one refresh at most, as long as the notch can have been on
        // screen: a frame made late by the notch's own work, while something else was
        // moving, stepped two, and the first notch after a note opened went 19 px, then 10.
        self.t += if self.t == 0.0 { dt.min(crate::win::render::anim::period()) } else { dt };
        if self.t >= self.dur {
            self.snap(self.target);
            return false;
        }
        self.value = self.at(self.t).0;
        true
    }
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
    Some((rc.right as f32 - EDGE * s - 1.0 * s - w, y, w, th))
}

/// The window's outermost pixels beside the bar (DIP): there the note is resized, as along
/// the rest of its edge, rather than scrolled.
const EDGE: f32 = 5.0;

/// The pointer is over the bar's column (beside the text, along the track), short of the
/// window's resize edge.
pub(super) fn on_bar(h: HWND, x: i32, y: i32) -> bool {
    let Some(st) = state(h) else { return false };
    if metrics(st.edit).is_none() {
        return false;
    }
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let (top, len) = track(h, st);
    x >= rc.right - (15.0 * st.s) as i32 && x < rc.right - (EDGE * st.s) as i32 && (y as f32) >= top && (y as f32) < top + len
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
                // Painted now, text and thumb together: the pointer's moves come in ahead of
                // paints, so on a quick drag the thumb was left behind and then jumped.
                unsafe {
                    windows_sys::Win32::Graphics::Gdi::RedrawWindow(
                        h,
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        windows_sys::Win32::Graphics::Gdi::RDW_UPDATENOW | windows_sys::Win32::Graphics::Gdi::RDW_ALLCHILDREN,
                    )
                };
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

    /// The glide's position at 60 frames a second, with a notch of `step` px down at each
    /// of `notches` (s), until it lands.
    fn glide(notches: &[f32], step: f32) -> Vec<f32> {
        let mut g = Glide::new();
        let (mut t, dt) = (0.0f32, 1.0 / 60.0);
        let mut left = notches.to_vec();
        let mut out = vec![0.0];
        loop {
            while left.first().is_some_and(|&n| n <= t + 1e-4) {
                left.remove(0);
                let from = if g.settled() { g.value } else { g.target };
                g.to(from + step);
            }
            let moving = g.step(dt);
            t += dt;
            out.push(if moving { g.value } else { g.target });
            if !moving && left.is_empty() {
                return out;
            }
            assert!(out.len() < 200, "lands");
        }
    }

    fn steps(p: &[f32]) -> Vec<f32> {
        p.windows(2).map(|w| w[1] - w[0]).collect()
    }

    /// A notch's first step is one refresh, however late its frame (the notch's own work
    /// made one late while something else moved: 19 px, then 10).
    #[test]
    fn a_notchs_first_step_is_one_refresh() {
        let p = crate::win::render::anim::period();
        let first = |dt: f32| {
            let mut g = Glide::new();
            g.to(100.0);
            g.step(dt);
            g.value
        };
        assert_eq!(first(2.0 * p), first(p));
        // From there, as far as the frames go.
        let mut g = Glide::new();
        g.to(100.0);
        g.step(p);
        let a = g.value;
        g.step(2.0 * p);
        assert!(g.value - a > 1.5 * a, "{a} then {}", g.value - a);
    }

    /// One notch: the text moves at once, its first step the biggest, each after it no
    /// bigger than the one before, and it lands on its whole pixel in the glide's time,
    /// with no late step.
    #[test]
    fn a_notch_eases_out_and_lands() {
        let p = glide(&[0.0], 60.0);
        let s = steps(&p);
        assert!(s[0] > 8.0, "moves at once: {s:?}");
        assert!(s.windows(2).all(|w| w[1] <= w[0] + 1e-3), "never speeds up: {s:?}");
        assert_eq!(*p.last().unwrap(), 60.0);
        assert!(p.len() as f32 - 1.0 <= (GLIDE * 60.0).ceil() + 1.0, "{} frames", p.len());
        // Easing in to land: the last steps are the smallest.
        assert!(s[s.len() - 1] < 1.0, "{s:?}");
        // Up to the last frame it's still moving: no pause before the end.
        assert!(s.iter().all(|&d| d > 0.0), "{s:?}");
    }

    /// Notches 30 and 50 ms apart (quick spins): the speed changes smoothly from frame to
    /// frame (it went 64, 32, 65 px a frame), sags little between notches, and the text
    /// lands where they add up to.
    #[test]
    fn a_spin_glides_without_surges() {
        for (notches, end) in [(vec![0.0, 0.05, 0.10], 180.0), (vec![0.0, 0.03, 0.06, 0.09, 0.12], 300.0)] {
            let p = glide(&notches, 60.0);
            let s = steps(&p);
            assert_eq!(*p.last().unwrap(), end);
            // No step more than a quarter of the fastest one away from the one before, and
            // from the fastest after the last notch on, only slower.
            let last_notch = (notches.last().unwrap() * 60.0).round() as usize;
            let peak = s.iter().cloned().fold(0.0, f32::max);
            assert!(s.windows(2).all(|w| (w[1] - w[0]).abs() <= peak * 0.25), "{s:?}");
            let late = s[last_notch..].iter().cloned().fold(0.0, f32::max);
            let top = last_notch + s[last_notch..].iter().position(|&d| d == late).unwrap();
            assert!(s[top..].windows(2).all(|w| w[1] <= w[0]), "{s:?}");
            // Before it eases off for good, never under 80% of the first step.
            assert!(s[..=last_notch].iter().all(|&d| d > s[0] * 0.8), "{s:?}");
            assert!(s.iter().all(|&d| d > 0.0), "{s:?}");
        }
    }

    /// A notch while gliding carries on at the speed the text has.
    #[test]
    fn a_notch_keeps_the_speed_it_finds() {
        let mut g = Glide::new();
        g.to(60.0);
        for _ in 0..3 {
            g.step(1.0 / 60.0);
        }
        let v = g.at(g.t).1;
        g.to(120.0);
        let v2 = g.at(0.0).1;
        assert!((v - v2).abs() < 1.0, "{v} then {v2}");
        // Turning back starts afresh, the other way.
        g.to(0.0);
        assert!(g.at(0.0).1 < 0.0);
        // Snapped: settled where it is.
        g.snap(33.0);
        assert!(g.settled() && !g.step(0.1) && g.value == 33.0);
    }

    /// Typing on to a new line at the bottom edge of a scrolled note (a checklist item
    /// that wraps) keeps the caret's line in view at once.
    #[test]
    fn typing_on_to_a_new_line_keeps_it_in_view() {
        let (w, h) = (260, 240);
        let e = unsafe {
            LoadLibraryW(wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | (ES_MULTILINE | ES_AUTOVSCROLL) as u32, 0, 0, w, h, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        let mut s = String::from("Tasks\r");
        for i in 1..=14 {
            s.push_str(&format!("\u{2610} task number {i} to do\r"));
        }
        s.pop();
        rich::insert_text(e, &s);
        rich::apply_base_format(e, 0, 0xFFFFFF);
        rich::style_boxes(e);
        let t = rich::text(e);
        let at = rich::unmark_str(&t).find("task number 13 to do").unwrap() + "task number 13 to do".len();
        rich::set_sel(e, at as i32, at as i32);
        rich::set_scroll_y(e, max_scroll(e));
        assert!(caret_line_in_view(e));
        let mut wrapped = false;
        for c in " jumps over the lazy dog".encode_utf16() {
            let before = before_typing(e);
            let lines = rich::send(e, EM_GETLINECOUNT, 0, 0);
            rich::send(e, WM_CHAR, c as usize, 0);
            wrapped |= rich::send(e, EM_GETLINECOUNT, 0, 0) > lines;
            follow_typing(e, before);
            assert!(caret_line_in_view(e), "after {:?}", char::from_u32(c as u32));
        }
        assert!(wrapped, "the item wrapped");
        unsafe { DestroyWindow(e) };
    }

    /// An item that wraps at the bottom edge, with more of the note below it, moves the
    /// text up by the one line it grew, its new line where the line it came out of was:
    /// not as far as the note's end (RichEdit scrolled 35 px for a 20 px line there, the
    /// faded bottom edge left over nothing), and not under the edge either.
    #[test]
    fn a_wrap_at_the_bottom_moves_the_text_up_one_line() {
        let w = 280;
        let e = unsafe {
            LoadLibraryW(wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | (ES_MULTILINE | ES_AUTOVSCROLL) as u32, 0, 0, w, 600, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        let mut s = String::from("Tick test");
        for i in 1..=10 {
            s.push_str(&format!("\r\u{2610} task number {i} to do"));
        }
        rich::insert_text(e, &s);
        rich::apply_base_format(e, 0, 0xFFFFFF);
        rich::style_title(e);
        rich::style_boxes(e);
        let t = rich::text(e);
        let last = t.iter().rposition(|&c| c == rich::CR).unwrap() as i32;
        // The last item cut in half by the bottom edge, the one before it in view.
        let h = rich::pos_from_char(e, last + 1).y + 10;
        unsafe { SetWindowPos(e, null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        rich::set_sel(e, last, last);
        rich::set_scroll_y(e, 0);
        assert!(caret_line_in_view(e));
        let mut moves = Vec::new();
        for c in " jumps over the lazy dog and more".encode_utf16() {
            let before = before_typing(e);
            let (scroll, line) = (rich::scroll_y(e), caret_line(e).0);
            rich::send(e, WM_CHAR, c as usize, 0);
            follow_typing(e, before);
            assert!(caret_line_in_view(e), "after {:?}", char::from_u32(c as u32));
            let moved = rich::scroll_y(e) - scroll;
            if moved != 0 {
                // Where the line it came out of was.
                assert_eq!(caret_line(e).0, line, "after {:?}", char::from_u32(c as u32));
                moves.push(moved);
            }
        }
        // One move, of one line (as tall as a body line, not two), and more note below.
        assert_eq!(moves.len(), 1, "{moves:?}");
        assert!((15..=24).contains(&moves[0]), "{moves:?}");
        assert!(rich::scroll_y(e) < max_scroll(e), "scrolled to the end");
        unsafe { DestroyWindow(e) };
    }
}
