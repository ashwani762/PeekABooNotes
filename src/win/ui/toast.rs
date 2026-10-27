// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo HUD: a dark capsule that rises from the bottom of the screen with a countdown
//! ring. It shows the newest of the actions still in time (`app`'s undo stack, see
//! `common::undo`); a click takes that one back, and then shows the one before it, while
//! that one is in time. The countdown waits while the pointer is on the toast, so reaching
//! for Undo never races the clock. A notice (notes can't be saved) shows over it for a
//! while, with nothing to undo.
//!
//! While only the clock runs, the ring sweeps on at the display's rate: each frame puts
//! back the few hundred pixels around the ring as they were without its arc, draws the arc
//! again and hands the compositor just that square (the arc's end moves a pixel every
//! 160 ms or so, and a 100 ms timer drew it in steps of one to four pixels). The whole
//! toast is drawn again only as it rises, sinks or lights up, or the seconds change.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::common::undo::SECONDS;
use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::util::{WM_MOUSELEAVE, rect, wide, x_of, y_of};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};

const CLASS: &str = "PeekABooToast";
/// How far along the ring (in pixels) its arc's end moves before it's drawn again: any
/// way at all, so every frame while it moves (a few hundred pixels a frame).
const ARC_STEP: f32 = 0.0;

struct Toast {
    hwnd: HWND,
    /// What Undo takes back now ("2 notes deleted"), and its seconds left; None when
    /// there's nothing to undo.
    undo: Option<String>,
    left: f32,
    /// A plain message instead (notes can't be saved): nothing to undo, so no button and
    /// no ring; it goes after its time (and an action still in time shows again).
    notice: Option<String>,
    notice_left: f32,
    /// What it said last, and whether that was a notice: it goes on showing it as it
    /// sinks away.
    said: (String, bool),
    /// When the clock last ran; None while the pointer holds it.
    clock_at: Option<Instant>,
    rise: Spring,
    /// How much of it shows: as it rises and sinks, and with Reduce Motion, where it fades
    /// in and out without rising.
    shown: Spring,
    /// It has just set off rising or sinking: its first step is one refresh, however
    /// long the work that raised it (a delete, an undo) kept the frames waiting. Stepped by
    /// that wait, its first frame came already half shown, or a fifth of the way down.
    fresh: bool,
    /// The Undo button lights up while the pointer is on the toast (all of it undoes).
    hot: Spring,
    /// Mouse button held down on the toast: the button sinks a little.
    pressed: bool,
    alive: bool,
    canvas: Option<Canvas>,
    /// What was last drawn while nothing moved: the seconds and the button pressed. None
    /// draws again.
    drawn: Option<(i32, bool)>,
    /// The ring as last drawn: its centre and radius, the square around it with the pixels
    /// there without its arc, and how long the arc was (pixels along the ring).
    ring: Option<Ring>,
    /// Where the window was last put.
    placed: (i32, i32),
    /// The work area it rises in (the monitor in use when it was raised) and that
    /// monitor's scale.
    work: RECT,
    s: f32,
    /// The capsule's width and the Undo pill's (`sizes`), measured when it's raised.
    size: (i32, f32),
    /// The capsule as last drawn, in the window: only it takes the pointer, not the shadow
    /// around it.
    capsule: RECT,
    body: Font,
    bold: Font,
    tiny: Font,
}

thread_local! {
    static TOAST: RefCell<Option<Toast>> = const { RefCell::new(None) };
}

/// The toast's window (null when there's none), whose display its frames keep to.
pub fn window() -> HWND {
    TOAST.with(|t| t.try_borrow().ok().and_then(|t| t.as_ref().map(|t| t.hwnd))).unwrap_or(null_mut())
}

/// What Undo would take back now changed (an action was done, undone or made final):
/// the toast shows `msg`, rising if it wasn't up, or goes when there's nothing left and
/// no notice.
pub fn undo(msg: Option<String>) {
    if msg.is_none() && TOAST.with(|t| t.borrow().is_none()) {
        return;
    }
    with_toast(|t| {
        t.undo = msg;
        t.left = SECONDS;
    });
}

/// Tell the user something that can't be undone or acted on here, for a few seconds.
pub fn notice(text: String) {
    with_toast(|t| {
        t.notice = Some(text);
        t.notice_left = SECONDS;
    });
}

