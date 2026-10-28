// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ghost transitions: the card that travels between the deck and a note window.
//!
//! Opening, the note's card grows from where it was (its tab, the plus button, the pill)
//! into the window's frame; the real window is revealed underneath on arrival and the
//! ghost dissolves. Closing runs the same path in reverse, so a note always returns to
//! where it came from (spatial consistency).

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::util::{rect, wide};

const CLASS: &str = "PeekABooGhost";

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    /// Grow into a window; `on_arrive` reveals it.
    Open,
    /// Return home (tab / pill).
    Close,
    /// Sink and dissolve in place (delete, archive).
    Dissolve,
}

/// What a flying card shows.
pub struct Content {
    pub title: String,
    pub body: String,
    /// Where the body starts, in px below the card's top, when it's a scrolled window's
    /// view (no title then); None for the title and the top of the text.
    pub body_top: Option<f32>,
    /// The window itself, pixel for pixel: shown instead of the text when there is one,
    /// so the card turns into the window (or the window into the card) without a jump.
    pub image: Option<crate::win::render::gfx::Image>,
    /// Opening: the deck card it takes off from, pixel for pixel, cross-faded into `image`
    /// so the content never blinks out on the way.
    pub from_image: Option<crate::win::render::gfx::Image>,
}

struct Ghost {
    hwnd: HWND,
    kind: Kind,
    /// Where the springs' coordinates start on screen: the top-left of the flight's path.
    origin: (i32, i32),
    /// The room around the card in its window, for its shadow.
    pad: i32,
    canvas: Canvas,
    x: Spring,
    y: Spring,
    w: Spring,
    h: Spring,
    r: Spring,
    fade: Spring,
    start_area: f32,
    end_area: f32,
    color: u8,
    title: String,
    body: String,
    body_top: Option<f32>,
    image: Option<crate::win::render::gfx::Image>,
    from_image: Option<crate::win::render::gfx::Image>,
    s: f32,
    title_font: Font,
    body_font: Font,
    on_arrive: Option<Box<dyn FnOnce()>>,
    arrived: bool,
    /// Its first frame, the pose it sets off from, is up but not yet on screen: the next
    /// frame leaves it there to be seen, and the flight moves from the one after. Taking
    /// over from a window (closing), whose place it takes in that frame; opening, it's what
    /// it leaves (the deck card, the plus button, the capsule) already on screen, and it
    /// moves from the next frame.
    fresh: bool,
    /// Frames the card stays whole over the window it has just revealed, which may take a
    /// frame to come up, before it fades.
    hold: u8,
    /// How many frames it has moved: its first `START` steps are a refresh each, however
    /// long the frames before them took (the window it opens into made just before it set
    /// off, and painted while its first frames show), so it never sets off with a leap.
    moves: u8,
    /// Growing out of a round button (the deck's plus): where its centre is on screen. The
    /// button's picture shrinks and fades there in the card's first few frames, the button
    /// turning into the card, rather than riding the card's corner for half the flight.
    button: Option<(f32, f32)>,
    /// How long it has been moving, in seconds.
    flown: f32,
    /// Fading in where the window comes up, rather than flying out of something.
    appear: bool,
    /// When the message that set it flying was taken, and when its first frame was put up
    /// (for PEEKABOO_PERF's line on how soon it moved).
    lag: Option<(f64, f64)>,
    /// The window it opens into, while its picture is still to be had, and the frames tried.
    /// The card sets off before the window is made, and the compositor has a window just
    /// made a frame or so later; meanwhile it's paper.
    awaiting: Option<(HWND, u8)>,
    /// How much of that picture shows, as it comes in late.
    picture_in: Spring,
    /// Whether the window it is (opening into, or closing from) has Windows' shadow for an
    /// active window, the bigger and darker one, or an inactive one's.
    active: bool,
    /// The window it opens into (`adopt`), null for none.
    window: HWND,
    /// Opening, it has arrived and its frame there is to be put up by the window as it
    /// shows (`landed`), in the same frame, so its shadow and the window's swap without one
    /// with neither or both.
    landing: bool,
    /// Room to draw the window's shadow in (`window_shadow`), kept from frame to frame.
    shade: Vec<u32>,
}

/// Frames a card tries for the picture of the window it opens into, before it does without.
const PICTURE_TRIES: u8 = 30;

/// The frames a card's flight starts with that step a refresh each, whenever they come
/// (about 60 to 100 ms: while the window it opens into settles in).
const START: u8 = 6;

/// How much longer a flight takes for its distance (`d` px between the centres where it
/// starts and ends, at scale `s`): as long as from a deck card to the window beside it
/// (about 350 to 650 px) up to 700 px, longer beyond, but less than in proportion, so a
/// card crossing a wide display goes faster but never streaks (2,500 px opening went up to
/// 290 px a frame at 60 Hz; now under 130, and closing under 140).
fn far(d: f32, s: f32) -> f32 {
    (d / (700.0 * s)).max(1.0).powf(0.7).min(2.6)
}

/// How near its end a card has to be to have arrived, in px. A card that opens into a
/// window arrives once its picture sits on the window's own pixels (it's drawn on whole
/// ones), so it hands over to the window without a jump. One flying home is all but faded
/// by then, so it needn't come as close.
fn arrive_within(kind: Kind) -> f32 {
    if kind == Kind::Open { 0.5 } else { 1.5 }
}

/// What a card's frame did.
#[derive(PartialEq, Debug)]
enum Step {
    /// Nothing to draw: its first frame is showing still, or it holds over its window.
    Still,
    /// It moved: draw it.
    Moved,
    /// It got there this frame, exactly at its end.
    Arrived,
}

thread_local! {
    static GHOSTS: RefCell<Vec<Ghost>> = const { RefCell::new(Vec::new()) };
}

/// The room around a card at scale `s` for its shadow, which is a window's (Windows' own,
/// reaching about 80 px below it) where the card is the window (`window_shadow`).
fn pad_of(s: f32) -> i32 {
    (84.0 * s) as i32
}

/// One part of Windows' shadow under a note window: the window's rect grown `side` px at its
/// sides, its top moved down `top` px and its bottom `bottom` px, blurred (Gaussian, `sx` px
/// across, `sy` up and down) and laid on at strength `k`. In px at 100%.
struct Blur {
    k: f32,
    sx: f32,
    sy: f32,
    side: f32,
    top: f32,
    bottom: f32,
}

/// Windows' own shadow under a note window, active, as it shows on screen (Windows 11, a 320
/// by 360 note on plain grey; fitted to within 0.025 of its strength everywhere, four levels
/// on mid grey, and 0.004 on average): one lower down, a darker one close in, and a wide one
/// darkening it towards the bottom. It reaches about 80 px below the window, 45 beside it and
/// 30 above. A card's own (about 18 px below, 9 beside and none above), made bigger, was 6
/// levels off above the window and 5 below: the shadow jumped in the frame the two handed
/// over in.
const ACTIVE: [Blur; 3] = [
    Blur { k: 0.09, sx: 15.09, sy: 15.15, side: 9.10, top: 150.0, bottom: 46.88 },
    Blur { k: 0.19, sx: 13.56, sy: 13.48, side: 3.56, top: 0.61, bottom: 16.13 },
    Blur { k: 0.11, sx: 49.96, sy: 27.08, side: -54.88, top: 33.85, bottom: 16.70 },
];

/// Windows' shadow under an inactive note window (measured as `ACTIVE`, from fewer pixels:
/// above and beside its top, and below it): as far below, but lighter, and about half as
/// far beside and above it.
const INACTIVE: [Blur; 3] = [
    Blur { k: 0.11, sx: 6.66, sy: 19.02, side: -4.44, top: 41.92, bottom: 35.05 },
    Blur { k: 0.15, sx: 9.48, sy: 7.24, side: -0.18, top: 1.11, bottom: 2.09 },
    Blur { k: 0.05, sx: 50.69, sy: 21.09, side: -52.59, top: 52.42, bottom: 28.52 },
];

/// The standard normal distribution up to `z` (Abramowitz and Stegun's erf, 7.1.26).
fn phi(z: f32) -> f32 {
    let x = z.abs() * std::f32::consts::FRAC_1_SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = ((((1.061_405_4 * t - 1.453_152) * t + 1.421_413_7) * t - 0.284_496_74) * t + 0.254_829_6) * t;
    let erf = 1.0 - poly * (-x * x).exp();
    if z >= 0.0 { 0.5 + 0.5 * erf } else { 0.5 - 0.5 * erf }
}

