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
    /// frame leaves it there to be seen, and the flight moves from the one after.
    fresh: bool,
    /// Frames the card stays whole over the window it has just revealed, which may take a
    /// frame to come up, before it fades.
    hold: u8,
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

/// Fly a card from `from` to `to` (screen rects with corner radii).
pub fn fly(kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, on_arrive: Option<Box<dyn FnOnce()>>) {
    if anim::reduced() {
        if let Some(f) = on_arrive {
            f();
        }
        return;
    }
    // The card shows what the note window draws, so it takes the scale of the monitor the
    // window is on (it may not be the deck's): where it opens, or where it closes from.
    let window = if kind == Kind::Open { &to.0 } else { &from.0 };
    let (_, s) = crate::win::ui::screen::of_rect(window);
    let pad = (48.0 * s) as i32;
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
    let mut g = card(hwnd, kind, from, to, color, content, s);
    g.on_arrive = on_arrive;
    render(&mut g);
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    GHOSTS.with(|gs| gs.borrow_mut().push(g));
    anim::wake(Client::Ghost);
}

/// The card flying from `from` to `to` at scale `s`, drawn in `hwnd`.
fn card(hwnd: HWND, kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, content: Content, s: f32) -> Ghost {
    let pad = (48.0 * s) as i32;
    let b = union(&from.0, &to.0, pad);
    let (ww, wh) = window_size(&from.0, &to.0, pad);
    let params = match kind {
        Kind::Open => (0.86, 0.38),
        Kind::Close => (1.0, 0.3),
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
        fresh: true,
        hold: 0,
    };
    g.x.to(lx(t));
    g.y.to(ly(t));
    g.w.to((t.right - t.left) as f32);
    g.h.to((t.bottom - t.top) as f32);
    g.r.to(to.1);
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
    let (x, y) = paint(g, app::dark());
    g.canvas.present(g.hwnd, x, y);
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

fn look(g: &Ghost) -> Look {
    let p = progress(g);
    let card = match g.kind {
        Kind::Open => g.fade.value,
        Kind::Close => 1.0 - clamp01((p - 0.7) / 0.3),
        Kind::Dissolve => 1.0 - p,
    };
    // Content sits where the note window draws it; the growing card reveals it.
    let content = match (g.kind, g.from_image.is_some() && g.image.is_some()) {
        // From the deck card into the window: the card's words fade before the window's
        // come, so the two never show at once, doubled a little apart (they're laid out
        // differently), and the paper never stands empty for long.
        (Kind::Open, true) => card * clamp01((p - 0.27) / 0.33),
        (Kind::Open, false) => card * clamp01((p - 0.25) / 0.6),
        // Archived or deleted, the note fades as one, words and all: never an empty card.
        (Kind::Dissolve, _) => card,
        (Kind::Close, _) => card * (1.0 - clamp01(p / 0.6)),
    };
    let from = if g.kind == Kind::Open && g.from_image.is_some() { card * (1.0 - clamp01((p - 0.05) / 0.25)) } else { 0.0 };
    Look { card, from, content }
}

/// Draw the card as it is now; returns where its window goes on screen.
fn paint(g: &mut Ghost, dark: bool) -> (i32, i32) {
    let s = g.s;
    let Look { card: alpha, from: from_a, content } = look(g);
    let c = &mut g.canvas;
    c.clear(0);
    // The window sits on whole pixels where the card is; the card keeps its fraction of one
    // inside it, so the flight stays smooth.
    let pad = g.pad as f32;
    let (wx, wy) = (g.x.value.floor(), g.y.value.floor());
    let (cw, ch) = ((c.w as f32 - 2.0 * pad).max(1.0), (c.h as f32 - 2.0 * pad).max(1.0));
    let sh = Shape::new(pad + g.x.value - wx, pad + g.y.value - wy, g.w.value.clamp(1.0, cw), g.h.value.clamp(1.0, ch), g.r.value.max(0.0));
    c.card(&sh, theme::paper(g.color, dark), alpha, 0x000000, 0.06 * alpha, 1.6 * alpha, s);
    c.begin_batch();
    if let Some(img) = g.image.as_ref() {
        // Anchored at the card's top-left, as the window lays itself out.
        c.flush();
        if let Some(from) = g.from_image.as_ref() {
            c.image(from, sh.x, sh.y, &sh, from_a);
        }
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
    (g.origin.0 + wx as i32 - g.pad, g.origin.1 + wy as i32 - g.pad)
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

/// One frame of a card's flight, `dt` seconds on.
fn advance(g: &mut Ghost, dt: f32) -> Step {
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
    let mut moving = false;
    for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r, &mut g.fade] {
        moving |= sp.step(dt);
    }
    if g.arrived {
        return Step::Moved;
    }
    let near = arrive_within(g.kind);
    if !moving || [&g.x, &g.y, &g.w, &g.h].iter().all(|sp| (sp.target - sp.value).abs() < near) {
        g.arrived = true;
        // Exactly at its end: on the window's own place and size, so its picture is the
        // window's pixels where the window is.
        for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r] {
            sp.snap(sp.target);
        }
        match g.kind {
            Kind::Open => g.hold = HOLD,
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
            match advance(g, dt) {
                Step::Still => {}
                Step::Moved => render(g),
                Step::Arrived => {
                    if let Some(f) = g.on_arrive.take() {
                        arrivals.push(f);
                    }
                    // Flying home or sinking, it has faded out: it goes with this frame.
                    if !done(g) {
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
    alive
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::render::anim::{Pacer, Vsync};
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
    fn misses(g: &mut Ghost, win: (i32, i32), at: RECT, want: &[u32], r: i32) -> usize {
        let local = RECT { left: at.left - win.0, top: at.top - win.1, right: at.right - win.0, bottom: at.bottom - win.1 };
        let got = g.canvas.copy_out(local);
        let (w, h) = (at.right - at.left, at.bottom - at.top);
        if got.len() != want.len() {
            return want.len();
        }
        let corner = |x: i32, y: i32| (x < r || x >= w - r) && (y < r || y >= h - r);
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| !corner(x, y) && got[(y * w + x) as usize] != want[(y * w + x) as usize]).count()
    }

    /// One presented frame of a flight: the refresh it shows from, its step, where the card is
    /// on screen and how strongly it shows.
    struct Seen {
        refresh: u32,
        dt: f32,
        edges: [f32; 4],
        look: Look,
    }

    fn seen(g: &Ghost, refresh: u32, dt: f32) -> Seen {
        let (ox, oy) = (g.origin.0 as f32, g.origin.1 as f32);
        let edges = [ox + g.x.value, oy + g.y.value, ox + g.x.value + g.w.value, oy + g.y.value + g.h.value];
        Seen { refresh, dt, edges, look: look(g) }
    }

    /// What a flight showed, frame by frame, as the message loop drives it at `hz`: put up
    /// late in a refresh (after the window's picture and layout took most of it), then a
    /// frame just after each refresh. With the pixels checked at the hand-offs: the first
    /// frame against `first` at its rect, the arrival against `last`.
    struct Flight {
        frames: Vec<Seen>,
        first_misses: Option<usize>,
        last_misses: Option<usize>,
        held: u32,
    }

    fn fly_off_screen(kind: Kind, from: (RECT, f32), to: (RECT, f32), image: (Image, Vec<u32>), from_image: Option<(Image, Vec<u32>)>, hz: f64) -> Flight {
        let p = 1.0 / hz;
        let (image, image_px) = image;
        let (from_image, from_px) = from_image.map_or((None, None), |(i, px)| (Some(i), Some(px)));
        let content = Content { title: "Q4 goals, draft".into(), body: "- Cut p95 latency to 200 ms\n- Ship offline mode".into(), body_top: None, image: Some(image), from_image };
        let mut g = card(null_mut(), kind, from, to, 3, content, 1.0);
        let mut pacer = Pacer::new();
        // `fly` draws the first frame and puts it up at once.
        let win = paint(&mut g, false);
        let first = if kind == Kind::Open { from_px.as_deref().map(|px| (from.0, px, 14)) } else { Some((from.0, &image_px[..], 8)) };
        let first_misses = first.map(|(at, px, r)| misses(&mut g, win, at, px, r));
        let mut frames = vec![seen(&g, 1, 0.0)];
        let (mut last_misses, mut held) = (None, 0);
        for tick in 0..400u32 {
            // The first tick comes right after `fly`, in the same refresh; the rest just after
            // a refresh, as DwmFlush returns.
            let t = if tick == 0 { 0.92 * p } else { (tick as f64 + 0.1) * p };
            let dt = pacer.step(t, Some(Vsync { vblank: (t / p).floor() * p, period: p }));
            let refresh = (t / p).floor() as u32 + 1;
            match advance(&mut g, dt) {
                Step::Still => held += 1,
                Step::Moved => {
                    paint(&mut g, false);
                    frames.push(seen(&g, refresh, dt));
                }
                Step::Arrived => {
                    if !done(&g) {
                        let win = paint(&mut g, false);
                        if kind == Kind::Open {
                            last_misses = Some(misses(&mut g, win, to.0, &image_px, 8));
                        }
                        frames.push(seen(&g, refresh, dt));
                    }
                }
            }
            if done(&g) {
                break;
            }
        }
        Flight { frames, first_misses, last_misses, held }
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
                let first = &f.frames[0];
                let ends: Vec<String> = [first, f.frames.last().unwrap()].iter().map(|s| format!("[{:.1} {:.1} {:.1} {:.1}] card {:.2} from {:.2} content {:.2}", s.edges[0], s.edges[1], s.edges[2], s.edges[3], s.look.card, s.look.from, s.look.content)).collect();
                eprintln!("PROBE   first {}; last {}", ends[0], ends[1]);
            }
        }
    }
}