/// The toast, made if need be on the monitor in use, after `f` changed what it says:
/// measured again, named after it (what screen readers read), up or on its way out.
fn with_toast(f: impl FnOnce(&mut Toast)) {
    let (work, s) = crate::win::ui::screen::active();
    TOAST.with(|t| {
        let mut t = t.borrow_mut();
        if t.is_none() {
            app::register_class(CLASS, Some(wndproc), IDC_HAND);
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    wide(CLASS).as_ptr(),
                    wide("").as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    app::hinst(),
                    null_mut(),
                )
            };
            crate::win::util::no_transitions(hwnd);
            let (body, bold, tiny) = fonts(s);
            *t = Some(Toast {
                hwnd,
                undo: None,
                left: SECONDS,
                notice: None,
                notice_left: 0.0,
                said: (String::new(), false),
                clock_at: None,
                rise: Spring::unit(0.0, theme::motion::MOMENTUM),
                shown: Spring::fade(0.0, (1.0, 0.24)),
                fresh: false,
                hot: Spring::unit(0.0, (1.0, 0.12)),
                pressed: false,
                alive: true,
                canvas: None,
                drawn: None,
                ring: None,
                placed: (0, 0),
                work,
                s,
                size: (0, 0.0),
                capsule: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                body,
                bold,
                tiny,
            });
        }
        let t = t.as_mut().unwrap();
        let was = (t.undo.clone(), t.notice.clone());
        f(t);
        let showing = t.undo.is_some() || t.notice.is_some();
        if showing && (!t.alive || was != (t.undo.clone(), t.notice.clone())) {
            // Raised again while showing, perhaps from another monitor: it moves there.
            t.work = work;
            if t.s != s {
                (t.body, t.bold, t.tiny) = fonts(s);
                t.s = s;
            }
        }
        settle(t);
    });
    anim::wake(Client::Toast);
}

/// After what the toast says changed: measured and named again, and up while it says
/// anything, else on its way out.
fn settle(t: &mut Toast) {
    t.drawn = None;
    match message(t) {
        Some(msg) => {
            t.fresh |= !t.alive || t.shown.target < 1.0;
            t.alive = true;
            t.size = measure(t);
            // It rises at once, from the first frame, rather than easing off.
            t.rise.launch_as(1.0, theme::motion::MOMENTUM);
            t.shown.launch_as(1.0, (1.0, 0.24));
            unsafe {
                SetWindowTextW(t.hwnd, wide(&msg).as_ptr());
                // Over notes in use, which are kept on top of other windows too.
                SetWindowPos(t.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            t.said = (msg, t.notice.is_some());
        }
        None => {
            // It sinks away at once, quicker than it rose.
            t.fresh |= t.alive;
            t.alive = false;
            t.rise.launch_as(0.0, (1.0, 0.22));
            t.shown.launch_as(0.0, (1.0, 0.18));
        }
    }
}

/// The toast's (body, bold, ring digit) fonts at scale `s`.
fn fonts(s: f32) -> (Font, Font, Font) {
    (
        Font::new(13.5 * s, FW_NORMAL, theme::FACE_TEXT),
        Font::new(13.5 * s, FW_SEMIBOLD, theme::FACE_TEXT),
        Font::new(9.5 * s, FW_SEMIBOLD, theme::FACE_SMALL),
    )
}

/// Gone at once, without the slide out (quitting).
pub fn close() {
    if let Some(t) = TOAST.with(|t| t.borrow_mut().take()) {
        unsafe { DestroyWindow(t.hwnd) };
    }
}

pub fn frame(dt: f32) -> bool {
    step(dt)
}

/// Run the clock (the undo stack's and the notice's) and the springs `dt` seconds on, and
/// draw what changed. Whether frames are wanted: while something moves, or the clock runs
/// (the ring sweeping on).
fn step(dt: f32) -> bool {
    // The clock first, outside the toast's borrow: actions whose time ran out are made
    // final, and what the toast says may change with it.
    let Some((over, clock)) = TOAST.with(|c| {
        c.borrow_mut().as_mut().map(|t| {
            let over = hovered(t);
            let now = Instant::now();
            let clock = if over { 0.0 } else { t.clock_at.map_or(0.0, |at| now.duration_since(at).as_secs_f32()) };
            t.clock_at = (!over && t.alive).then_some(now);
            (over, clock)
        })
    }) else {
        return false;
    };
    let left = app::undo_clock(clock);
    TOAST.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(t) = slot.as_mut() else { return false };
        if let Some(left) = left {
            t.left = left;
        }
        if t.notice.is_some() {
            t.notice_left -= clock;
            if t.notice_left <= 0.0 {
                t.notice = None;
                settle(t);
            }
        }
        let dt = if std::mem::take(&mut t.fresh) { dt.min(anim::period()) } else { dt };
        let moving = t.rise.step(dt) | t.shown.step(dt);
        t.pressed &= over;
        t.hot.to(if over && t.alive && t.notice.is_none() { 1.0 } else { 0.0 });
        let lighting = t.hot.step(dt);
        if !t.alive && (t.shown.value <= 0.01 || !moving) {
            unsafe { DestroyWindow(t.hwnd) };
            *slot = None;
            // What it offered to undo (an archived note kept ready to come back) is gone
            // too: memory can go back to Windows once nothing else is open.
            crate::win::app::schedule_trim();
            return false;
        }
        let still = !moving && !lighting && t.alive;
        let now = drawn_key(t);
        if !still || t.drawn != Some(now) {
            render(t);
            t.drawn = still.then_some(now);
        } else {
            sweep(t);
        }
        // Held under the pointer, with nothing moving, the clock stops: the pointer moving
        // off it (WM_MOUSEMOVE, WM_MOUSELEAVE) starts it again.
        !still || !over
    })
}