/// Windows' shadow for a window at `sh` on canvas `c` (just cleared), at scale `s`, `active`
/// or not, at `strength` (0 to 1), drawn by way of `buf`. The parts' offsets are for a note
/// of its usual size; on a smaller one they keep in proportion to it.
fn window_shadow(c: &mut Canvas, sh: &Shape, s: f32, active: bool, strength: f32, buf: &mut Vec<u32>) {
    let (cw, ch) = (c.w as usize, c.h as usize);
    let parts = if active { &ACTIVE } else { &INACTIVE };
    // A part's strength at a pixel is its column's share times its row's.
    let mut cols = vec![[0.0f32; 3]; cw];
    let mut rows = vec![[0.0f32; 3]; ch];
    for (i, b) in parts.iter().enumerate() {
        let side = if b.side < 0.0 { (b.side * s).max(-0.17 * sh.w) } else { b.side * s };
        let (l, r) = (sh.x - side, sh.x + sh.w + side);
        let (t, bottom) = (sh.y + (b.top * s).min(0.42 * sh.h), sh.y + sh.h + b.bottom * s);
        let (sx, sy) = (b.sx * s, b.sy * s);
        for (x, col) in cols.iter_mut().enumerate() {
            let px = x as f32 + 0.5;
            col[i] = phi((px - l) / sx) - phi((px - r) / sx);
        }
        for (y, row) in rows.iter_mut().enumerate() {
            let py = y as f32 + 0.5;
            row[i] = b.k * (phi((py - t) / sy) - phi((py - bottom) / sy));
        }
    }
    // Under the card's paper, away from its edge and corners, it never shows: drawn around
    // it, above and below it and at its sides, not under it (half the canvas, for a big note).
    let r = sh.r.max(0.0) + 2.0;
    let clampx = |v: f32| (v.max(0.0) as usize).min(cw);
    let clampy = |v: f32| (v.max(0.0) as usize).min(ch);
    let (x0, x1) = (clampx((sh.x + 2.0).ceil()), clampx((sh.x + sh.w - 2.0).floor()));
    let (y0, y1) = (clampy((sh.y + r).ceil()), clampy((sh.y + sh.h - r).floor()));
    let (x1, y1) = (x1.max(x0), y1.max(y0));
    for (l, t, rr, b) in [(0, 0, cw, y0), (0, y1, cw, ch), (0, y0, x0, y1), (x1, y0, cw, y1)] {
        if rr <= l || b <= t {
            continue;
        }
        buf.clear();
        buf.reserve((rr - l) * (b - t));
        for row in &rows[t..b] {
            // Beyond its reach, above or below: nothing.
            if row.iter().all(|v| *v < 1e-4) {
                buf.resize(buf.len() + rr - l, 0);
                continue;
            }
            for col in &cols[l..rr] {
                let keep = (1.0 - row[0] * col[0]) * (1.0 - row[1] * col[1]) * (1.0 - row[2] * col[2]);
                let a = (strength * (1.0 - keep) * 255.0 + 0.5) as u32;
                buf.push(a.min(255) << 24);
            }
        }
        c.copy_in(RECT { left: l as i32, top: t as i32, right: rr as i32, bottom: b as i32 }, buf);
    }
}

fn union(a: &RECT, b: &RECT, pad: i32) -> RECT {
    RECT { left: a.left.min(b.left) - pad, top: a.top.min(b.top) - pad, right: a.right.max(b.right) + pad, bottom: a.bottom.max(b.bottom) + pad }
}

/// The flying card's window for a flight between `a` and `b`: as big as the bigger of the
/// two with room for a spring's overshoot and `pad` around for the shadow. It moves with
/// the card, so a flight across two monitors never needs a canvas the size of both.
fn window_size(a: &RECT, b: &RECT, pad: i32) -> (i32, i32) {
    let w = (a.right - a.left).max(b.right - b.left).max(1);
    let h = (a.bottom - a.top).max(b.bottom - b.top).max(1);
    ((w as f32 * 1.1).ceil() as i32 + 2 * pad, (h as f32 * 1.1).ceil() as i32 + 2 * pad)
}

/// Fly a card from `from` to `to` (screen rects with corner radii). Its first frame is up
/// when this returns; the card, to `adopt` a window, unless motion is reduced.
pub fn fly(kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, on_arrive: Option<Box<dyn FnOnce()>>) -> Option<HWND> {
    set_off(kind, from, to, color, content, on_arrive, false, true)
}

/// `fly`, from a window (closing it, or sinking where it was) that is `active` or not: the
/// card sets off with that window's shadow, which is smaller for an inactive window.
pub fn fly_from(kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, active: bool) -> Option<HWND> {
    set_off(kind, from, to, color, content, None, false, active)
}

/// A card for a window opening at `to` with nothing to fly out of (Reopen Last Note for a
/// note not in the deck): it fades in where the window comes up, growing the last few
/// pixels into it, and the window is revealed under it as from a flight. Shown at once
/// instead, the window's first frame could be its empty surface, see-through but for its
/// border. To `adopt` the window, as `fly`'s.
pub fn appear(to: (RECT, f32), color: u8, content: Content) -> Option<HWND> {
    let r = to.0;
    let (dx, dy) = ((r.right - r.left) * 3 / 100, (r.bottom - r.top) * 3 / 100);
    let from = RECT { left: r.left + dx, top: r.top + dy, right: r.right - dx, bottom: r.bottom - dy };
    set_off(Kind::Open, (from, to.1), to, color, content, None, true, true)
}

