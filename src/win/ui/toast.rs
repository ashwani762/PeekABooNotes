// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo HUD: a dark capsule that rises from the bottom of the screen with a countdown
//! ring. It shows the actions still in time (`app`'s undo stack, see `common::undo`) as a
//! stack of cards: the newest on top, the ones before it peeking out above it, each a
//! little narrower and dimmer. A click takes the top one back and its × makes it final
//! now; either way it drops out in front and the one under it comes up into its place,
//! with the time it has left. The countdown waits while the pointer is on the toast, so
//! reaching for Undo never races the clock. A notice (notes can't be saved) shows over it
//! for a while, with nothing to undo.
//!
//! While only the clock runs, the ring sweeps on at the display's rate: each frame puts
//! back the few hundred pixels around the ring as they were without its arc, draws the arc
//! again and hands the compositor just that square (the arc's end moves a pixel every
//! 160 ms or so, and a 100 ms timer drew it in steps of one to four pixels). The whole
//! toast is drawn again only as it rises, sinks, lights up or its cards move, or the
//! seconds change.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::common::undo::SECONDS;
use crate::win::render::anim::{self, Client, Ease, Spring, clamp01, lerp};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::ui::tooltip;
use crate::win::util::{WM_MOUSELEAVE, rect, wide, x_of, y_of};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};

const CLASS: &str = "PeekABooToast";
/// How far along the ring (in pixels) its arc's end moves before it's drawn again: any
/// way at all, so every frame while it moves (a few hundred pixels a frame).
const ARC_STEP: f32 = 0.0;

/// The most cards the toast keeps up with: the top, the two peeking out behind it, and
/// one hidden behind those, ready to come up.
pub const CARDS: usize = 4;
/// How many cards peek out behind the top one.
const BEHIND: f32 = 2.0;
/// How far each card behind the top one peeks out above it, at 100%, and how much
/// narrower it is, a share of the top one's width.
const PEEK: f32 = 7.0;
const NARROW: f32 = 0.055;
/// How far below its place a card coming in or going out in front is, out of sight.
const DROP: f32 = 16.0;
/// How long a card takes to its new place, in seconds.
const MOVE: f32 = 0.26;
/// The ×'s disc across, at 100%.
const CLOSE: f32 = 28.0;

/// One action on the undo stack as the toast shows it: which card it is, what it says
/// ("2 notes deleted") and its seconds left.
pub struct Card {
    pub id: u64,
    pub text: String,
    pub left: f32,
}

/// A card as it's drawn in the stack.
struct Stacked {
    id: u64,
    text: String,
    left: f32,
    /// Its capsule's width and its Undo pill's (`sizes`).
    size: (i32, f32),
    /// Its place: 0 on top, 1 and 2 peeking out behind, 3 hidden behind those; below 0, in
    /// front of the top, coming in or going out (-1 out of sight).
    place: Ease,
    /// How much of it shows: one coming up from under the hidden one fades in, one whose
    /// time ran out behind the top fades out where it is.
    fade: Ease,
    /// No longer on the stack (undone, dismissed, made final): on its way out, and
    /// dropped once there. It keeps the width it had.
    gone: bool,
}

/// What on the top card the pointer is on: the × dismisses, the rest of it undoes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Undo,
    Close,
}