/// What a drawing of the still toast shows that changes with the clock, but for the ring:
/// the seconds, and the button pressed.
fn drawn_key(t: &Toast) -> (i32, bool) {
    (t.left.clamp(0.0, SECONDS).ceil() as i32, t.pressed)
}

/// The countdown ring as drawn: its centre and radius, the square around it, the pixels
/// there without the arc, and the arc's length in pixels along the ring.
struct Ring {
    cx: f32,
    cy: f32,
    r: f32,
    square: RECT,
    under: Vec<u32>,
    arc: f32,
}

/// The arc's length now, in pixels along the ring.
fn arc_px(t: &Toast, r: f32) -> f32 {
    std::f32::consts::TAU * r * t.left.clamp(0.0, SECONDS) / SECONDS
}

/// The ring's arc, `r` across, at `frac` of a turn, as the toast draws it.
fn draw_arc(c: &mut Canvas, cx: f32, cy: f32, r: f32, s: f32, frac: f32, a: f32) {
    c.ring(cx, cy, r, 2.0 * s, frac, theme::COLORS[0].accent, a);
}

/// The ring swept on, each frame its arc's end has moved: the square around it put back as
/// it was without the arc, the arc drawn again, and only that square given to the
/// compositor.
fn sweep(t: &mut Toast) {
    let Some(ring) = t.ring.as_ref() else { return };
    let arc = arc_px(t, ring.r);
    if (arc - ring.arc).abs() <= ARC_STEP {
        return;
    }
    let (s, (x, y)) = (t.s, t.placed);
    let Some(c) = t.canvas.as_mut() else { return };
    let ring = t.ring.as_mut().unwrap();
    c.copy_in(ring.square, &ring.under);
    draw_arc(c, ring.cx, ring.cy, ring.r, s, arc / (std::f32::consts::TAU * ring.r), 1.0);
    ring.arc = arc;
    c.present_dirty(t.hwnd, x, y, ring.square);
}

/// Whether the pointer is on the capsule (not on the shadow around it).
fn hovered(t: &Toast) -> bool {
    let mut p = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(t.hwnd, &mut r);
    }
    crate::win::util::pt_in(&t.capsule, p.x - r.left, p.y - r.top)
}

/// Whether window point (`x`, `y`) is on the capsule.
fn on_capsule(x: i32, y: i32) -> bool {
    TOAST.with(|t| t.borrow().as_ref().is_some_and(|t| crate::win::util::pt_in(&t.capsule, x, y)))
}