/// `active`: the window opened into or flown from has Windows' shadow for an active window
/// (one opening always has: it's shown activated).
#[allow(clippy::too_many_arguments)]
fn set_off(kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, on_arrive: Option<Box<dyn FnOnce()>>, appear: bool, active: bool) -> Option<HWND> {
    if anim::reduced() {
        if let Some(f) = on_arrive {
            f();
        }
        return None;
    }
    // The card shows what the note window draws, so it takes the scale of the monitor the
    // window is on (it may not be the deck's): where it opens, or where it closes from.
    let window = if kind == Kind::Open { &to.0 } else { &from.0 };
    let (_, s) = crate::win::ui::screen::of_rect(window);
    let pad = pad_of(s);
    let (ww, wh) = window_size(&from.0, &to.0, pad);
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            wide(CLASS).as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            from.0.left - pad,
            from.0.top - pad,
            ww,
            wh,
            null_mut(),
            null_mut(),
            app::hinst(),
            null_mut(),
        )
    };
    crate::win::util::no_transitions(hwnd);
    // Under a card flying open (a note closing as another opens passes behind the one
    // coming out, rather than jumping in front of it), and under a tooltip fading out (the
    // plus button's, as a new note grows out of it).
    let under = GHOSTS.with(|gs| gs.borrow().iter().rev().find(|g| kind != Kind::Open && g.kind == Kind::Open && !done(g)).map(|g| g.hwnd));
    let under = under.unwrap_or_else(crate::win::ui::tooltip::window);
    if !under.is_null() && unsafe { IsWindowVisible(under) } != 0 {
        unsafe { SetWindowPos(hwnd, under, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }
    let mut g = card(hwnd, kind, from, to, color, content, s);
    g.on_arrive = on_arrive;
    g.active = active;
    if appear {
        g.appear = true;
        g.fade.snap(0.0);
        g.fade.to(1.0);
    }
    render(&mut g);
    if anim::perf_on() {
        g.lag = Some((anim::message_at(), crate::win::render::vblank::now()));
    }
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    GHOSTS.with(|gs| gs.borrow_mut().push(g));
    anim::wake(Client::Ghost);
    // Its frames keep to the refreshes of the display the window is on.
    anim::pace_on(window);
    Some(hwnd)
}

/// Card `ghost`, flying open, opens into `window`, made after it set off (so it set off at
/// once): it takes the window's picture as soon as the compositor has it, and `on_arrive`
/// runs when it gets there. With no card (motion reduced), `on_arrive` runs now.
pub fn adopt(ghost: Option<HWND>, window: HWND, on_arrive: Option<Box<dyn FnOnce()>>) {
    let left = GHOSTS.with(|gs| {
        let mut gs = gs.borrow_mut();
        match gs.iter_mut().find(|g| Some(g.hwnd) == ghost && !g.arrived) {
            Some(g) => {
                // With the window's picture already (drawn as it was made), nothing to wait for.
                g.awaiting = (!window.is_null() && g.image.is_none()).then_some((window, 0));
                g.on_arrive = on_arrive;
                g.window = window;
                None
            }
            None => on_arrive,
        }
    });
    if let Some(f) = left {
        f();
    }
}

/// A card waiting for its window's picture tries once more (without waiting): got, it comes
/// in, faded in when the card shows it already, at once when it doesn't yet.
fn fetch_picture(g: &mut Ghost) {
    let Some((window, tried)) = g.awaiting else { return };
    let picture = if unsafe { IsWindow(window) } != 0 { crate::win::render::gfx::capture_now(window) } else { None };
    if picture.is_none() && tried + 1 < PICTURE_TRIES {
        g.awaiting = Some((window, tried + 1));
        return;
    }
    received(g, picture);
}

/// The picture a card was waiting for came (or never will: None).
fn received(g: &mut Ghost, picture: Option<crate::win::render::gfx::Image>) {
    if picture.is_some() {
        g.image = picture;
    }
    g.awaiting = None;
    // What the card would show by now comes in over a moment rather than in one frame.
    g.picture_in.snap(0.0);
    g.picture_in.to(1.0);
    if look(g).content <= 0.002 {
        g.picture_in.snap(1.0);
    }
}

/// The card flying from `from` to `to` at scale `s`, drawn in `hwnd`.
fn card(hwnd: HWND, kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, s: f32) -> Ghost {
    let pad = pad_of(s);
    let b = union(&from.0, &to.0, pad);
    let (ww, wh) = window_size(&from.0, &to.0, pad);
    let centre = |r: &RECT| ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
    let ((ax, ay), (bx, by)) = (centre(&from.0), centre(&to.0));
    let far = far((bx - ax).hypot(by - ay), s);
    let params = match kind {
        // Set off moving (see below), a little longer, so it's no faster at its fastest.
        Kind::Open => (0.86, 0.45 * far),
        Kind::Close => (1.0, 0.3 * far),
        Kind::Dissolve => (1.0, 0.26),
    };
    let lx = |r: &RECT| (r.left - b.left) as f32;
    let ly = |r: &RECT| (r.top - b.top) as f32;
    let (f, t) = (&from.0, &to.0);
    let mut g = Ghost {
        hwnd,
        kind,
        origin: (b.left, b.top),
        pad,
        canvas: Canvas::new(ww, wh),
        x: Spring::px(lx(f), params),
        y: Spring::px(ly(f), params),
        w: Spring::px((f.right - f.left) as f32, params),
        h: Spring::px((f.bottom - f.top) as f32, params),
        r: Spring::px(from.1, params),
        fade: Spring::unit(1.0, theme::motion::SNAPPY),
        start_area: ((f.right - f.left) * (f.bottom - f.top)) as f32,
        end_area: ((t.right - t.left) * (t.bottom - t.top)) as f32,
        color,
        title: content.title,
        body: content.body.chars().take(900).collect(),
        body_top: content.body_top,
        image: content.image,
        from_image: content.from_image,
        s,
        // The note's own typography, so the flight matches the window it becomes.
        title_font: crate::win::editor::rich::preview_fonts(s).0,
        body_font: crate::win::editor::rich::preview_fonts(s).1,
        on_arrive: None,
        arrived: false,
        fresh: kind != Kind::Open,
        hold: 0,
        moves: 0,
        appear: false,
        button: None,
        flown: 0.0,
        lag: None,
        awaiting: None,
        picture_in: Spring::unit(1.0, theme::motion::DEFAULT),
        active: true,
        window: null_mut(),
        landing: false,
        shade: Vec::new(),
    };
    // A round button it grows out of: a square start, its radius half its side, and its
    // picture the start's size.
    let side = f.right - f.left;
    let round = side == f.bottom - f.top && from.1 * 2.0 >= side as f32 - 1.0;
    if round && g.from_image.as_ref().is_some_and(|i| i.w == side && i.h == side) {
        g.button = Some(((f.left + f.right) as f32 / 2.0, (f.top + f.bottom) as f32 / 2.0));
    }
    let ends = [lx(t), ly(t), (t.right - t.left) as f32, (t.bottom - t.top) as f32, to.1];
    for (sp, end) in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r].into_iter().zip(ends) {
        // Opening, it's in hand already (the click took the deck card, the plus button):
        // it sets off moving, at a pace that only slows from there, its first steps as long
        // as the ones after them. Let go from rest, a spring's second step is twice its
        // first or more, a leap just as it sets off. A window flying home sets off from
        // rest, gathering speed.
        if kind == Kind::Open {
            sp.launch(end);
        } else {
            sp.to(end);
        }
    }
    g
}

/// 0 at the start of the flight, 1 on arrival (by size, so it is robust to overshoot).
fn progress(g: &Ghost) -> f32 {
    let area = g.w.value * g.h.value;
    if (g.end_area - g.start_area).abs() < 1.0 {
        let total = (g.x.target - g.x.value).abs() + (g.y.target - g.y.value).abs();
        return 1.0 - clamp01(total / 200.0);
    }
    clamp01((area - g.start_area) / (g.end_area - g.start_area))
}

fn render(g: &mut Ghost) {
    let (x, y, alpha) = paint(g, app::dark());
    g.canvas.present_faded(g.hwnd, x, y, alpha);
}

/// How strongly a card shows, by how far it has come.
#[derive(Clone, Copy, Debug)]
struct Look {
    /// The card itself: its paper and shadow.
    card: f32,
    /// The deck card it opens from, fading out.
    from: f32,
    /// The window's picture (or its words, without one).
    content: f32,
}

/// 0 before `a`, 1 after `b`, easing in and out between: a fade that starts and ends at
/// rest, as the motion it's part of does, never with a step.
fn ease(p: f32, a: f32, b: f32) -> f32 {
    let t = clamp01((p - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}

fn look(g: &Ghost) -> Look {
    let p = progress(g);
    let card = match g.kind {
        Kind::Open => g.fade.value,
        Kind::Close => 1.0 - ease(p, 0.7, 1.0),
        Kind::Dissolve => 1.0 - ease(p, 0.0, 1.0),
    };
    // Content sits where the note window draws it; the growing card reveals it.
    let content = match (g.kind, g.from_image.is_some() && g.image.is_some()) {
        // Opening, it's paper until the window's picture comes.
        (Kind::Open, _) if g.awaiting.is_some() => 0.0,
        // Fading in where the window comes up: the window's picture, as one.
        (Kind::Open, _) if g.appear => card,
        // From the deck card into the window: the window's picture comes in over the deck
        // card's, which stays whole under it until it's covered. The words are as strong as
        // the two together all the way; the deck card's fading out before the window's came
        // in, for some frames neither showed strongly, the card's content washed out. Quickly,
        // in a frame or two of its fastest stretch: the two are laid out differently (the
        // deck card's title beside its label, the window's under its header), and over a
        // longer stretch (from 30% to 65% of the way) both showed half through each other,
        // the title and the list doubled, for four frames.
        (Kind::Open, true) if g.button.is_none() => card * ease(p, 0.38, 0.54),
        (Kind::Open, true) => card * ease(p, 0.35, 0.9),
        (Kind::Open, false) => card * ease(p, 0.25, 0.85),
        // Archived or deleted, the note fades as one, words and all: never an empty card.
        (Kind::Dissolve, _) => card,
        (Kind::Close, _) => card * (1.0 - ease(p, 0.0, 0.6)),
    } * g.picture_in.value;
    let from = match (g.kind, g.from_image.is_some(), g.button) {
        (Kind::Open, true, Some(_)) => card * (1.0 - button_gone(g)),
        // Under the window's picture, whole, until that covers it.
        (Kind::Open, true, None) if content < 0.999 * card => card,
        (Kind::Open, true, None) => 0.0,
        _ => 0.0,
    };
    Look { card, from, content }
}

/// How much of a card's shadow is the window's (`window_shadow`), 0 to 1, by how far it has
/// come: all of it where it is the window (landing, or setting off from it), none from
/// halfway on, where it's a card's own.
fn window_share(g: &Ghost) -> f32 {
    let p = progress(g);
    ease(if g.kind == Kind::Open { p } else { 1.0 - p }, 0.5, 1.0)
}

/// How far the button a card grows out of has gone (0 to 1): over its first 45 ms, three
/// or four frames.
fn button_gone(g: &Ghost) -> f32 {
    ease(g.flown, 0.0, 0.045)
}

/// Picture `img` at `k` times its size (bilinear, premultiplied), for a button's few pixels.
fn shrunk(img: &crate::win::render::gfx::Image, k: f32) -> crate::win::render::gfx::Image {
    let (w, h) = (((img.w as f32 * k).round() as i32).max(1), ((img.h as f32 * k).round() as i32).max(1));
    let src = img.pixels();
    let at = |x: i32, y: i32| src[(y.clamp(0, img.h - 1) * img.w + x.clamp(0, img.w - 1)) as usize];
    let px = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (sx, sy) = ((x as f32 + 0.5) / k - 0.5, (y as f32 + 0.5) / k - 0.5);
            let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let q = [at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1)];
            let wts = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
            (0..4).fold(0u32, |acc, c| {
                let v: f32 = q.iter().zip(wts).map(|(p, w)| ((p >> (8 * c)) & 0xFF) as f32 * w).sum();
                acc | ((v.round() as u32).min(255) << (8 * c))
            })
        })
        .collect();
    crate::win::render::gfx::Image::from_pixels(w, h, px)
}