struct Toast {
    hwnd: HWND,
    /// What Undo takes back now (the top card's "2 notes deleted"), and its seconds left;
    /// None when there's nothing to undo.
    undo: Option<String>,
    left: f32,
    /// The stack's cards, those on their way out too.
    cards: Vec<Stacked>,
    /// The stack's width: its top card's own, moving to the next one's as that comes up.
    width: Ease,
    /// The cards were just put up where they are, not moved there: the width too.
    at_once: bool,
    /// A plain message instead (notes can't be saved): nothing to undo, so no button and
    /// no ring; it goes after its time (and an action still in time shows again).
    notice: Option<String>,
    notice_left: f32,
    /// The notice's capsule width: it hugs its text.
    notice_w: i32,
    /// What it said last, and whether that was a notice: it goes on showing it as it
    /// sinks away.
    said: (String, bool),
    /// When the clock last ran; None while the pointer holds it.
    clock_at: Option<Instant>,
    rise: Ease,
    /// How much of it shows: as it rises and sinks, and with Reduce Motion, where it fades
    /// in and out without rising.
    shown: Ease,
    /// Frames to hold still as it's put up, before it fades in: the first a window is
    /// shown at comes and goes before the screen has it, and the fade, already under way
    /// there, first showed at 18 to 32%.
    hold: u8,
    /// Its first step after it set off rising or sinking is one refresh at most, however
    /// long the work that raised it (a delete, an undo) kept the frames waiting.
    fresh: bool,
    /// What it said before a notice came or went, as it was drawn (its pixels and width),
    /// cross-fading into the new (`swap`, 0 to 1), rather than cut in a frame. The undo
    /// stack's own changes move its cards instead.
    old: Option<(Vec<u32>, i32)>,
    swap: Ease,
    /// The Undo button lights up while the pointer is on the toast (all of it undoes but
    /// the ×), and the × while the pointer is on it.
    hot: Spring,
    hot_close: Spring,
    /// What the mouse button went down on: it sinks a little.
    pressed: Option<Part>,
    alive: bool,
    canvas: Option<Canvas>,
    /// What was last drawn while nothing moved: the seconds and what's pressed. None
    /// draws again.
    drawn: Option<(i32, Option<Part>)>,
    /// The ring as last drawn: its centre and radius, the square around it with the pixels
    /// there without its arc, and how long the arc was (pixels along the ring).
    ring: Option<Ring>,
    /// Where the window was last put.
    placed: (i32, i32),
    /// The work area it rises in (the monitor in use when it was raised) and that
    /// monitor's scale.
    work: RECT,
    s: f32,
    /// The top card (or the notice) as last drawn, in the window, where it's going: only
    /// it takes the pointer, not the cards behind or the shadow around it. And its ×
    /// (empty for a notice).
    capsule: RECT,
    close: RECT,
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

/// What Undo would take back changed (an action was done, undone, dismissed or made
/// final): the toast stacks `cards`, newest first, rising if it wasn't up, or goes when
/// there's nothing left and no notice. A new action raises it on the display it was done
/// on, `at` (work area and scale); otherwise (None) it stays where it is.
pub fn undo(cards: Vec<Card>, at: Option<(RECT, f32)>) {
    if cards.is_empty() && TOAST.with(|t| t.borrow().is_none()) {
        return;
    }
    with_toast(at, |t| restack(t, cards));
}

/// Tell the user something that can't be undone or acted on here, for a few seconds.
pub fn notice(text: String) {
    with_toast(None, |t| {
        t.notice = Some(text);
        t.notice_left = SECONDS;
    });
}

/// The stack's cards after it changed, newest first: one no longer on it goes, out in
/// front when it was on top (undone, dismissed) or fading where it is behind (its time
/// ran out); the rest move to their new places, and a new one on top comes in in front.
/// Put up from nothing, they're at their places at once. With nothing left, the cards stay
/// as they are while the toast sinks, the ring where it stopped.
fn restack(t: &mut Toast, cards: Vec<Card>) {
    let Some(first) = cards.first() else {
        t.undo = None;
        return;
    };
    t.undo = Some(first.text.clone());
    t.left = first.left;
    let at_once = t.cards.is_empty() || t.shown.value() <= 0.0;
    if at_once {
        t.cards.clear();
        t.at_once = true;
    }
    let width = t.width.value().round() as i32;
    for c in t.cards.iter_mut().filter(|c| !c.gone) {
        if cards.iter().all(|k| k.id != c.id) {
            c.gone = true;
            if c.place.target() <= 0.0 {
                // The top, at the stack's width as it is now, dropping out in front.
                c.size.0 = width;
                c.place.go(-1.0, MOVE);
            } else {
                c.fade.go(0.0, MOVE);
            }
        }
    }
    for (k, card) in cards.into_iter().enumerate() {
        let place = (k as f32).min(BEHIND + 1.0);
        if let Some(c) = t.cards.iter_mut().find(|c| c.id == card.id && !c.gone) {
            c.text = card.text;
            c.left = card.left;
            c.place.go(place, MOVE);
            continue;
        }
        let (from, fade) = if at_once {
            (place, 1.0)
        } else if k == 0 {
            (-1.0, 1.0)
        } else {
            (place, 0.0)
        };
        let mut c = Stacked { id: card.id, text: card.text, left: card.left, size: (0, 0.0), place: Ease::at(from), fade: Ease::at(fade), gone: false };
        c.place.go(place, MOVE);
        c.fade.go(1.0, MOVE);
        t.cards.push(c);
    }
}

/// The toast, made if need be, after `f` changed what it says: measured again, named
/// after it (what screen readers read), up or on its way out. It rises on `at`, or (None)
/// where it is, or on the monitor in use when it wasn't up.
fn with_toast(at: Option<(RECT, f32)>, f: impl FnOnce(&mut Toast)) {
    let (work, s) = at.unwrap_or_else(crate::win::ui::screen::active);
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
                cards: Vec::new(),
                width: Ease::at(0.0),
                at_once: true,
                notice: None,
                notice_left: 0.0,
                notice_w: 0,
                said: (String::new(), false),
                clock_at: None,
                rise: Ease::at(0.0),
                shown: Ease::at(0.0),
                hold: 0,
                fresh: false,
                old: None,
                swap: Ease::at(1.0),
                hot: Spring::unit(0.0, (1.0, 0.12)),
                hot_close: Spring::unit(0.0, (1.0, 0.12)),
                pressed: None,
                alive: true,
                canvas: None,
                drawn: None,
                ring: None,
                placed: (0, 0),
                work,
                s,
                capsule: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                close: RECT { left: 0, top: 0, right: 0, bottom: 0 },
                body,
                bold,
                tiny,
            });
        }
        let t = t.as_mut().unwrap();
        f(t);
        let showing = t.undo.is_some() || t.notice.is_some();
        if showing && (!t.alive || at.is_some()) {
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
    measure(t);
    match message(t) {
        Some(msg) => {
            let up = t.alive && t.shown.target() >= 1.0;
            if up && msg != t.said.0 && (t.notice.is_some() || t.said.1) && t.shown.value() > 0.5 {
                // A notice coming or going: what it said cross-fades into it.
                if let Some(c) = t.canvas.as_mut() {
                    t.old = Some((c.copy_out(rect(0, 0, c.w, c.h)), c.w));
                    t.swap = Ease::at(0.0);
                    t.swap.go(1.0, 0.18);
                }
            }
            if !up {
                t.fresh = true;
                if t.shown.value() <= 0.0 {
                    t.hold = 1;
                }
            }
            t.alive = true;
            // Up evenly from its first frame, landing on time without a tail.
            t.rise.go(1.0, 0.26);
            t.shown.go(1.0, 0.22);
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
            t.rise.go(0.0, 0.2);
            t.shown.go(0.0, 0.18);
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
        tooltip::leave(t.hwnd);
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
    let Some((part, clock)) = TOAST.with(|c| {
        c.borrow_mut().as_mut().map(|t| {
            let part = pointed(t);
            let over = part.is_some();
            let now = Instant::now();
            let clock = if over { 0.0 } else { t.clock_at.map_or(0.0, |at| now.duration_since(at).as_secs_f32()) };
            t.clock_at = (!over && t.alive).then_some(now);
            (part, clock)
        })
    }) else {
        return false;
    };
    let over = part.is_some();
    let lefts = app::undo_clock(clock);
    TOAST.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(t) = slot.as_mut() else { return false };
        // Each card's own time; the top's is the ring's.
        for &(id, left) in &lefts {
            if let Some(c) = t.cards.iter_mut().find(|c| c.id == id && !c.gone) {
                c.left = left;
            }
        }
        if let Some(&(_, left)) = lefts.first() {
            t.left = left;
        }
        if t.notice.is_some() {
            t.notice_left -= clock;
            if t.notice_left <= 0.0 {
                t.notice = None;
                settle(t);
            }
        }
        let dt = if t.hold > 0 {
            t.hold -= 1;
            0.0
        } else if std::mem::take(&mut t.fresh) {
            dt.min(anim::period())
        } else {
            dt
        };
        let was = t.shown.value();
        let mut moving = t.rise.step(dt) | t.shown.step(dt) | t.swap.step(dt) | t.width.step(dt) | (dt == 0.0 && !t.shown.done());
        for c in &mut t.cards {
            moving |= c.place.step(dt) | c.fade.step(dt);
        }
        t.cards.retain(|c| !(c.gone && c.place.done() && c.fade.done()));
        if t.swap.done() {
            t.old = None;
        }
        if !over {
            t.pressed = None;
        }
        let undoing = t.alive && t.notice.is_none();
        t.hot.to(if undoing && part == Some(Part::Undo) { 1.0 } else { 0.0 });
        t.hot_close.to(if undoing && part == Some(Part::Close) { 1.0 } else { 0.0 });
        let lighting = t.hot.step(dt) | t.hot_close.step(dt);
        // Gone once a frame of nothing has shown.
        if !t.alive && t.shown.done() && was <= 0.0 {
            tooltip::leave(t.hwnd);
            unsafe { DestroyWindow(t.hwnd) };
            *slot = None;
            // Memory can go back to Windows once nothing else is open.
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
/// the seconds, and what's pressed.
fn drawn_key(t: &Toast) -> (i32, Option<Part>) {
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

/// What the pointer is on: the top card's × or the rest of it (not the cards behind or
/// the shadow around it).
fn pointed(t: &Toast) -> Option<Part> {
    let mut p = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(t.hwnd, &mut r);
    }
    part_in(t, p.x - r.left, p.y - r.top)
}

/// What window point (`x`, `y`) is on.
fn part_in(t: &Toast, x: i32, y: i32) -> Option<Part> {
    use crate::win::util::pt_in;
    if !pt_in(&t.capsule, x, y) {
        None
    } else if pt_in(&t.close, x, y) {
        Some(Part::Close)
    } else {
        Some(Part::Undo)
    }
}

fn part_at(x: i32, y: i32) -> Option<Part> {
    TOAST.with(|t| t.borrow().as_ref().and_then(|t| part_in(t, x, y)))
}

/// How a card at place `p` in the stack is drawn (see `Stacked::place`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    /// How far it's below its place on top, at 100% (behind, it peeks out above: less
    /// than 0).
    dy: f32,
    /// Its width, a share of the stack's.
    k: f32,
    /// How much of its capsule shows, and of what's on it.
    body: f32,
    content: f32,
    /// How dim it is: 0 on top, 1 two behind.
    dim: f32,
}

fn look(p: f32) -> Look {
    if p < 0.0 {
        // In front, coming in or going out: lower, and fading quicker than it moves.
        let f = clamp01(1.0 + p);
        Look { dy: -p * DROP, k: 1.0, body: f * f, content: f * f, dim: 0.0 }
    } else {
        // Behind: higher, narrower and dimmer a place at a time; what's on it fades out
        // within its first place, and the third behind is out of sight.
        let q = p.min(BEHIND);
        Look { dy: -q * PEEK, k: 1.0 - q * NARROW, body: clamp01(BEHIND + 1.0 - p), content: clamp01(1.0 - p), dim: q / BEHIND }
    }
}

/// Where a card at place `p` is drawn, and how much of it, once or twice: at `p` itself,
/// or with Reduce Motion at the whole places either side of it, the one fading into the
/// other, so nothing moves (one in front, at the top's place).
fn places(p: f32, reduced: bool) -> [(f32, f32); 2] {
    if !reduced {
        [(p, 1.0), (p, 0.0)]
    } else if p < 0.0 {
        let f = clamp01(1.0 + p);
        [(0.0, f * f), (0.0, 0.0)]
    } else {
        let lo = p.floor();
        [(lo, 1.0 - (p - lo)), (lo + 1.0, p - lo)]
    }
}

/// `a`'s colour `k` of the way to `b`'s.
fn mix(a: u32, b: u32, k: f32) -> u32 {
    let ch = |sh: u32| (lerp(((a >> sh) & 0xFF) as f32, ((b >> sh) & 0xFF) as f32, clamp01(k)).round() as u32) << sh;
    ch(16) | ch(8) | ch(0)
}

/// The Undo pill and the × on a card `w` by `h` at (`x`, `y`), its pill `bw` wide: the ×
/// at the right end, the pill just inside it.
fn parts(x: f32, y: f32, w: f32, h: f32, bw: f32, s: f32) -> (Shape, Shape) {
    let d = CLOSE * s;
    let close = Shape::new(x + w - 10.0 * s - d, y + (h - d) / 2.0, d, d, d / 2.0);
    let bh = 32.0 * s;
    let button = Shape::new(close.x - 4.0 * s - bw, y + (h - bh) / 2.0, bw, bh, bh / 2.0);
    (button, close)
}

fn render(t: &mut Toast) {
    let s = t.s;
    let (h, m, room) = ((52.0 * s) as i32, (24.0 * s) as i32, ((BEHIND * PEEK + 2.0) * s).ceil() as i32);
    let hf = h as f32;
    let msg = message(t).unwrap_or_else(|| t.said.0.clone());
    let notice = t.notice.is_some() || (t.undo.is_none() && t.said.1);
    // As wide as the widest card (one going keeps its own width) or the notice, with room
    // above for the cards peeking out behind; an even width, so the middle is a pixel's edge.
    let inner = if notice { t.notice_w } else { t.cards.iter().map(|c| c.size.0).max().unwrap_or(0).max(t.width.value().ceil() as i32) };
    let (cw, ch) = ((inner + 2 * m + 1) & !1, h + 2 * m + room);
    // Cross-fading from what it said, on a canvas wide enough for both, centred alike.
    let wide = t.old.as_ref().map_or(cw, |o| o.1.max(cw));
    if t.canvas.as_ref().map(|c| (c.w, c.h)) != Some((wide, ch)) {
        t.canvas = Some(Canvas::new(wide, ch));
    }
    let mut c = t.canvas.take().unwrap();
    c.clear(0);
    t.ring = None;
    let a = clamp01(t.shown.value());
    let lift = if anim::reduced() { 0.0 } else { ((1.0 - t.rise.value()) * 28.0 * s).round() };
    let (mid, y) = ((wide / 2) as f32, room as f32 + (m as f32 * 0.6).round() + lift);
    if notice {
        let w = t.notice_w;
        let x = (mid - w as f32 / 2.0).round();
        let card = Shape::new(x, y, w as f32, hf, hf / 2.0);
        c.soft_shadow(&card, s, 1.4 * a);
        c.shape(&card, 0x1C1C1E, 0.94 * a, 1.0, 0.0);
        c.shape(&card, 0xFFFFFF, 0.10 * a, 1.0, 1.0);
        c.begin_batch();
        c.text(&msg, &t.body, rect((x + 24.0 * s) as i32, y as i32, w - (48.0 * s) as i32, h), 0xF2F2F2, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        c.flush();
        t.capsule = rect(x as i32, y as i32, w, h);
        t.close = rect(0, 0, 0, 0);
    } else {
        // The cards from the back to the front: those behind, the top, then one coming in
        // or going out in front of it.
        let mut order: Vec<usize> = (0..t.cards.len()).collect();
        order.sort_by(|&i, &j| t.cards[j].place.value().total_cmp(&t.cards[i].place.value()));
        let reduced = anim::reduced();
        let settled = t.width.done() && t.cards.iter().all(|c| c.place.done() && c.fade.done());
        let mut ring = None;
        for i in order {
            let card = &t.cards[i];
            let top = !card.gone && card.place.target() == 0.0;
            for (p, weight) in places(card.place.value(), reduced) {
                if weight <= 0.0 {
                    continue;
                }
                let lk = look(p);
                // One going keeps its width, and so does the top with Reduce Motion, where
                // the stack's width changes at once; the rest are the stack's width.
                let own = card.gone || (reduced && p <= 0.0);
                let w = (if own { card.size.0 as f32 } else { t.width.value() } * lk.k).round();
                let x = (mid - w / 2.0).round();
                let drew = draw_card(&mut c, t, card, (x, (y + lk.dy * s).round(), w, hf), lk, a * weight * clamp01(card.fade.value()), top);
                if top && settled && weight >= 1.0 && a >= 0.999 && t.old.is_none() {
                    ring = drew;
                }
            }
        }
        t.ring = ring;
        // The top card where it's going takes the pointer.
        let w = t.width.target().round();
        let x = (mid - w / 2.0).round();
        let (_, close) = parts(x, y, w, hf, t.cards.iter().find(|c| !c.gone && c.place.target() == 0.0).map_or(0.0, |c| c.size.1), s);
        t.capsule = rect(x as i32, y as i32, w as i32, h);
        t.close = rect(close.x as i32, close.y as i32, close.w.ceil() as i32, close.h.ceil() as i32);
    }
    t.canvas = Some(c);
    present(t, wide, ch);
}

/// Card `card` drawn at `lk` in `at` (x, y, width, height), `a` of it showing: its capsule,
/// and what's on it as far as that shows. `top`, the one Undo takes back, lights up under
/// the pointer. The ring as drawn, for it to sweep on.
fn draw_card(c: &mut Canvas, t: &Toast, card: &Stacked, at: (f32, f32, f32, f32), lk: Look, a: f32, top: bool) -> Option<Ring> {
    let (s, (x, y, w, h)) = (t.s, at);
    let ba = a * lk.body;
    if ba <= 0.002 {
        return None;
    }
    let shape = Shape::new(x, y, w, h, h / 2.0);
    c.soft_shadow(&shape, s, 1.4 * ba);
    c.shape(&shape, mix(0x1C1C1E, 0x3A3A3C, 0.5 * lk.dim), 0.94 * (1.0 - 0.2 * lk.dim) * ba, 1.0, 0.0);
    c.shape(&shape, 0xFFFFFF, 0.10 * ba, 1.0, 1.0);
    let a = a * lk.content;
    if a <= 0.004 {
        return None;
    }
    let (hot, hot_close, pressed) = if top { (clamp01(t.hot.value), clamp01(t.hot_close.value), t.pressed) } else { (0.0, 0.0, None) };
    c.begin_batch();
    // Countdown ring: its track and the seconds, then (below, once the text is down) its arc.
    let left = card.left.clamp(0.0, SECONDS);
    let (rcx, rcy, rr) = (x + 26.0 * s, y + h / 2.0, 10.0 * s);
    c.ring(rcx, rcy, rr, 2.0 * s, 1.0, 0xFFFFFF, 0.16 * a);
    let secs = rect((rcx - rr) as i32, (rcy - rr) as i32, (2.0 * rr) as i32, (2.0 * rr) as i32);
    c.text(&format!("{}", left.ceil() as i32), &t.tiny, secs, 0xFFFFFF, 0.85 * a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let (button, close) = parts(x, y, w, h, card.size.1, s);
    let tx = (x + 48.0 * s) as i32;
    let tw = (button.x - 12.0 * s) as i32 - tx;
    c.text(&card.text, &t.body, rect(tx, y as i32, tw, h as i32), 0xF2F2F2, a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    // Brighter under the pointer; pressed, it sinks a little and brightens a touch more.
    let undo_down = pressed == Some(Part::Undo);
    let fill = if undo_down { button.inset(1.5 * s) } else { button };
    let lit = 0.12 * hot + if undo_down { 0.08 } else { 0.0 };
    c.shape(&fill, 0xFFFFFF, (0.14 + lit) * a, 1.0, 0.0);
    let br = rect(button.x as i32, button.y as i32, button.w as i32, button.h as i32);
    c.text(t!("toast_undo"), &t.bold, br, theme::COLORS[0].accent, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    // The ×: a faint disc behind it under the pointer, sinking a little when pressed.
    let close_down = pressed == Some(Part::Close);
    let disc = if close_down { close.inset(1.5 * s) } else { close };
    c.shape(&disc, 0xFFFFFF, (0.14 * hot_close + if close_down { 0.08 } else { 0.0 }) * a, 1.0, 0.0);
    let (ccx, ccy, arm) = (close.x + close.w / 2.0, close.y + close.h / 2.0, 10.0 * s);
    for turn in [std::f32::consts::FRAC_PI_4, -std::f32::consts::FRAC_PI_4] {
        let bar = Shape::new(ccx - arm / 2.0, ccy - 0.75 * s, arm, 1.5 * s, 0.75 * s).rotated(turn, ccx, ccy);
        c.shape(&bar, 0xFFFFFF, (0.62 + 0.3 * hot_close) * a, 1.0, 0.0);
    }
    c.flush();
    let reach = (rr + 2.0 * s + 2.0).ceil() as i32;
    let square = rect(rcx as i32 - reach, rcy as i32 - reach, 2 * reach + 1, 2 * reach + 1);
    let under = c.copy_out(square);
    let arc = std::f32::consts::TAU * rr * left / SECONDS;
    draw_arc(c, rcx, rcy, rr, s, left / SECONDS, a);
    Some(Ring { cx: rcx, cy: rcy, r: rr, square, under, arc })
}

/// What the toast says: the notice while there is one, else what Undo takes back.
fn message(t: &Toast) -> Option<String> {
    t.notice.clone().or_else(|| t.undo.clone())
}

/// The capsule's width and its Undo pill's, in pixels at scale `s`, for a message and a
/// button label measured `msg_w` and `label_w` wide: the pill fits its label (German and
/// Hindi run longer than "Undo") and the capsule grows, within reason, to keep the
/// message beside it and the × after it.
fn sizes(s: f32, msg_w: i32, label_w: i32) -> (i32, f32) {
    let bw = (label_w as f32 + 2.0 * 16.0 * s).max(76.0 * s).ceil();
    let need = (48.0 + 12.0 + 4.0 + CLOSE + 10.0) * s + msg_w as f32 + bw;
    (need.clamp(320.0 * s, 560.0 * s).ceil() as i32, bw)
}

/// Each card measured for what it says (`sizes`; one on its way out keeps its width) and
/// the stack as wide as its top card, and the notice. A notice has no ring or button: the
/// capsule hugs its text, centred, rather than leaving the Undo toast's room empty beside
/// it.
fn measure(t: &mut Toast) {
    let s = t.s;
    let label = t.bold.measure(t!("toast_undo")).0;
    for c in t.cards.iter_mut().filter(|c| !c.gone) {
        c.size = sizes(s, t.body.measure(&c.text).0, label);
    }
    if let Some(top) = t.cards.iter().find(|c| !c.gone && c.place.target() == 0.0) {
        let w = top.size.0 as f32;
        // With Reduce Motion it doesn't grow or shrink either: the top card is at its own
        // width, cross-fading with the one before (see `render`).
        if std::mem::take(&mut t.at_once) || anim::reduced() {
            t.width = Ease::at(w);
        } else {
            t.width.go(w, MOVE);
        }
    }
    if let Some(msg) = &t.notice {
        t.notice_w = ((t.body.measure(msg).0 as f32 + 48.0 * s).clamp(140.0 * s, 560.0 * s)).ceil() as i32;
    }
}

/// Put the drawn toast at the bottom of the work area.
fn present(t: &mut Toast, cw: i32, ch: i32) {
    let (px, py) = crate::win::ui::screen::bottom_centre(t.work, cw, ch, (12.0 * t.s) as i32);
    t.placed = (px, py);
    if let Some(c) = t.canvas.as_mut() {
        if let Some((old, ow)) = t.old.as_ref() {
            // What it said before, centred on the canvas, fading out as the new fades in.
            let mut was = vec![0u32; (cw * ch) as usize];
            let (dx, rows) = ((cw - ow) / 2, (old.len() as i32 / ow.max(&1)).min(ch));
            for y in 0..rows {
                let (src, dst) = ((y * ow) as usize, (y * cw + dx.max(0)) as usize);
                let n = (*ow).min(cw) as usize;
                let from = src + (-dx).max(0) as usize;
                was[dst..dst + n].copy_from_slice(&old[from..from + n]);
            }
            c.mix_in(rect(0, 0, cw, ch), &was, clamp01(t.swap.value()));
        }
        c.present(t.hwnd, px, py);
    }
    unsafe {
        if IsWindowVisible(t.hwnd) == 0 {
            ShowWindow(t.hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// The pointer at window point (`x`, `y`): on the ×, its tooltip; elsewhere, none.
fn tip(h: HWND, x: i32, y: i32) {
    let close = TOAST.with(|t| t.borrow().as_ref().and_then(|t| (part_in(t, x, y) == Some(Part::Close)).then_some(t.close)));
    match close {
        Some(r) => tooltip::hover_over(h, r, t!("toast_dismiss"), ""),
        None => tooltip::leave(h),
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
                tip(h, x_of(lp), y_of(lp));
                anim::wake(Client::Toast);
                return 0;
            }
            WM_MOUSELEAVE => {
                tooltip::leave(h);
                anim::wake(Client::Toast);
                return 0;
            }
            // A hand on the capsule, the usual arrow on the shadow around it.
            WM_SETCURSOR => {
                let mut p = POINT { x: 0, y: 0 };
                let mut r: RECT = std::mem::zeroed();
                GetCursorPos(&mut p);
                GetWindowRect(h, &mut r);
                SetCursor(LoadCursorW(null_mut(), if part_at(p.x - r.left, p.y - r.top).is_some() { IDC_HAND } else { IDC_ARROW }));
                return 1;
            }
            // Shown on the press, done on the release, on the capsule only.
            WM_LBUTTONDOWN => {
                let Some(part) = part_at(x_of(lp), y_of(lp)) else { return 0 };
                if part == Part::Close {
                    tooltip::mute();
                }
                TOAST.with(|t| {
                    if let Some(t) = t.borrow_mut().as_mut() {
                        t.pressed = Some(part);
                    }
                });
                anim::wake(Client::Toast);
                return 0;
            }
            WM_LBUTTONUP => {
                let part = part_at(x_of(lp), y_of(lp));
                let (pressed, notice) = TOAST.with(|t| t.borrow_mut().as_mut().map_or((None, false), |t| (t.pressed.take(), t.notice.is_some())));
                anim::wake(Client::Toast);
                match part {
                    None => {}
                    // A notice has nothing to undo: a click just dismisses it (and shows
                    // what can still be undone, if anything).
                    Some(_) if notice => with_toast(None, |t| t.notice = None),
                    // The × makes the top one final, pressed and let go on it; the rest of
                    // the card undoes, unless the press began on the ×.
                    Some(Part::Close) if pressed == Some(Part::Close) => app::dismiss_undo(),
                    Some(Part::Undo) if pressed != Some(Part::Close) => app::undo(),
                    Some(_) => {}
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
    use super::{BEHIND, CLOSE, draw_arc, look, places, sizes};
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
    fn the_rings_end_moves_a_little_every_frame_rather_than_a_pixel_at_a_time() {
        // A frame at 100 Hz: the end moves 0.06 px along the ring.
        let step = 0.06 / (std::f32::consts::TAU * 10.0);
        let draw = |frac: f32| {
            let mut c = capsule();
            draw_arc(&mut c, 30.0, 30.0, 10.0, 1.0, frac, 1.0);
            c.copy_out(rect(0, 0, 80, 60))
        };
        let mut changed = 0;
        let mut frac = 0.6;
        for _ in 0..20 {
            let (a, b) = (draw(frac), draw(frac - step));
            let most = a.iter().zip(&b).map(|(p, q)| (0..4).map(|k| ((p >> (8 * k)) & 0xFF).abs_diff((q >> (8 * k)) & 0xFF)).max().unwrap()).max().unwrap();
            // Never a pixel turning over whole in one frame.
            assert!(most < 40, "{most}");
            changed += (most > 0) as i32;
            frac -= step;
        }
        assert!(changed >= 18, "{changed} of 20 frames moved it");
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
    fn the_undo_pill_and_the_x_fit_beside_the_message() {
        // A short label keeps the usual pill and capsule.
        assert_eq!(sizes(1.0, 120, 30), (320, 76.0));
        // "पूर्ववत करें" or "Widerrufen" in bold: the pill grows past its label, the capsule
        // keeps the message beside it and the × after it, uncut.
        let (w, bw) = sizes(1.0, 150, 90);
        assert_eq!(bw, 122.0);
        assert!(w as f32 >= 48.0 + 150.0 + 12.0 + bw + 4.0 + CLOSE + 10.0);
        // At 150% everything scales, and a very long message stops growing the capsule.
        assert_eq!(sizes(1.5, 180, 45), (480, 114.0));
        let (w, bw) = sizes(1.5, 260, 45);
        assert!(w as f32 >= 1.5 * (48.0 + 12.0 + 4.0 + CLOSE + 10.0) + 260.0 + bw);
        assert_eq!(sizes(1.0, 2000, 30).0, 560);
    }

    /// The cards peek out behind the top one, each higher, narrower and dimmer, with
    /// nothing on them; the third behind is out of sight, ready to come up.
    #[test]
    fn the_cards_behind_peek_out_narrower_and_dimmer() {
        let (top, one, two, hidden) = (look(0.0), look(1.0), look(2.0), look(3.0));
        assert_eq!((top.dy, top.k, top.body, top.content, top.dim), (0.0, 1.0, 1.0, 1.0, 0.0));
        assert!(one.dy < 0.0 && two.dy < one.dy && two.dy == -BEHIND * super::PEEK);
        assert!(one.k < 1.0 && two.k < one.k);
        assert!(one.dim > 0.0 && two.dim > one.dim);
        assert!(one.body == 1.0 && two.body == 1.0 && one.content == 0.0 && two.content == 0.0);
        assert_eq!(hidden.body, 0.0);
        // One out in front, coming in or gone, is out of sight below the top's place.
        let out = look(-1.0);
        assert!(out.body == 0.0 && out.content == 0.0 && out.dy > 0.0);
    }

    /// A card moving between places goes through every look between them a little at a
    /// time: never a jump in a frame.
    #[test]
    fn a_card_moves_between_places_without_a_jump() {
        let mut p = -1.0f32;
        let mut was = look(p);
        while p < 3.0 {
            p += 0.01;
            let now = look(p);
            assert!((now.dy - was.dy).abs() <= 0.2 && (now.k - was.k).abs() <= 0.002, "{p}");
            assert!((now.body - was.body).abs() <= 0.03 && (now.content - was.content).abs() <= 0.03 && (now.dim - was.dim).abs() <= 0.01, "{p}");
            was = now;
        }
    }

    /// With Reduce Motion a card is drawn only at whole places, one fading into the next as
    /// it would have moved: nothing moves, and it's whole at its place.
    #[test]
    fn with_reduce_motion_the_cards_only_cross_fade() {
        for k in 0..=300 {
            let p = -1.0 + k as f32 * 0.0133;
            let drawn = places(p, true);
            for (at, _) in drawn {
                assert_eq!(at, at.round(), "{p}");
            }
            if p >= 0.0 {
                assert!((drawn[0].1 + drawn[1].1 - 1.0).abs() < 1e-4, "{p}");
            }
        }
        assert_eq!(places(1.0, true), [(1.0, 1.0), (2.0, 0.0)]);
        assert_eq!(places(-1.0, true)[0].1, 0.0);
        assert_eq!(places(0.6, false), [(0.6, 1.0), (0.6, 0.0)]);
    }
}