fn render(t: &mut Toast) {
    let s = t.s;
    let (w, bw) = t.size;
    let (h, m) = ((52.0 * s) as i32, (24.0 * s) as i32);
    let msg = message(t).unwrap_or_else(|| t.said.0.clone());
    let notice = t.notice.is_some() || (t.undo.is_none() && t.said.1);
    let (cw, ch) = (w + 2 * m, h + 2 * m);
    if t.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
        t.canvas = Some(Canvas::new(cw, ch));
    }
    let c = t.canvas.as_mut().unwrap();
    c.clear(0);
    t.ring = None;
    let a = clamp01(t.shown.value);
    let lift = ((1.0 - t.rise.value) * 28.0 * s).round();
    let (x, y) = (m as f32, m as f32 * 0.6 + lift);
    let card = Shape::new(x, y, w as f32, h as f32, h as f32 / 2.0);
    t.capsule = rect(x as i32, y as i32, w, h);
    c.soft_shadow(&card, s, 1.4 * a);
    c.shape(&card, 0x1C1C1E, 0.94 * a, 1.0, 0.0);
    c.shape(&card, 0xFFFFFF, 0.10 * a, 1.0, 1.0);
    c.begin_batch();
    if notice {
        c.text(&msg, &t.body, rect((x + 24.0 * s) as i32, y as i32, w - (48.0 * s) as i32, h), 0xF2F2F2, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        c.flush();
        present(t, cw, ch);
        return;
    }
    // Countdown ring: its track and the seconds, then (below, once the text is down) its arc.
    let left = t.left.clamp(0.0, SECONDS);
    let (rcx, rcy, rr) = (x + 26.0 * s, y + h as f32 / 2.0, 10.0 * s);
    c.ring(rcx, rcy, rr, 2.0 * s, 1.0, 0xFFFFFF, 0.16 * a);
    let secs = rect((rcx - rr) as i32, (rcy - rr) as i32, (2.0 * rr) as i32, (2.0 * rr) as i32);
    c.text(&format!("{}", left.ceil() as i32), &t.tiny, secs, 0xFFFFFF, 0.85 * a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let bh = 32.0 * s;
    let button = Shape::new(x + w as f32 - bw - 10.0 * s, y + (h as f32 - bh) / 2.0, bw, bh, bh / 2.0);
    let tx = (x + 48.0 * s) as i32;
    let tw = (button.x - 12.0 * s) as i32 - tx;
    c.text(&msg, &t.body, rect(tx, y as i32, tw, h), 0xF2F2F2, a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    // Brighter under the pointer; pressed, it sinks a little and brightens a touch more.
    let fill = if t.pressed { button.inset(1.5 * s) } else { button };
    let lit = 0.12 * clamp01(t.hot.value) + if t.pressed { 0.08 } else { 0.0 };
    c.shape(&fill, 0xFFFFFF, (0.14 + lit) * a, 1.0, 0.0);
    let br = rect(button.x as i32, button.y as i32, bw as i32, bh as i32);
    c.text(t!("toast_undo"), &t.bold, br, theme::COLORS[0].accent, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    c.flush();
    let reach = (rr + 2.0 * s + 2.0).ceil() as i32;
    let square = rect(rcx as i32 - reach, rcy as i32 - reach, 2 * reach + 1, 2 * reach + 1);
    let under = c.copy_out(square);
    let arc = std::f32::consts::TAU * rr * left / SECONDS;
    draw_arc(c, rcx, rcy, rr, s, left / SECONDS, a);
    t.ring = (a >= 0.999).then_some(Ring { cx: rcx, cy: rcy, r: rr, square, under, arc });
    present(t, cw, ch);
}

/// What the toast says: the notice while there is one, else what Undo takes back.
fn message(t: &Toast) -> Option<String> {
    t.notice.clone().or_else(|| t.undo.clone())
}

/// The capsule's width and its Undo pill's, in pixels at scale `s`, for a message and a
/// button label measured `msg_w` and `label_w` wide: the pill fits its label (German and
/// Hindi run longer than "Undo") and the capsule grows, within reason, to keep the
/// message beside it.
fn sizes(s: f32, msg_w: i32, label_w: i32) -> (i32, f32) {
    let bw = (label_w as f32 + 2.0 * 16.0 * s).max(76.0 * s).ceil();
    let need = (48.0 + 12.0 + 10.0) * s + msg_w as f32 + bw;
    (need.clamp(320.0 * s, 560.0 * s).ceil() as i32, bw)
}

/// `sizes` for what the toast says now. A notice has no ring or button: the capsule hugs
/// its text, centred, rather than leaving the Undo toast's room empty beside it.
fn measure(t: &Toast) -> (i32, f32) {
    let s = t.s;
    match (&t.notice, &t.undo) {
        (Some(msg), _) => (((t.body.measure(msg).0 as f32 + 48.0 * s).clamp(140.0 * s, 560.0 * s)).ceil() as i32, 0.0),
        (None, Some(msg)) => sizes(s, t.body.measure(msg).0, t.bold.measure(t!("toast_undo")).0),
        (None, None) => t.size,
    }
}

/// Put the drawn toast at the bottom of the work area.
fn present(t: &mut Toast, cw: i32, ch: i32) {
    let (px, py) = crate::win::ui::screen::bottom_centre(t.work, cw, ch, (12.0 * t.s) as i32);
    t.placed = (px, py);
    if let Some(c) = t.canvas.as_mut() {
        c.present(t.hwnd, px, py);
    }
    unsafe {
        if IsWindowVisible(t.hwnd) == 0 {
            ShowWindow(t.hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            // The countdown waits while the pointer is on the capsule: going on, off, or
            // away, it looks again.
            WM_MOUSEMOVE => {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut tme);
                anim::wake(Client::Toast);
                return 0;
            }
            WM_MOUSELEAVE => {
                anim::wake(Client::Toast);
                return 0;
            }
            // A hand on the capsule, the usual arrow on the shadow around it.
            WM_SETCURSOR => {
                let mut p = POINT { x: 0, y: 0 };
                let mut r: RECT = std::mem::zeroed();
                GetCursorPos(&mut p);
                GetWindowRect(h, &mut r);
                SetCursor(LoadCursorW(null_mut(), if on_capsule(p.x - r.left, p.y - r.top) { IDC_HAND } else { IDC_ARROW }));
                return 1;
            }
            // Shown on the press, done on the release, on the capsule only.
            WM_LBUTTONDOWN => {
                if !on_capsule(x_of(lp), y_of(lp)) {
                    return 0;
                }
                TOAST.with(|t| {
                    if let Some(t) = t.borrow_mut().as_mut() {
                        t.pressed = true;
                    }
                });
                anim::wake(Client::Toast);
                return 0;
            }
            WM_LBUTTONUP => {
                if !on_capsule(x_of(lp), y_of(lp)) {
                    TOAST.with(|t| {
                        if let Some(t) = t.borrow_mut().as_mut() {
                            t.pressed = false;
                        }
                    });
                    return 0;
                }
                let notice = TOAST.with(|t| {
                    t.borrow_mut().as_mut().map(|t| {
                        t.pressed = false;
                        t.notice.is_some()
                    })
                });
                // A notice has nothing to undo: a click just dismisses it (and shows what
                // can still be undone, if anything).
                if notice == Some(true) {
                    with_toast(|t| t.notice = None);
                } else {
                    app::undo();
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::{draw_arc, sizes};
    use crate::win::render::gfx::{Canvas, Shape};
    use crate::win::util::rect;

    /// The toast's capsule with the ring's track, as `render` leaves it before the arc.
    fn capsule() -> Canvas {
        let mut c = Canvas::new(80, 60);
        c.clear(0);
        c.shape(&Shape::new(4.0, 6.0, 72.0, 48.0, 24.0), 0x1C1C1E, 0.94, 1.0, 0.0);
        c.ring(30.0, 30.0, 10.0, 2.0, 1.0, 0xFFFFFF, 0.16);
        c
    }

    #[test]
    fn the_ring_swept_on_in_place_is_the_ring_drawn_afresh() {
        let square = rect(30 - 15, 30 - 15, 31, 31);
        let mut c = capsule();
        let under = c.copy_out(square);
        draw_arc(&mut c, 30.0, 30.0, 10.0, 1.0, 0.73, 1.0);
        // Many sweeps on, each putting the square back and drawing the arc a little shorter.
        let mut frac = 0.73;
        for _ in 0..40 {
            frac -= 0.0031;
            c.copy_in(square, &under);
            draw_arc(&mut c, 30.0, 30.0, 10.0, 1.0, frac, 1.0);
        }
        let mut fresh = capsule();
        draw_arc(&mut fresh, 30.0, 30.0, 10.0, 1.0, frac, 1.0);
        assert!(c.copy_out(rect(0, 0, 80, 60)) == fresh.copy_out(rect(0, 0, 80, 60)));
    }

    #[test]
    fn the_rings_arc_moves_a_fraction_of_a_pixel_a_frame_and_is_drawn_each_time() {
        // Its end moves 2 pi r a turn over the toast's time: at 100% and 200%, a frame at
        // 60 Hz moves it a fraction of a pixel, and any move is drawn.
        for s in [1.0f32, 2.0] {
            let per_frame = std::f32::consts::TAU * 10.0 * s / crate::common::undo::SECONDS / 60.0;
            assert!(per_frame < 0.25 && per_frame > super::ARC_STEP, "{per_frame}");
        }
    }

    #[test]
    fn the_undo_pill_fits_its_label() {
        // A short label keeps the usual pill and capsule.
        assert_eq!(sizes(1.0, 120, 30), (320, 76.0));
        // "पूर्ववत करें" or "Widerrufen" in bold: the pill grows past its label, the capsule
        // keeps the message beside it.
        let (w, bw) = sizes(1.0, 150, 90);
        assert_eq!(bw, 122.0);
        assert!(w as f32 >= 48.0 + 150.0 + 12.0 + bw + 10.0);
        // At 150% everything scales, and a very long message stops growing the capsule.
        assert_eq!(sizes(1.5, 180, 45), (480, 114.0));
        assert_eq!(sizes(1.0, 2000, 30).0, 560);
    }
}