/// Draw the card as it is now; returns where its window goes on screen, and how strongly
/// it shows there. The card is drawn whole, what's on it at its strength on the card, and
/// the whole put up at the card's: faded as one, a window's picture over its paper stays
/// the window's pixels. Each drawn at the card's strength instead, the paper showed
/// through the picture, and a card fading out over the window it opened into lightened
/// the window's text by a quarter halfway (dark again only once it had gone).
fn paint(g: &mut Ghost, dark: bool) -> (i32, i32, f32) {
    let s = g.s;
    let Look { card: alpha, from, content } = look(g);
    let on_card = |v: f32| if alpha > 0.002 { (v / alpha).min(1.0) } else { 0.0 };
    let (from_a, content) = (on_card(from), on_card(content));
    let gone = button_gone(g);
    let at_window = window_share(g);
    let c = &mut g.canvas;
    c.clear(0);
    // The window sits on whole pixels where the card is; the card keeps its fraction of one
    // inside it, so the flight stays smooth.
    let pad = g.pad as f32;
    let (wx, wy) = (g.x.value.floor(), g.y.value.floor());
    let (cw, ch) = ((c.w as f32 - 2.0 * pad).max(1.0), (c.h as f32 - 2.0 * pad).max(1.0));
    let sh = Shape::new(pad + g.x.value - wx, pad + g.y.value - wy, g.w.value.clamp(1.0, cw), g.h.value.clamp(1.0, ch), g.r.value.max(0.0));
    // Its shadow: on the way a card's; where it's the window, Windows' for it, as the two
    // hand over. Opening, it goes as the window it reveals comes up with its own, in the
    // same frame (`landed`), rather than the two lying one over the other, darker, for the
    // frames it stays whole.
    let shadow = if g.kind == Kind::Open && g.arrived { 0.0 } else { 1.0 };
    if shadow * at_window > 0.002 {
        window_shadow(c, &sh, s, g.active, at_window, &mut g.shade);
    }
    c.card(&sh, theme::paper(g.color, dark), 1.0, 0x000000, 0.06, 1.6 * shadow * (1.0 - at_window), s);
    c.begin_batch();
    // The deck card it opens from, then the window's picture, both anchored at the card's
    // top-left, as the window lays itself out.
    if let Some(from) = g.from_image.as_ref() {
        c.flush();
        match g.button {
            // Shrinking where the button was, the card growing out of it.
            Some((bx, by)) if from_a > 0.002 => {
                let small = shrunk(from, 1.0 - 0.5 * gone);
                let at_x = bx - (g.origin.0 + wx as i32 - g.pad) as f32 - small.w as f32 / 2.0;
                let at_y = by - (g.origin.1 + wy as i32 - g.pad) as f32 - small.h as f32 / 2.0;
                c.image(&small, at_x.round(), at_y.round(), &sh, from_a);
            }
            Some(_) => {}
            None => c.image(from, sh.x, sh.y, &sh, from_a),
        }
    }
    if let Some(img) = g.image.as_ref() {
        c.flush();
        c.image(img, sh.x, sh.y, &sh, content);
    } else if content > 0.01 && sh.w > 40.0 * s && sh.h > 30.0 * s {
        let (x0, y0) = (sh.x + 18.0 * s, sh.y + 42.0 * s);
        let right = sh.x + sh.w - 16.0 * s;
        let bottom = sh.y + sh.h - 12.0 * s;
        if right > x0 && bottom > y0 {
            let by = if let Some(top) = g.body_top {
                sh.y + top
            } else {
                let tr = rect(x0 as i32, y0 as i32, (right - x0) as i32, (26.0 * s) as i32);
                let clip_title = rect(tr.left, tr.top, tr.right - tr.left, ((bottom - y0) as i32).min(tr.bottom - tr.top));
                c.text(&g.title, &g.title_font, clip_title, theme::PAPER_TEXT, content, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
                y0 + 32.0 * s
            };
            if bottom > by && !g.body.is_empty() {
                let br = rect(x0 as i32, by as i32, (right - x0) as i32, (bottom - by) as i32);
                crate::win::ui::draw_body(c, &g.body, &g.body_font, br, g.color, dark, content, s);
            }
        }
    }
    c.flush();
    // The window's border, which Windows draws over its edge (its picture hasn't it): as
    // much there as the window is.
    if content > 0.002 {
        c.shape(&sh, crate::win::ui::note::border_color(g.color, dark), content, 1.0, 1.0);
    }
    (g.origin.0 + wx as i32 - g.pad, g.origin.1 + wy as i32 - g.pad, alpha)
}

/// Drop every card still in flight (quitting).
pub fn clear() {
    for g in GHOSTS.with(|gs| std::mem::take(&mut *gs.borrow_mut())) {
        unsafe { DestroyWindow(g.hwnd) };
    }
}

/// Frames a card that opened into a window stays whole over it: the frame the window is
/// revealed in, and the one after, as a window that was hidden can take a frame to show
/// (the reveal's work, its paint, is done then too, so it never shortens the fade).
const HOLD: u8 = 2;

/// One frame of a card's flight, `dt` seconds on, on a display refreshing every `period`.
fn advance(g: &mut Ghost, dt: f32, period: f32) -> Step {
    if g.fresh {
        // Put up as the window (or deck card) it takes over from, pixel for pixel, in the
        // frame that one goes: that frame has to be seen before the card moves.
        g.fresh = false;
        return Step::Still;
    }
    if g.hold > 0 {
        g.hold -= 1;
        if g.hold > 0 {
            return Step::Still;
        }
        g.fade.to(0.0);
    }
    // Setting off, from rest: a refresh a frame, whatever the frames before took (the window
    // it opens into made and painted as it set off), so it never sets off with a leap.
    let dt = if g.moves >= START { dt } else { dt.min(period) };
    g.moves = g.moves.saturating_add(1);
    g.flown += dt;
    let mut moving = false;
    for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r, &mut g.fade, &mut g.picture_in] {
        moving |= sp.step(dt);
    }
    if g.arrived {
        return Step::Moved;
    }
    let near = arrive_within(g.kind);
    // Fading in where the window comes up, it's there once it's whole too: revealed under a
    // card at 95%, the window made its last 5% in one frame.
    let whole = !g.appear || g.fade.value >= 0.998;
    if !moving || ([&g.x, &g.y, &g.w, &g.h].iter().all(|sp| (sp.target - sp.value).abs() < near) && whole) {
        g.arrived = true;
        // Exactly at its end: on the window's own place and size, so its picture is the
        // window's pixels where the window is.
        for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r] {
            sp.snap(sp.target);
        }
        match g.kind {
            Kind::Open => {
                g.hold = HOLD;
                if g.appear {
                    g.fade.snap(1.0);
                }
            }
            _ => g.fade.snap(0.0),
        }
        return Step::Arrived;
    }
    Step::Moved
}

/// Whether a card is done: arrived and faded.
fn done(g: &Ghost) -> bool {
    g.arrived && g.hold == 0 && g.fade.settled() && g.fade.value <= 0.01
}

pub fn frame(dt: f32) -> bool {
    let mut arrivals: Vec<Box<dyn FnOnce()>> = Vec::new();
    let alive = GHOSTS.with(|gs| {
        let mut gs = gs.borrow_mut();
        for g in gs.iter_mut() {
            fetch_picture(g);
            match advance(g, dt, anim::period()) {
                Step::Still => {}
                Step::Moved => {
                    render(g);
                    if let Some((taken, up)) = g.lag.take() {
                        let now = crate::win::render::vblank::now();
                        let kind = match g.kind {
                            Kind::Open => "open",
                            Kind::Close => "close",
                            Kind::Dissolve => "dissolve",
                        };
                        let input = anim::last_input().map_or(String::new(), |(at, queued)| format!("; the last input waited {queued}ms in the queue and was taken {:.1}ms before that message", (taken - at) * 1000.0));
                        anim::perf_line(&format!("flight={kind} message->first frame up {:.1}ms, ->first move drawn {:.1}ms, refresh={:.2}ms{input}", (up - taken) * 1000.0, (now - taken) * 1000.0, anim::period() * 1000.0));
                    }
                }
                Step::Arrived => {
                    if let Some(f) = g.on_arrive.take() {
                        // Opening into a window: its frame here, without its shadow, is put
                        // up by the window, painted but kept from the screen, just before it
                        // shows with its own (`landed`). Put up first, as the window was
                        // painted and shown after it, for a frame there was no shadow at all.
                        g.landing = g.kind == Kind::Open && !g.window.is_null();
                        arrivals.push(f);
                    }
                    // Flying home or sinking, it has faded out: it goes with this frame.
                    if !done(g) && !g.landing {
                        render(g);
                    }
                }
            }
        }
        // Retire ghosts that have arrived and faded.
        gs.retain(|g| {
            let done = done(g);
            if done {
                unsafe { DestroyWindow(g.hwnd) };
            }
            !done
        });
        !gs.is_empty()
    });
    for f in arrivals {
        f();
    }
    // A window that didn't show after all (closed meanwhile): the card's frame goes up anyway.
    GHOSTS.with(|gs| {
        for g in gs.borrow_mut().iter_mut().filter(|g| g.landing) {
            g.landing = false;
            render(g);
        }
    });
    alive
}

/// Window `window`, which a card has flown open into, is painted and about to show: the card
/// puts up its frame there, without its shadow, now, so the two go up in the same frame, the
/// window's shadow (Windows') where the card's was.
pub fn landed(window: HWND) {
    GHOSTS.with(|gs| {
        if let Some(g) = gs.borrow_mut().iter_mut().find(|g| g.landing && g.window == window) {
            g.landing = false;
            render(g);
        }
    });
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::render::anim::{Pacer, Timing};
    use crate::win::render::gfx::Image;

    #[test]
    fn a_flight_across_monitors_keeps_a_card_sized_window() {
        // From a capsule at the right edge of a 4K monitor to a note on the one to its left.
        let capsule = RECT { left: 3800, top: 1000, right: 3808, bottom: 1020 };
        let note = RECT { left: -1500, top: 200, right: -1100, bottom: 700 };
        let (w, h) = window_size(&capsule, &note, 48);
        assert_eq!((w, h), (440 + 96, 550 + 96));
        // Four bytes a pixel: about a megabyte, where the whole path would take 50.
        assert!(w * h * 4 < 2_000_000);
    }

    const TAB: RECT = RECT { left: 2236, top: 400, right: 2540, bottom: 560 };
    const NOTE: RECT = RECT { left: 1880, top: 300, right: 2200, bottom: 660 };
    const PLUS: RECT = RECT { left: 2490, top: 900, right: 2524, bottom: 934 };
    const CAPSULE: RECT = RECT { left: 2540, top: 500, right: 2548, bottom: 520 };
    const SLOT: RECT = RECT { left: 2514, top: 400, right: 2560, bottom: 560 };

    /// A picture with something different in every pixel near its neighbours, so one drawn
    /// a pixel off shows.
    fn pattern(w: i32, h: i32, seed: u32) -> (Image, Vec<u32>) {
        let px: Vec<u32> = (0..h as u32)
            .flat_map(|y| (0..w as u32).map(move |x| 0xFF00_0000 | (((x * 37 + y * 11 + seed) & 0xFF) << 16) | (((x ^ y) & 0xFF) << 8) | ((y * 5 + x * 3 + seed) & 0xFF)))
            .collect();
        (Image::from_pixels(w, h, px.clone()), px)
    }

    /// How many pixels inside screen rect `at` (its corners, `r` px each way, left out) the
    /// card's canvas, put up at `win` on screen, shows other than `want`.
    /// A window's edge, 1 px all round, is Windows' border over its picture: `border`, when
    /// it's a window's picture that's wanted.
    fn misses(g: &mut Ghost, win: (i32, i32), at: RECT, want: &[u32], r: i32, border: Option<u32>) -> usize {
        let local = RECT { left: at.left - win.0, top: at.top - win.1, right: at.right - win.0, bottom: at.bottom - win.1 };
        let got = g.canvas.copy_out(local);
        let (w, h) = (at.right - at.left, at.bottom - at.top);
        if got.len() != want.len() {
            return want.len();
        }
        let corner = |x: i32, y: i32| (x < r || x >= w - r) && (y < r || y >= h - r);
        let near = |a: u32, b: u32| (0..4).all(|k| ((a >> (8 * k)) & 0xFF).abs_diff((b >> (8 * k)) & 0xFF) <= 1);
        let miss = |x: i32, y: i32| {
            let px = got[(y * w + x) as usize];
            match border {
                Some(b) if x == 0 || y == 0 || x == w - 1 || y == h - 1 => !near(px, 0xFF00_0000 | b),
                _ => px != want[(y * w + x) as usize],
            }
        };
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| !corner(x, y) && miss(x, y)).count()
    }

    /// One presented frame of a flight: the refresh it shows from, its step, where the card is
    /// on screen and how strongly it shows.
    struct Seen {
        refresh: u32,
        dt: f32,
        edges: [f32; 4],
        look: Look,
        /// How strong its shadow is 8 px below its middle, and 2 px above it.
        shadow: (f32, f32),
        arrived: bool,
    }

    fn seen(g: &mut Ghost, refresh: u32, dt: f32) -> Seen {
        let (ox, oy) = (g.origin.0 as f32, g.origin.1 as f32);
        let edges = [ox + g.x.value, oy + g.y.value, ox + g.x.value + g.w.value, oy + g.y.value + g.h.value];
        // On the canvas as just painted, where the card's top-left is `pad` in.
        let (pad, mid, h) = (g.pad, g.pad + (g.w.value / 2.0) as i32, g.h.value.ceil() as i32);
        let mut at = |x: i32, y: i32| g.canvas.copy_out(RECT { left: x, top: y, right: x + 1, bottom: y + 1 }).first().map_or(0.0, |p| (p >> 24) as f32 / 255.0);
        let shadow = (at(mid, pad + h + 8), at(mid, pad - 3));
        Seen { refresh, dt, edges, look: look(g), shadow, arrived: g.arrived }
    }

    /// What a flight showed, frame by frame, as the message loop drives it at `hz`: put up
    /// late in a refresh (after the window's picture and layout took most of it), then a
    /// frame just after each refresh. With the pixels checked at the hand-offs: the first
    /// frame against `first` at its rect, the arrival against `last`.
    struct Flight {
        frames: Vec<Seen>,
        first_misses: Option<usize>,
        last_misses: Option<usize>,
        /// Opening: the most pixels a frame of the card fading out over the window it
        /// revealed shows other than the window's own.
        fade_misses: usize,
        held: u32,
    }

    /// Opening, as the app does it: the window is made, and its picture drawn, before the
    /// card sets off. `late`: how many refreshes late each of the first ticks comes (the
    /// window just made painting before them), every tick after them on time.
    fn fly_off_screen(kind: Kind, from: (RECT, f32), to: (RECT, f32), image: (Image, Vec<u32>), from_image: Option<(Image, Vec<u32>)>, hz: f64) -> Flight {
        fly_late(kind, from, to, image, from_image, hz, &[])
    }

    fn fly_late(kind: Kind, from: (RECT, f32), to: (RECT, f32), image: (Image, Vec<u32>), from_image: Option<(Image, Vec<u32>)>, hz: f64, late: &[f64]) -> Flight {
        let p = 1.0 / hz;
        let (image, image_px) = image;
        let (from_image, from_px) = from_image.map_or((None, None), |(i, px)| (Some(i), Some(px)));
        let opening = kind == Kind::Open;
        let content = Content { title: "Q4 goals, draft".into(), body: "- Cut p95 latency to 200 ms\n- Ship offline mode".into(), body_top: None, image: Some(image), from_image };
        let mut g = card(null_mut(), kind, from, to, 3, content, 1.0);
        let mut pacer = Pacer::new();
        // `fly` draws the first frame and puts it up at once.
        let (win, _) = at_alpha(paint(&mut g, false));
        // A window's picture has Windows' border over its edge; the deck card's is as it is.
        let border = crate::win::ui::note::border_color(3, false);
        let first = if kind == Kind::Open { from_px.as_deref().map(|px| (from.0, px, from.1.ceil() as i32, None)) } else { Some((from.0, &image_px[..], 8, Some(border))) };
        let first_misses = first.map(|(at, px, r, b)| misses(&mut g, win, at, px, r, b));
        let mut frames = vec![seen(&mut g, 1, 0.0)];
        let (mut last_misses, mut held, mut fade_misses) = (None, 0, 0);
        let mut behind = 0.0;
        for tick in 0..400u32 {
            // The first tick comes right after `fly`, in the same refresh; the rest just after
            // a refresh, as the wait returns, unless they come late.
            behind += late.get(tick as usize).copied().unwrap_or(0.0);
            let t = if tick == 0 { 0.92 * p } else { (tick as f64 + behind + 0.1) * p };
            let dt = pacer.step(t, Some(Timing { period: p, vblank: Some((t / p).floor() * p), display: 1 }));
            let refresh = (t / p).floor() as u32 + 1;
            let dt = if g.moves >= START || g.fresh { dt } else { dt.min(p as f32) };
            match advance(&mut g, dt, p as f32) {
                Step::Still => held += 1,
                Step::Moved => {
                    let (win, alpha) = at_alpha(paint(&mut g, false));
                    if opening && g.arrived {
                        fade_misses = fade_misses.max(over_misses(&mut g, win, to.0, &image_px, 8, alpha));
                    }
                    frames.push(seen(&mut g, refresh, dt));
                }
                Step::Arrived => {
                    if !done(&g) {
                        let (win, _) = at_alpha(paint(&mut g, false));
                        if kind == Kind::Open {
                            last_misses = Some(misses(&mut g, win, to.0, &image_px, 8, Some(border)));
                        }
                        frames.push(seen(&mut g, refresh, dt));
                    }
                }
            }
            if done(&g) {
                break;
            }
        }
        Flight { frames, first_misses, last_misses, fade_misses, held }
    }

    /// Where `paint` put the card's window, and the card's strength.
    fn at_alpha((x, y, a): (i32, i32, f32)) -> ((i32, i32), f32) {
        ((x, y), a)
    }

    /// How many pixels inside screen rect `at` (its corners and 1 px edge left out) the
    /// card, put up at `win` at `alpha` as the compositor puts a layered window up (one
    /// constant alpha over its premultiplied pixels), shows over `want` other than `want`.
    fn over_misses(g: &mut Ghost, win: (i32, i32), at: RECT, want: &[u32], r: i32, alpha: f32) -> usize {
        let local = RECT { left: at.left - win.0, top: at.top - win.1, right: at.right - win.0, bottom: at.bottom - win.1 };
        let got = g.canvas.copy_out(local);
        let (w, h) = (at.right - at.left, at.bottom - at.top);
        let k = (alpha.clamp(0.0, 1.0) * 255.0).round() as u32;
        let corner = |x: i32, y: i32| (x < r || x >= w - r) && (y < r || y >= h - r);
        let edge = |x: i32, y: i32| x == 0 || y == 0 || x == w - 1 || y == h - 1;
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| !corner(x, y) && !edge(x, y))
            .filter(|&(x, y)| {
                let (s, d) = (got[(y * w + x) as usize], want[(y * w + x) as usize]);
                let sa = ((s >> 24) * k + 127) / 255;
                (0..3).any(|c| {
                    let sh = 8 * c;
                    let out = (((s >> sh) & 0xFF) * k + 127) / 255 + (((d >> sh) & 0xFF) * (255 - sa) + 127) / 255;
                    out.abs_diff((d >> sh) & 0xFF) > 1
                })
            })
            .count()
    }

    /// The biggest step between frames (any edge, px), and the biggest spike: a step over
    /// both its neighbours', as a ratio of the bigger of them (1 or less is smooth).
    fn steps(f: &Flight, of: impl Fn(&Seen) -> Vec<f32>) -> (Vec<f32>, f32) {
        let s: Vec<f32> = f.frames.windows(2).map(|w| of(&w[1]).iter().zip(of(&w[0])).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)).collect();
        let spike = s.windows(3).filter(|w| w[1] > 0.25).map(|w| w[1] / w[0].max(w[2]).max(1e-3)).fold(0.0, f32::max);
        (s, spike)
    }

    #[test]
    fn a_card_starts_as_the_window_it_takes_over_and_is_seen_so_before_it_moves() {
        for kind in [Kind::Close, Kind::Dissolve] {
            let to = if kind == Kind::Close { (SLOT, 14.0) } else { (RECT { left: 1896, top: 330, right: 2184, bottom: 651 }, 8.0) };
            let f = fly_off_screen(kind, (NOTE, 8.0), to, pattern(320, 360, 1), None, 99.0);
            assert_eq!(f.first_misses, Some(0), "the first frame is the window's pixels");
            let first = &f.frames[0];
            assert_eq!(first.edges, [1880.0, 300.0, 2200.0, 660.0]);
            assert!((first.look.content - 1.0).abs() < 1e-6 && (first.look.card - 1.0).abs() < 1e-6);
            // Left on screen for a refresh of its own; the next frame is a refresh's step on.
            assert!(f.held >= 1);
            let second = &f.frames[1];
            assert_eq!(second.refresh, first.refresh + 1);
            assert!((second.dt - 1.0 / 99.0).abs() < 1e-6);
        }
    }

    #[test]
    fn a_card_opening_into_a_window_ends_on_its_pixels_and_stays_whole_while_it_comes_up() {
        let f = fly_off_screen(Kind::Open, (TAB, 14.0), (NOTE, 8.0), pattern(320, 360, 2), Some(pattern(304, 160, 3)), 99.0);
        assert_eq!(f.first_misses, Some(0), "the first frame is the deck card's pixels");
        assert_eq!(f.last_misses, Some(0), "the arrival is the window's pixels");
        assert_eq!(f.fade_misses, 0, "fading out over the window, it shows the window's pixels");
        let at = f.frames.iter().position(|s| s.edges == [1880.0, 300.0, 2200.0, 660.0]).expect("arrives exactly");
        assert!((f.frames[at].look.content - 1.0).abs() < 1e-6 && f.frames[at].look.from < 1e-6);
        // Whole for the refresh the window is revealed in and the one after, then fading
        // where it is, never moving.
        assert_eq!(f.frames[at + 1].refresh, f.frames[at].refresh + 2);
        for s in &f.frames[at + 1..] {
            assert_eq!(s.edges, f.frames[at].edges);
            assert!(s.look.card < 1.0);
        }
    }

    #[test]
    fn a_card_opening_moves_in_the_frame_after_the_click_a_refresh_a_frame_from_its_start() {
        for hz in [60.0, 99.0, 144.0] {
            let p = 1.0 / hz;
            // The second and third frames come late (the window just made painting first).
            let f = fly_late(Kind::Open, (TAB, 14.0), (NOTE, 8.0), pattern(320, 360, 2), Some(pattern(304, 160, 3)), hz, &[0.0, 1.6, 1.6]);
            assert_eq!(f.first_misses, Some(0), "it sets off as the deck card");
            // What it leaves is on screen already: it moves at the first frame, in the
            // refresh the click came in, rather than standing a refresh as that card.
            assert_eq!(f.frames[1].refresh, f.frames[0].refresh);
            for s in &f.frames[1..=START as usize] {
                assert!((s.dt as f64 - p).abs() < 1e-6, "{hz} Hz: a refresh a frame at the start, late or not");
            }
            let (moves, _) = steps(&f, |s| s.edges.to_vec());
            // Even from the first step: neither a slow first step nor a leap after it.
            assert!(moves[1] <= 1.25 * moves[0] && moves[0] <= 1.25 * moves[1], "{hz} Hz: {moves:?}");
        }
    }

    #[test]
    fn a_card_appearing_fades_in_as_the_window_and_hands_over_on_its_pixels() {
        for hz in [60.0, 99.0] {
            let p = 1.0 / hz;
            let (image, px) = pattern(320, 360, 4);
            let (dx, dy) = (320 * 3 / 100, 360 * 3 / 100);
            let from = RECT { left: NOTE.left + dx, top: NOTE.top + dy, right: NOTE.right - dx, bottom: NOTE.bottom - dy };
            let content = Content { title: String::new(), body: String::new(), body_top: None, image: Some(image), from_image: None };
            let mut g = card(null_mut(), Kind::Open, (from, 8.0), (NOTE, 8.0), 3, content, 1.0);
            g.appear = true;
            g.fade.snap(0.0);
            g.fade.to(1.0);
            let (_, a0) = at_alpha(paint(&mut g, false));
            assert!(a0 < 0.01, "it starts unseen");
            let (mut alphas, mut moves, mut last) = (Vec::new(), Vec::new(), g.x.value);
            let mut fade_misses = 0;
            for _ in 0..200 {
                match advance(&mut g, p as f32, p as f32) {
                    Step::Still => {}
                    Step::Moved | Step::Arrived => {
                        let (win, alpha) = at_alpha(paint(&mut g, false));
                        // On the card, the window's picture is whole whatever the card's strength.
                        assert!((look(&g).content - alpha).abs() < 1e-6);
                        if g.arrived {
                            fade_misses = fade_misses.max(over_misses(&mut g, win, NOTE, &px, 8, alpha));
                        } else {
                            alphas.push(alpha);
                            moves.push((g.x.value - last).abs());
                            last = g.x.value;
                        }
                    }
                }
                if done(&g) {
                    break;
                }
            }
            assert!(done(&g), "{hz} Hz: it lands and goes");
            assert!(alphas.windows(2).take(6).all(|w| w[1] > w[0]), "{hz} Hz: it fades in from its first frame: {alphas:?}");
            assert!(alphas.windows(2).all(|w| w[1] - w[0] < 0.35), "{hz} Hz: over several frames: {alphas:?}");
            assert!(moves.iter().all(|m| *m < 3.0), "{hz} Hz: it grows the last few pixels: {moves:?}");
            assert_eq!(fade_misses, 0, "{hz} Hz: over the window, it shows the window's pixels");
        }
    }

    #[test]
    fn a_new_note_grows_out_of_the_plus_button_which_goes_in_its_first_few_frames() {
        for hz in [60.0, 99.0, 144.0] {
            let f = fly_off_screen(Kind::Open, (PLUS, 17.0), (NOTE, 8.0), pattern(320, 360, 5), Some(pattern(34, 34, 6)), hz);
            assert_eq!(f.first_misses, Some(0), "{hz} Hz: it sets off as the button");
            // Gone within 45 ms (three to six frames), never riding the card on.
            let gone = f.frames.iter().position(|s| s.look.from < 1e-6).expect("the button goes");
            assert!(gone as f64 <= 0.045 * hz + 1.0, "{hz} Hz: gone at frame {gone}");
            assert!(f.frames[1].look.from > 0.3, "{hz} Hz: not in one frame");
        }
    }

    /// Where a card is the window, its shadow is Windows' own for the window, as measured on
    /// screen (a 320 by 360 note on plain grey, Windows 11), active and inactive: below it,
    /// above it and beside it, within 0.025 (four levels on mid grey) all the way out.
    #[test]
    fn a_card_has_the_windows_shadow_where_it_is_the_window() {
        // How strong the shadow is at `d` px out from the window's edge, below its middle
        // (x 160, or 140 for the inactive one), above it, and beside it (y 180, or 49).
        type Side = (&'static str, &'static [(f32, f32)]);
        let active: [Side; 3] = [
            ("below", &[(0.0, 0.307), (4.0, 0.287), (8.0, 0.260), (16.0, 0.213), (24.0, 0.167), (32.0, 0.120), (48.0, 0.053), (64.0, 0.013), (80.0, 0.0)]),
            ("above", &[(0.0, 0.107), (2.0, 0.093), (4.0, 0.080), (8.0, 0.053), (16.0, 0.020), (24.0, 0.007), (32.0, 0.0)]),
            ("beside", &[(0.0, 0.160), (4.0, 0.133), (8.0, 0.100), (16.0, 0.060), (24.0, 0.027), (32.0, 0.007), (48.0, 0.0)]),
        ];
        let inactive: [Side; 3] = [
            ("below", &[(0.0, 0.223), (4.0, 0.184), (8.0, 0.165), (16.0, 0.126), (24.0, 0.107), (32.0, 0.078), (48.0, 0.039), (80.0, 0.0)]),
            ("above", &[(0.0, 0.062), (2.0, 0.045), (4.0, 0.036), (8.0, 0.018), (16.0, 0.0)]),
            ("beside", &[(0.0, 0.089), (2.0, 0.071), (4.0, 0.054), (8.0, 0.036), (12.0, 0.018), (24.0, 0.0)]),
        ];
        let pad = pad_of(1.0);
        for (is_active, sides, (mx, my)) in [(true, active, (160, 180)), (false, inactive, (140, 49))] {
            let (w, h) = (320 + 2 * pad, 360 + 2 * pad);
            let mut c = Canvas::new(w, h);
            c.clear(0);
            let sh = Shape::new(pad as f32, pad as f32, 320.0, 360.0, 8.0);
            window_shadow(&mut c, &sh, 1.0, is_active, 1.0, &mut Vec::new());
            let px = c.copy_out(RECT { left: 0, top: 0, right: w, bottom: h });
            let alpha = |x: i32, y: i32| (px[(y * w + x) as usize] >> 24) as f32 / 255.0;
            for (side, want) in sides {
                for &(d, a) in want {
                    let d = d as i32;
                    let (x, y) = match side {
                        "below" => (pad + mx, pad + 360 + d),
                        "above" => (pad + mx, pad - 1 - d),
                        _ => (pad - 1 - d, pad + my),
                    };
                    let got = alpha(x, y);
                    assert!((got - a).abs() <= 0.025, "active {is_active}, {side} at {d} px: {got:.3}, Windows' {a:.3}");
                }
            }
            // Under the card's paper, nothing; at the canvas's edge, nothing to cut off.
            assert_eq!(alpha(pad + 160, pad + 180), 0.0);
            assert!((0..w).all(|x| alpha(x, h - 1) == 0.0) && (0..h).all(|y| alpha(0, y) == 0.0));
        }
    }

    /// Opening, the card's last frame on the way has the window's shadow whole, and its
    /// arrival none (the window's own comes up with it); closing, its first frame has the
    /// window's shadow whole, and from halfway on, a card's own.
    #[test]
    fn a_card_hands_its_shadow_over_to_the_window_and_back() {
        let content = || Content { title: String::new(), body: String::new(), body_top: None, image: None, from_image: None };
        let mut g = card(null_mut(), Kind::Open, (TAB, 14.0), (NOTE, 8.0), 3, content(), 1.0);
        let mut shares = Vec::new();
        for _ in 0..400 {
            match advance(&mut g, 1.0 / 99.0, 1.0 / 99.0) {
                Step::Moved if !g.arrived => shares.push(window_share(&g)),
                Step::Arrived => break,
                _ => {}
            }
        }
        assert!(g.arrived && *shares.last().unwrap() > 0.999, "{shares:?}");
        // Gradually: never more than a fifth of the way in a frame.
        assert!(shares.windows(2).all(|w| w[1] - w[0] < 0.2 && w[1] >= w[0] - 1e-3), "{shares:?}");
        let g = card(null_mut(), Kind::Close, (NOTE, 8.0), (SLOT, 14.0), 3, content(), 1.0);
        assert!(window_share(&g) > 0.999);
    }

    #[test]
    fn opening_from_a_deck_card_the_window_comes_in_over_it_never_washing_out() {
        for hz in [60.0, 99.0] {
            let f = fly_off_screen(Kind::Open, (TAB, 14.0), (NOTE, 8.0), pattern(320, 360, 2), Some(pattern(304, 160, 3)), hz);
            let mut mixing = 0;
            for s in f.frames.iter().filter(|s| s.look.card > 0.999) {
                let (from, content) = (s.look.from, s.look.content);
                // The deck card is whole under the window's picture, or covered by it whole:
                // what shows is the one, the other or both together, at full strength.
                assert!(from > 0.999 || content > 0.999, "{hz} Hz: from {from}, content {content}");
                if from > 0.999 && content > 0.01 {
                    mixing += 1;
                }
            }
            // Quickly, but not a cut: for 30 ms or so, not four frames at 60 Hz of both laid
            // half through each other.
            assert!(mixing >= 1 && mixing as f64 / hz < 0.035, "{hz} Hz: {mixing} frames");
        }
    }

    #[test]
    fn a_long_flight_takes_longer_and_never_streaks() {
        // From the capsule at the right edge of a 2,560 px display to a note remembered at
        // its far left.
        let far_note = RECT { left: 80, top: 300, right: 400, bottom: 660 };
        let near = fly_off_screen(Kind::Open, (TAB, 14.0), (NOTE, 8.0), pattern(320, 360, 2), None, 60.0);
        let long = fly_off_screen(Kind::Open, (CAPSULE, 4.0), (far_note, 8.0), pattern(320, 360, 2), None, 60.0);
        let (near_moves, _) = steps(&near, |s| s.edges.to_vec());
        let (long_moves, spike) = steps(&long, |s| s.edges.to_vec());
        let biggest = |v: &[f32]| v.iter().fold(0.0f32, |a, b| a.max(*b));
        // A flight from a deck card to the window beside it is as it was.
        assert!(biggest(&near_moves) < 45.0, "{}", biggest(&near_moves));
        assert!(biggest(&long_moves) < 130.0, "{}", biggest(&long_moves));
        assert!(long.frames.len() > near.frames.len() * 3 / 2);
        // Speeding up and slowing down smoothly, without a leap (the steps under a pixel at
        // the end aside, where it settles).
        let big: Vec<f32> = long_moves.iter().copied().filter(|m| *m > 2.0).collect();
        assert!(big.windows(3).all(|w| w[1] <= 1.6 * w[0].max(w[2])), "{spike} {long_moves:?}");
        // Back home the same way.
        let back = fly_off_screen(Kind::Close, (far_note, 8.0), (CAPSULE, 4.0), pattern(320, 360, 2), None, 60.0);
        let (back_moves, _) = steps(&back, |s| s.edges.to_vec());
        assert!(biggest(&back_moves) < 140.0, "{}", biggest(&back_moves));
    }

    /// Probe (run with --ignored --nocapture): every flight frame by frame at 60, 99, 120
    /// and 144 Hz, off screen: the hand-offs' pixels, each frame's step and opacity, and
    /// what a frame costs to draw (the first eight steps; PROBE_ALL=1 lists them all).
    #[test]
    #[ignore]
    fn probe_flights() {
        // Where a flight starts or ends: a screen rect and its corner radius.
        type End = (RECT, f32);
        let dissolve_to = RECT { left: 1896, top: 330, right: 2184, bottom: 651 };
        let flights: [(&str, Kind, End, End, bool); 6] = [
            ("open from its card", Kind::Open, (TAB, 14.0), (NOTE, 8.0), true),
            ("new note from +", Kind::Open, (PLUS, 17.0), (NOTE, 8.0), false),
            ("open from the pill", Kind::Open, (CAPSULE, 4.0), (NOTE, 8.0), false),
            ("close to its card", Kind::Close, (NOTE, 8.0), (SLOT, 14.0), false),
            ("close to the pill", Kind::Close, (NOTE, 8.0), (CAPSULE, 4.0), false),
            ("archive or delete", Kind::Dissolve, (NOTE, 8.0), (dissolve_to, 8.0), false),
        ];
        for (name, kind, from, to, deck_card) in flights {
            for hz in [60.0, 99.0, 120.0, 144.0] {
                let t0 = std::time::Instant::now();
                let f = fly_off_screen(kind, from, to, pattern(320, 360, 1), deck_card.then(|| pattern(304, 160, 2)), hz);
                let cost = t0.elapsed() / f.frames.len() as u32;
                let (moves, spike) = steps(&f, |s| s.edges.to_vec());
                let (fades, fade_spike) = steps(&f, |s| vec![s.look.card, s.look.from, s.look.content].into_iter().map(|a| a * 100.0).collect());
                let dts: Vec<String> = f.frames.iter().skip(1).map(|s| format!("{:.2}", s.dt as f64 * hz)).collect();
                let off = dts.iter().filter(|d| *d != "1.00").count();
                eprintln!(
                    "PROBE {name} at {hz} Hz: {} frames, {} refreshes held still, {:.2?} a frame; first frame {} pixels off, arrival {}; steps in refreshes all 1: {}",
                    f.frames.len(),
                    f.held,
                    cost,
                    f.first_misses.map_or("-".into(), |m| m.to_string()),
                    f.last_misses.map_or("-".into(), |m| m.to_string()),
                    off == 0
                );
                let fmt = |v: &[f32]| v.iter().take(if std::env::var_os("PROBE_ALL").is_some() { 999 } else { 8 }).map(|x| format!("{x:.1}")).collect::<Vec<_>>().join(" ");
                eprintln!("PROBE   moves (px): {} ... biggest {:.1}, spike {:.2}", fmt(&moves), moves.iter().fold(0.0f32, |a, b| a.max(*b)), spike);
                eprintln!("PROBE   opacity steps (%): {} ... spike {:.2}", fmt(&fades), fade_spike);
                // Its shadow on mid grey (150), in levels, 8 px below it and 3 above, frame by
                // frame while it's whole and on its way (opening, the window's own takes over
                // as it arrives), and Windows' for the window it hands over to.
                let level = |a: f32| a * 150.0;
                let whole: Vec<&Seen> = f.frames.iter().filter(|s| s.look.card > 0.999 && !(kind == Kind::Open && s.arrived)).collect();
                let (below, above): (Vec<f32>, Vec<f32>) = whole.iter().map(|s| (level(s.shadow.0), level(s.shadow.1))).unzip();
                let biggest = |v: &[f32]| v.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
                let window = if kind == Kind::Open { to } else { from };
                let (ww, wh) = (window.0.right - window.0.left, window.0.bottom - window.0.top);
                let pad = pad_of(1.0);
                let mut c = Canvas::new(ww + 2 * pad, wh + 2 * pad);
                c.clear(0);
                window_shadow(&mut c, &Shape::new(pad as f32, pad as f32, ww as f32, wh as f32, window.1), 1.0, true, 1.0, &mut Vec::new());
                let at = |c: &mut Canvas, x: i32, y: i32| level((c.copy_out(RECT { left: x, top: y, right: x + 1, bottom: y + 1 })[0] >> 24) as f32 / 255.0);
                let windows = (at(&mut c, pad + ww / 2, pad + wh + 8), at(&mut c, pad + ww / 2, pad - 3));
                eprintln!(
                    "PROBE   shadow below/above (levels): {} / {} ... biggest step {:.1} / {:.1}; Windows' {:.1} / {:.1}",
                    fmt(&below),
                    fmt(&above),
                    biggest(&below),
                    biggest(&above),
                    windows.0,
                    windows.1
                );
                if kind == Kind::Open {
                    let n = below.len();
                    eprintln!("PROBE   its last frames on the way: {} / {}", fmt(&below[n.saturating_sub(8)..]), fmt(&above[n.saturating_sub(8)..]));
                }
                let first = &f.frames[0];
                let ends: Vec<String> = [first, f.frames.last().unwrap()].iter().map(|s| format!("[{:.1} {:.1} {:.1} {:.1}] card {:.2} from {:.2} content {:.2}", s.edges[0], s.edges[1], s.edges[2], s.edges[3], s.look.card, s.look.from, s.look.content)).collect();
                eprintln!("PROBE   first {}; last {}", ends[0], ends[1]);
            }
        }
    }
}

#[cfg(test)]
mod cost {
    use super::*;
    use crate::win::render::gfx::Image;

    /// Probe (run with --ignored --nocapture): what a frame of a big note's card costs (a
    /// note with a picture, 480 x 700 px), late in its flight from a deck card, the whole and
    /// each part, and put up on a layered window that's never shown.
    #[test]
    #[ignore]
    fn probe_card_costs() {
        let (w, h) = (480, 700);
        let img = Image::from_pixels(w, h, vec![0xFF80_9070; (w * h) as usize]);
        let from_img = Image::from_pixels(304, 160, vec![0xFF70_8090; 304 * 160]);
        let note = RECT { left: 1500, top: 200, right: 1500 + w, bottom: 200 + h };
        let tab = RECT { left: 2236, top: 400, right: 2540, bottom: 560 };
        let content = Content { title: String::new(), body: String::new(), body_top: None, image: Some(img), from_image: Some(from_img) };
        let mut g = card(null_mut(), Kind::Open, (tab, 14.0), (note, 8.0), 3, content, 1.0);
        for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h] {
            sp.snap(sp.target * 0.97 + sp.value * 0.03);
        }
        let n = 50;
        let t = std::time::Instant::now();
        for _ in 0..n { paint(&mut g, false); }
        eprintln!("PROBE paint whole: {:?} canvas {}x{}", t.elapsed() / n, g.canvas.w, g.canvas.h);
        let pad = g.pad as f32;
        let sh = Shape::new(pad, pad, g.w.value, g.h.value, 8.0);
        let c = &mut g.canvas;
        let t = std::time::Instant::now();
        for _ in 0..n { c.clear(0); }
        eprintln!("PROBE clear {:?}", t.elapsed() / n);
        let t = std::time::Instant::now();
        for _ in 0..n { c.card(&sh, 0xF0E0A0, 1.0, 0, 0.06, 1.6, 1.0); }
        eprintln!("PROBE card {:?}", t.elapsed() / n);
        let img2 = Image::from_pixels(w, h, vec![0xFF80_9070; (w * h) as usize]);
        let t = std::time::Instant::now();
        for _ in 0..n { c.image(&img2, sh.x, sh.y, &sh, 0.5); }
        eprintln!("PROBE image half {:?}", t.elapsed() / n);
        let t = std::time::Instant::now();
        for _ in 0..n { c.image(&img2, sh.x, sh.y, &sh, 1.0); }
        eprintln!("PROBE image whole {:?}", t.elapsed() / n);
        let t = std::time::Instant::now();
        for _ in 0..n { c.shape(&sh, 0x808080, 0.7, 1.0, 1.0); }
        eprintln!("PROBE border {:?}", t.elapsed() / n);
        // Put up on a layered window never shown.
        let hw = unsafe { CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, wide("STATIC").as_ptr(), std::ptr::null(), WS_POPUP, -4000, -4000, 10, 10, null_mut(), null_mut(), null_mut(), null_mut()) };
        assert!(!hw.is_null());
        assert!(unsafe { IsWindowVisible(hw) } == 0);
        let t = std::time::Instant::now();
        for i in 0..n { c.present_faded(hw, -4000 + i as i32, -4000, 0.9); }
        eprintln!("PROBE present whole {:?}", t.elapsed() / n);
        unsafe { DestroyWindow(hw) };
    }
}
