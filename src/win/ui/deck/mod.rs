// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The edge deck.
//!
//! Physical model: every note is a paper card tucked into the screen edge; its *spine*
//! (an uppercase label and a perforation) is the tab you see. At rest the deck folds into
//! a translucent pill holding one colour capsule per note. On hover each capsule morphs
//! into its card's spine in a shingled spring cascade; dwelling on a tab pulls the card
//! itself out of the deck to preview it; clicking hands the card to the ghost transition
//! that grows it into the note window.
//!
//! All motion is springs driven by `anim`'s vsync clock, always from current values, so
//! leaving and re-entering mid-animation simply reverses course. Nothing runs when idle.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, DT_CENTER,
    FW_NORMAL, FW_SEMIBOLD,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_APPS, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F10, VK_HOME, VK_MENU,
    VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::Shell::{ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA, SHAppBarMessage};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, Spring, clamp01, lerp};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::common::store::{body_of, title_of};
use crate::win::ui::without_objects;
use crate::win::theme::{self, motion};
use crate::win::util::{WM_MOUSELEAVE, key_down, rect, wide, x_of, y_of};

mod layout;
mod draw;
mod keys;
mod fullscreen;
#[cfg(test)]
mod probe;
use layout::*;
use draw::*;
use keys::{Move, Stop};

const CLASS: &str = "PeekABooDeck";
const T_PREVIEW: usize = 2;
const T_COLLAPSE: usize = 3;
const WM_APPBAR: u32 = WM_APP + 20;
/// Activated without the keys being wanted here: pass the activation on (posted, so it
/// happens after Windows is done activating the deck).
const WM_PASS_ON: u32 = WM_APP + 21;

// Shape tokens (in device-independent pixels; multiplied by the DPI scale).
const PILL_W: f32 = 18.0;
const PILL_INSET: f32 = 5.0; // distance of the pill from the screen edge
const CAP_W: f32 = 8.0;
const CAP_H: f32 = 20.0;
const CAP_GAP: f32 = 5.0;
const TAB_W: f32 = 46.0; // visible depth of a tucked card
const SPINE_W: f32 = 34.0;
const CARD_W: f32 = 304.0;
const CARD_R: f32 = 14.0;
const PREVIEW_MAX: f32 = 300.0; // tallest a pulled-out card grows for its preview
const BULGE: f32 = 14.0;
const LEAN: f32 = -0.022; // radians; cards lean very slightly, like a deck of index cards
const PULL_GAP: f32 = 18.0; // gap between the screen edge and a pulled-out card

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Tab(usize),
    Plus,
    More,
}

struct Tab {
    id: u64,
    title: String,
    label: String, // uppercase spine label
    body: String,
    color: u8,
    /// The note's window is open: its card is out of the deck (no spine, hollow capsule).
    open: bool,
    /// Pinned on top of other windows: a dot in its hollow capsule while it's open.
    pinned: bool,
    /// Position in the spine stack (for open cards: where the spine will return).
    slot: usize,
    reveal: Spring, // 0 = capsule in the pill, 1 = spine on the edge
    delay: f32,     // seconds before `reveal` heads to `reveal_goal` (stagger)
    reveal_goal: f32,
    bulge: Spring,
    lean: Spring,
    y: Spring,
    press: Spring,
    pull: Spring, // 0 = tucked, 1 = pulled out (preview)
    /// Cached spine label layout: (max length, fitted text, text length, cell height).
    fitted: Option<(i32, String, i32, i32)>,
    /// The spine label drawn (see `draw::label_sprite`), let go with `fitted`.
    sprites: Vec<Sprite>,
    /// The preview body's laid-out height at the current scale and typography, measured
    /// the first time the card is pulled out (see `begin_preview`).
    body_h: Option<f32>,
}

impl Tab {
    fn new(id: u64, y: f32) -> Tab {
        Tab {
            id,
            title: String::new(),
            label: String::new(),
            body: String::new(),
            color: 0,
            open: false,
            pinned: false,
            reveal: Spring::unit(0.0, motion::SHINGLE),
            delay: 0.0,
            reveal_goal: 0.0,
            bulge: Spring::px(0.0, motion::SNAPPY),
            lean: Spring::unit(LEAN, motion::DEFAULT),
            y: Spring::px(y, motion::DEFAULT),
            press: Spring::unit(1.0, motion::SNAPPY),
            pull: Spring::unit(0.0, motion::DEFAULT),
            fitted: None,
            sprites: Vec::new(),
            body_h: None,
            slot: 0,
        }
    }

    /// The label's text, size or font changed: measure and draw it again.
    fn relabel(&mut self) {
        self.fitted = None;
        self.sprites.clear();
    }

    fn springs(&mut self) -> [&mut Spring; 6] {
        [&mut self.reveal, &mut self.bulge, &mut self.lean, &mut self.y, &mut self.press, &mut self.pull]
    }
}

struct Drag {
    idx: usize,
    grab: f32,
    start_y: i32,
    active: bool,
    samples: Vec<(std::time::Instant, f32)>,
}

#[derive(Clone, Copy, Default)]
struct Geo {
    s: f32,
    left: bool,
    work: RECT,
    centre: i32, // screen y of the deck's vertical centre
    // Expanded window (screen).
    ex: i32,
    ey: i32,
    ew: i32,
    eh: i32,
    // Collapsed window (screen).
    cx: i32,
    cy: i32,
    cw: i32,
    ch: i32,
    tab_h: f32,
    step: f32,
    top: f32, // first slot y in expanded-window coordinates
    n: usize,
    more: usize,
    /// Capsules in the pill (every card in the deck, including those that are out).
    caps: usize,
    /// Height of what sits under the stack: the "+N" tile, the plus button, margins.
    tail: f32,
}

impl Geo {
    fn slot_y(&self, i: usize) -> f32 {
        self.top + i as f32 * self.step
    }
    fn pill_h(&self) -> f32 {
        let n = self.caps.max(1) as f32;
        (n * CAP_H + (n - 1.0) * CAP_GAP + 2.0 * (PILL_W - CAP_W) / 2.0) * self.s
    }
    /// Pill top in screen coordinates.
    fn pill_top(&self) -> f32 {
        self.centre as f32 - self.pill_h() / 2.0
    }
    fn more_y(&self) -> f32 {
        self.slot_y(self.n) + 8.0 * self.s
    }
    fn plus_cy(&self) -> f32 {
        let base = if self.more > 0 { self.more_y() + 26.0 * self.s + 10.0 * self.s } else { self.slot_y(self.n) + 8.0 * self.s };
        base + 17.0 * self.s
    }
}

struct Deck {
    hwnd: HWND,
    tabs: Vec<Tab>,
    open: bool,
    wide: bool, // window currently uses the expanded geometry
    previewing: bool,
    pill: Spring,
    extras: Spring, // plus button and "+N" tile
    extras_delay: f32,
    extras_goal: f32,
    plus_hot: Spring,
    more_hot: Spring,
    hover: Hit,
    pressed: Hit,
    tracking: bool,
    /// A context menu from the deck is up: don't collapse or preview underneath it.
    menu: bool,
    drag: Option<Drag>,
    fullscreen: bool,
    visible: bool,
    /// Hidden, and its pill still fading out: the window goes once it has.
    hiding: bool,
    geo: Geo,
    fonts: HashMap<(u8, i32, i32, u8), Font>,
    font_scale: f32,
    canvas: Option<Canvas>,
    /// The hovered tab whose spine label is too long scrolls.
    marquee: Option<Marquee>,
    /// A label scrolled in the last frame, so the frame clock keeps running.
    marquee_live: bool,
    /// The deck has the keys (Ctrl+Alt+K) until Esc, or until they go elsewhere.
    kbd: Option<Kbd>,
    /// Hidden (by the user, or for a full-screen window), it's out only while it has the
    /// keys: Ctrl+Alt+K brought it.
    summoned: bool,
    /// Where the pointer was when a note opened from the deck: until it moves, the cards
    /// sliding up under it don't count as pointed at.
    still: Option<POINT>,
}

struct Kbd {
    /// What wears the focus ring.
    at: Option<Stop>,
    /// The window that had the keys before the deck took them, to give them back to.
    prev: HWND,
}

thread_local! {
    static DECK: RefCell<Option<Deck>> = const { RefCell::new(None) };
    /// The window the deck last took the keys from, after it let them go: where they go
    /// if Windows hands the deck the activation again (a note opened from it closing).
    static PASS_TO: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

/// Let the label fonts go (with no window open): each keeps DirectWrite state alive. The
/// labels drawn go too.
pub fn trim() {
    with(|d| {
        d.fonts.clear();
        for t in d.tabs.iter_mut() {
            t.sprites = Vec::new();
        }
    });
}

/// Whether the deck is folded and still: its fonts aren't wanted for now.
pub fn resting() -> bool {
    with(|d| !d.open && !d.wide).unwrap_or(true)
}

fn with<R>(f: impl FnOnce(&mut Deck) -> R) -> Option<R> {
    DECK.with(|d| d.try_borrow_mut().ok().and_then(|mut d| d.as_mut().map(f)))
}

/// The deck's display: (work area, DPI scale). The one chosen in Settings, or the main
/// display while that one isn't connected; worked out once per display change.
pub fn monitor_geo() -> (RECT, f32) {
    crate::win::ui::displays::deck()
}

/// The deck is going to another display, or the other edge: fold it at once and fade the
/// pill in there, rather than have it blink from one place to the other (with Reduce
/// Motion too: it moves nothing).
pub fn arrive() {
    with(|d| {
        set_open(d, false, true);
        d.pill.snap(0.0);
        d.pill.to(1.0);
        anim::wake(Client::Deck);
    });
}

/// Whether `h` is the deck while it has the keys (Ctrl+Alt+K): the window they came from
/// isn't left for good, as they go back to it on Esc.
pub fn holds_keys(h: HWND) -> bool {
    !h.is_null() && with(|d| d.hwnd == h && d.kbd.is_some()).unwrap_or(false)
}

/// Look again whether a full-screen window covers the deck's display, and step aside for
/// it (or come back) accordingly.
fn check_fullscreen(d: &mut Deck) {
    let fs = fullscreen::on(&monitor_geo().0);
    if fs != d.fullscreen {
        d.fullscreen = fs;
        update_visibility(d);
        render(d);
    }
}

pub fn create() {
    app::register_class(CLASS, Some(wndproc), IDC_HAND);
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    let hwnd = unsafe {
        CreateWindowExW(ex, wide(CLASS).as_ptr(), wide(t!("app_name")).as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), app::hinst(), null_mut())
    };
    crate::win::util::no_transitions(hwnd);
    DECK.with(|d| {
        *d.borrow_mut() = Some(Deck {
            hwnd,
            tabs: Vec::new(),
            open: false,
            wide: false,
            previewing: false,
            pill: Spring::fade(1.0, motion::DEFAULT),
            extras: Spring::unit(0.0, motion::SHINGLE),
            extras_delay: 0.0,
            extras_goal: 0.0,
            plus_hot: Spring::unit(0.0, motion::SNAPPY),
            more_hot: Spring::unit(0.0, motion::SNAPPY),
            hover: Hit::None,
            pressed: Hit::None,
            tracking: false,
            menu: false,
            drag: None,
            fullscreen: false,
            visible: false,
            hiding: false,
            geo: Geo::default(),
            fonts: HashMap::new(),
            font_scale: 0.0,
            canvas: None,
            marquee: None,
            marquee_live: false,
            kbd: None,
            summoned: false,
            still: None,
        })
    });
    // As an app bar (without reserving space) we get ABN_FULLSCREENAPP for free.
    unsafe {
        let mut abd: APPBARDATA = std::mem::zeroed();
        abd.cbSize = size_of::<APPBARDATA>() as u32;
        abd.hWnd = hwnd;
        abd.uCallbackMessage = WM_APPBAR;
        SHAppBarMessage(ABM_NEW, &mut abd);
    }
    refresh();
}

pub fn shutdown() {
    // Taken out first, so no frame still in flight draws it back; its canvas and fonts go with it.
    if let Some(d) = DECK.with(|d| d.borrow_mut().take()) {
        unsafe {
            let mut abd: APPBARDATA = std::mem::zeroed();
            abd.cbSize = size_of::<APPBARDATA>() as u32;
            abd.hWnd = d.hwnd;
            SHAppBarMessage(ABM_REMOVE, &mut abd);
            DestroyWindow(d.hwnd);
        }
    }
}

/// Notes or settings changed: resync and redraw.
pub fn refresh() {
    with(|d| {
        let was = d.geo.work;
        update_visibility(d);
        sync(d);
        // At start, or on another display now: is a full-screen window covering this one?
        let now = d.geo.work;
        if (was.left, was.top, was.right, was.bottom) != (now.left, now.top, now.right, now.bottom) {
            check_fullscreen(d);
        }
        render(d);
    });
}

/// Note `id` was typed in. Nothing but its card's title and preview can have changed, so
/// only that card is brought up to date, and the deck is drawn again only when the card
/// reads differently (a pause in typing otherwise costs the deck nothing).
pub fn note_edited(id: u64) {
    with(|d| {
        let Some(i) = d.tabs.iter().position(|t| t.id == id) else { return };
        let (title, body) = tab_texts(id);
        let t = &mut d.tabs[i];
        if t.title == title && t.body == body {
            return;
        }
        let label = title.to_uppercase();
        if label != t.label {
            t.relabel();
        }
        t.label = label;
        t.title = title;
        t.body = body;
        t.body_h = None;
        if t.pull.target > 0.0 || t.pull.value > 0.0 {
            measure_body(d, i);
        }
        render(d);
        anim::wake(Client::Deck);
    });
}

pub fn frame(dt: f32) -> bool {
    let (more, folded) = with(|d| {
        let (moving, folded, draw) = advance(d, dt);
        if draw {
            render(d);
        }
        repoint(d);
        (moving || d.marquee_live, folded)
    })
    .unwrap_or((false, false));
    if folded {
        // What the cards' text took (Direct2D, fonts, layouts) goes back to Windows once
        // the deck has rested folded for a moment, as it does when a window closes.
        app::schedule_trim();
    }
    more
}

/// One frame's motion: (still moving, just folded, anything to draw).
fn advance(d: &mut Deck, dt: f32) -> (bool, bool, bool) {
    let moving = step(d, dt);
    if d.hiding && (!moving || d.pill.value < 0.01) {
        d.hiding = false;
        unsafe { ShowWindow(d.hwnd, SW_HIDE) };
        return (false, false, false);
    }
    let folded = !d.open && !moving && d.wide;
    if folded {
        d.wide = false; // fully folded: shrink back to the small pill window
    }
    // Nothing moves but a scrolling label, and it hasn't come a pixel further: the frame
    // would be the last one again (it moves 34 px a second, and rests at its ends).
    if !moving
        && let Some(m) = d.marquee.as_ref()
        && let (at, true) = m.now(d.geo.s)
        && at == m.drawn
    {
        return (false, folded, false);
    }
    d.marquee_live = false;
    (moving, folded, true)
}

/// Fan the deck out, with what its frames will draw made first (see `draw::warm`).
fn fan_out(d: &mut Deck) {
    let cold = !d.wide;
    set_open(d, true, false);
    if cold {
        warm(d);
    }
}

fn update_visibility(d: &mut Deck) {
    let (hidden, over_fs) = app::store(|s| (s.d.settings.deck_hidden, s.d.settings.over_fullscreen));
    let show = (!hidden && (!d.fullscreen || over_fs)) || d.summoned;
    if show != d.visible {
        d.visible = show;
        if !show {
            // Hiding the window hands its keys to whatever Windows picks next.
            drop_keys(d);
        }
        set_open(d, false, true);
        // The pill fades in, or out before the window goes, rather than blinking (at
        // start, shown or hidden from the tray, or stepping aside for a full-screen app).
        d.hiding = !show && !d.hwnd.is_null() && unsafe { IsWindowVisible(d.hwnd) } != 0;
        if show {
            d.pill.snap(0.0);
        }
        d.pill.to(if show { 1.0 } else { 0.0 });
        anim::wake(Client::Deck);
    }
}

/// Where tab `i` is drawn right now, in window-local coordinates: (shape, spine x, scale).
struct CardPose {
    shape: Shape,
    spine_x: f32, // local x of the spine's inner edge (the card side away from the screen edge)
    pull: f32,
    reveal: f32,
}

/// Screen rectangle and corner radius of note `id`'s card right now (for the ghost).
pub fn card_rect(id: u64) -> Option<(RECT, f32)> {
    with(|d| {
        if !d.wide {
            return None;
        }
        let i = d.tabs.iter().position(|t| t.id == id && !t.open)?;
        let g = d.geo;
        let cp = pose(d, i, g.ew, g.ey as f32);
        let sh = cp.shape;
        let x0 = sh.x.max(0.0);
        let x1 = (sh.x + sh.w).min(g.ew as f32);
        Some((
            RECT { left: g.ex + x0 as i32, top: g.ey + sh.y as i32, right: g.ex + x1 as i32, bottom: g.ey + (sh.y + sh.h) as i32 },
            CARD_R * g.s,
        ))
    })
    .flatten()
}

/// Note `id`'s pulled-out card as it is on screen right now, for the card that flies
/// open from it to start as exactly this.
pub fn card_image(id: u64) -> Option<crate::win::render::gfx::Image> {
    let (r, _) = card_rect(id)?;
    with(|d| {
        let mut w: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(d.hwnd, &mut w) };
        let local = RECT { left: r.left - w.left, top: r.top - w.top, right: r.right - w.left, bottom: r.bottom - w.top };
        d.canvas.as_mut()?.snapshot(local)
    })
    .flatten()
}

/// The plus button's screen rect, if the deck is fanned out (new notes grow from it).
pub fn plus_rect() -> Option<(RECT, f32)> {
    with(|d| {
        if !d.wide || !d.open {
            return None;
        }
        let g = d.geo;
        let s = g.s;
        let centre_near = (TAB_W / 2.0 + 2.0) * s;
        let (cx, _) = span(&g, g.ew, centre_near, centre_near);
        let (r, cy) = (17.0 * s, g.plus_cy() + footer_drop(d));
        let rc = RECT { left: g.ex + (cx - r) as i32, top: g.ey + (cy - r) as i32, right: g.ex + (cx + r) as i32, bottom: g.ey + (cy + r) as i32 };
        Some((rc, r))
    })
    .flatten()
}

/// Whether the deck is on screen with a card for note `id`: somewhere for a closing note
/// to fly back to. Not while the deck is hidden (by the user or for a fullscreen app), nor
/// for a note kept out of it.
pub fn holds(id: u64) -> bool {
    with(|d| d.visible && d.tabs.iter().any(|t| t.id == id)).unwrap_or(false)
}

/// Where a closing note should fly back to: its slot in the fanned-out stack, else its
/// capsule in the pill.
pub fn home_rect(id: u64) -> (RECT, f32) {
    let slot = with(|d| {
        if !(d.open && d.wide) {
            return None;
        }
        let t = d.tabs.iter().find(|t| t.id == id)?;
        let g = d.geo;
        let s = g.s;
        let vis = (TAB_W * s) as i32;
        let top = g.ey + g.slot_y(t.slot) as i32;
        let (l, r) = if g.left { (g.work.left, g.work.left + vis) } else { (g.work.right - vis, g.work.right) };
        Some((RECT { left: l, top, right: r, bottom: top + g.tab_h as i32 }, CARD_R * s))
    })
    .flatten();
    if let Some(r) = slot {
        return r;
    }
    let fallback = || {
        let (work, s) = monitor_geo();
        let y = (work.top + work.bottom) / 2;
        let x = work.right - (PILL_INSET * s) as i32 - (CAP_W * s) as i32;
        (RECT { left: x, top: y, right: x + (CAP_W * s) as i32, bottom: y + (CAP_H * s) as i32 }, CAP_W * s / 2.0)
    };
    with(|d| {
        let g = d.geo;
        let s = g.s;
        let i = d.tabs.iter().position(|t| t.id == id).unwrap_or(0);
        let top = g.pill_top() + ((PILL_W - CAP_W) / 2.0 + i as f32 * (CAP_H + CAP_GAP)) * s;
        let near = (PILL_INSET + (PILL_W - CAP_W) / 2.0) * s;
        let (l, r) = if g.left {
            (g.work.left + near as i32, g.work.left + (near + CAP_W * s) as i32)
        } else {
            (g.work.right - (near + CAP_W * s) as i32, g.work.right - near as i32)
        };
        (RECT { left: l, top: top as i32, right: r, bottom: (top + CAP_H * s) as i32 }, CAP_W * s / 2.0)
    })
    .unwrap_or_else(fallback)
}

/// Where a note window should appear: (edge x, centre y, scale, work area, left edge?).
pub fn anchor(id: Option<u64>) -> (i32, i32, f32, RECT, bool) {
    let r = id.and_then(card_rect);
    with(|d| {
        let g = d.geo;
        let s = g.s;
        let reach = ((TAB_W + 26.0) * s) as i32;
        let ex = if g.left { g.work.left + reach } else { g.work.right - reach };
        let cy = r.map(|(r, _)| r.top + (60.0 * s) as i32).unwrap_or(g.centre - (120.0 * s) as i32);
        (ex, cy, s, g.work, g.left)
    })
    .unwrap_or_else(|| {
        let (work, s) = monitor_geo();
        (work.right - 80, (work.top + work.bottom) / 2, s, work, false)
    })
}

/// A tab's card was handed to the ghost: tuck it away without animation.
pub fn hand_off(id: u64) {
    with(|d| {
        if let Some(t) = d.tabs.iter_mut().find(|t| t.id == id) {
            t.pull.snap(0.0);
            t.bulge.snap(0.0);
        }
        d.previewing = false;
        anim::wake(Client::Deck);
    });
}

enum Action {
    None,
    Open(u64),
    New,
    More,
    /// A card's menu; from the keyboard, beside the card at this screen height rather
    /// than at the pointer.
    TabMenu(u64, Option<POINT>),
    /// The deck's own menu; from the keyboard, beside the button the ring is on.
    DeckMenu(Option<POINT>),
    Reorder(Vec<u64>),
    /// Esc: the keys go back to the window that had them.
    GiveBack,
}

/// Everything the focus ring can reach, top to bottom: the cards in the deck (not those
/// out as open notes, which have no spine), the "+N" tile, the plus button.
fn stops(d: &Deck) -> Vec<Stop> {
    let mut v: Vec<Stop> = d.tabs.iter().filter(|t| !t.open).map(|t| Stop::Card(t.id)).collect();
    if d.geo.more > 0 {
        v.push(Stop::More);
    }
    v.push(Stop::Plus);
    v
}

/// What the focus ring is on, as the pointer would hit it.
fn ring_hit(d: &Deck) -> Option<Hit> {
    Some(match d.kbd.as_ref()?.at? {
        Stop::Card(id) => Hit::Tab(d.tabs.iter().position(|t| t.id == id && !t.open)?),
        Stop::More => Hit::More,
        Stop::Plus => Hit::Plus,
    })
}

/// The ring moved: what it's on lights up as it does under the pointer, and a card comes
/// out to be read after the same rest (at once while another is out, as when the pointer
/// scrubs along the deck).
fn show_ring(d: &mut Deck) {
    let hit = ring_hit(d).unwrap_or(Hit::None);
    d.hover = hit;
    unsafe { KillTimer(d.hwnd, T_PREVIEW) };
    match hit {
        Hit::Tab(i) if d.previewing => begin_preview(d, i),
        Hit::Tab(_) => unsafe {
            SetTimer(d.hwnd, T_PREVIEW, motion::PREVIEW_DWELL_MS, None);
        },
        _ if d.previewing => end_preview(d),
        _ => {}
    }
    tip(d, hit, true);
    anim::wake(Client::Deck);
}

/// Ctrl+Alt+K: the deck takes the keys, fanned out with the focus ring on its top card.
/// Pressed again while the deck has them, it gives them back. A hidden deck (by the user,
/// or for a full-screen app) comes out for as long as it has them, and hides again after.
/// A note in front of everything is being closed or archived: once Windows has decided
/// who comes next, if that's the desktop or the deck, the keys go back to the window the
/// deck took them from (when it did).
pub fn pass_on_later() {
    if let Some(h) = with(|d| d.hwnd) {
        unsafe { PostMessageW(h, WM_PASS_ON, 1, 0) };
    }
}

/// Whether `h` is the desktop (Explorer's window behind the icons).
fn is_desktop(h: HWND) -> bool {
    let mut buf = [0u16; 16];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..n]);
    class == "Progman" || class == "WorkerW" || h == unsafe { GetShellWindow() }
}

pub fn take_keys() {
    let Some((hwnd, has)) = with(|d| (d.hwnd, d.kbd.is_some())) else { return };
    if has {
        give_keys_back(hwnd);
        return;
    }
    let prev = unsafe { GetForegroundWindow() };
    with(|d| {
        if !d.visible {
            d.summoned = true;
            update_visibility(d);
        }
        unsafe { KillTimer(hwnd, T_COLLAPSE) };
        fan_out(d);
        d.kbd = Some(Kbd { at: stops(d).first().copied(), prev });
        show_ring(d);
        render(d);
    });
    // Outside the deck's borrow: its activation comes straight back to it. The deck never
    // activates on a click, but a hotkey lets it take the foreground; should Windows
    // refuse anyway, there's no ring without the keys.
    if unsafe { SetForegroundWindow(hwnd) } == 0 {
        with(drop_keys);
    }
}

/// The deck lets the keys go: the ring goes, and the deck folds after the usual grace
/// unless the pointer is on it. Returns the window that had the keys before.
fn drop_keys(d: &mut Deck) -> Option<HWND> {
    let k = d.kbd.take()?;
    PASS_TO.set(k.prev as isize);
    d.hover = Hit::None;
    unsafe {
        KillTimer(d.hwnd, T_PREVIEW);
        SetTimer(d.hwnd, T_COLLAPSE, motion::COLLAPSE_GRACE_MS, None);
    }
    if d.previewing {
        end_preview(d);
    }
    crate::win::ui::tooltip::leave(d.hwnd);
    render(d);
    anim::wake(Client::Deck);
    if std::mem::take(&mut d.summoned) {
        // Out only for the keys: hidden again.
        update_visibility(d);
    }
    Some(k.prev)
}

/// Esc (or Ctrl+Alt+K again): the keys go back to the window that had them, or to the
/// desktop if it has gone since.
fn give_keys_back(hwnd: HWND) {
    let Some(prev) = with(drop_keys).flatten() else { return };
    unsafe {
        let back = if !prev.is_null() && prev != hwnd && IsWindow(prev) != 0 && IsWindowVisible(prev) != 0 { prev } else { GetShellWindow() };
        if !back.is_null() {
            SetForegroundWindow(back);
        }
    }
}

/// A key while the deck has the keys: the arrows, Home and End, Tab and Shift+Tab move
/// the ring, Enter or Space presses what it's on, Shift+F10 or the Menu key opens its
/// menu, Esc hands the keys back. None: not one of ours (Alt+F4 and the like go on to
/// Windows).
fn on_key(d: &mut Deck, vk: u16) -> Option<Action> {
    let at = d.kbd.as_ref()?.at;
    let (shift, ctrl, alt) = (key_down(VK_SHIFT), key_down(VK_CONTROL), key_down(VK_MENU));
    if vk == VK_APPS || (shift && !ctrl && vk == VK_F10) {
        let hit = ring_hit(d).unwrap_or(Hit::None);
        return Some(menu_for(d, hit, true));
    }
    if ctrl || alt {
        return None;
    }
    let key = match vk {
        VK_UP => Move::Up,
        VK_DOWN => Move::Down,
        VK_HOME => Move::Home,
        VK_END => Move::End,
        VK_TAB if shift => Move::Prev,
        VK_TAB => Move::Next,
        VK_RETURN | VK_SPACE => {
            return Some(match ring_hit(d) {
                Some(Hit::Tab(i)) => Action::Open(d.tabs[i].id),
                Some(Hit::More) => Action::More,
                Some(Hit::Plus) => Action::New,
                _ => Action::None,
            });
        }
        VK_ESCAPE => return Some(Action::GiveBack),
        _ => return None,
    };
    let to = keys::step(&stops(d), at, key);
    ring_to(d, to);
    Some(Action::None)
}

/// A typed letter: the ring goes to the next card whose title starts with it.
fn on_char(d: &mut Deck, ch: char) {
    let Some(at) = d.kbd.as_ref().map(|k| k.at) else { return };
    let to = {
        let cards: Vec<(Stop, &str)> = d.tabs.iter().filter(|t| !t.open).map(|t| (Stop::Card(t.id), t.title.as_str())).collect();
        keys::by_letter(&cards, at, ch)
    };
    if to.is_some() {
        ring_to(d, to);
    }
}

fn ring_to(d: &mut Deck, to: Option<Stop>) {
    let Some(k) = d.kbd.as_mut() else { return };
    if k.at != to {
        k.at = to;
        show_ring(d);
        render(d);
    }
}

/// A menu for what the pointer (or, `by_key`, the focus ring) is on: a card's own, or
/// the deck's. The menu takes the stage: any pulled-out card tucks away at once, but the
/// deck stays open and the card the menu is for comes (or stays) out, so it's clear what
/// the menu acts on.
fn menu_for(d: &mut Deck, hit: Hit, by_key: bool) -> Action {
    crate::win::ui::tooltip::mute();
    unsafe {
        KillTimer(d.hwnd, T_PREVIEW);
        KillTimer(d.hwnd, T_COLLAPSE);
    }
    if let Hit::Tab(i) = hit {
        begin_preview(d, i);
    } else {
        for t in d.tabs.iter_mut() {
            t.pull.snap(0.0);
        }
        d.previewing = false;
    }
    d.menu = true;
    d.hover = if let Hit::Tab(_) = hit { hit } else { Hit::None };
    anim::wake(Client::Deck);
    render(d);
    // From the keyboard there's no pointer to open at: beside the card near its top, or
    // beside the button the ring is on.
    let g = d.geo;
    match hit {
        Hit::Tab(i) => Action::TabMenu(d.tabs[i].id, by_key.then(|| POINT { x: 0, y: g.ey + (d.tabs[i].y.value + 14.0 * g.s) as i32 })),
        _ => {
            let beside = footer_rect(d, hit).map(|r| POINT { x: g.ex + if g.left { r.right } else { r.left }, y: g.ey + (r.top + r.bottom) / 2 });
            Action::DeckMenu(if by_key { beside } else { None })
        }
    }
}

fn begin_preview(d: &mut Deck, i: usize) {
    measure_body(d, i);
    for (j, t) in d.tabs.iter_mut().enumerate() {
        t.pull.to(if j == i { 1.0 } else { 0.0 });
    }
    d.previewing = true;
    anim::wake(Client::Deck);
}

fn end_preview(d: &mut Deck) {
    for t in d.tabs.iter_mut() {
        t.pull.to(0.0);
    }
    d.previewing = false;
    anim::wake(Client::Deck);
}

/// The pointer is over `hit` now: it lights up, and a card comes out after a rest there (at
/// once while another is out, as the pointer scrubs along the deck).
fn point_at(d: &mut Deck, hit: Hit) {
    if hit == d.hover {
        return;
    }
    d.hover = hit;
    // While the deck has the keys, the ring goes where the pointer lights, so Enter opens
    // what's shown.
    let stop = match hit {
        Hit::Tab(i) => Some(Stop::Card(d.tabs[i].id)),
        Hit::More => Some(Stop::More),
        Hit::Plus => Some(Stop::Plus),
        Hit::None => None,
    };
    if let (Some(k), Some(stop)) = (d.kbd.as_mut(), stop) {
        k.at = Some(stop);
    }
    unsafe { KillTimer(d.hwnd, T_PREVIEW) };
    match hit {
        Hit::Tab(i) if d.previewing => begin_preview(d, i),
        Hit::Tab(_) => unsafe {
            SetTimer(d.hwnd, T_PREVIEW, motion::PREVIEW_DWELL_MS, None);
        },
        _ => {
            if d.previewing {
                end_preview(d);
            }
        }
    }
    anim::wake(Client::Deck);
}

/// A note opens from the deck: the cards close up behind its card, but what slides under
/// the pointer is not pointed at until the pointer moves.
fn hold_still() {
    let mut at = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut at) };
    with(|d| {
        d.still = Some(at);
        point_at(d, Hit::None);
    });
}

/// The cards moved under a pointer that didn't (fanning out, making room, tucking back):
/// what it's over now is what it points at, as if it had moved, so resting on a card that
/// slid under it brings that card out. Not while dragging, a menu is up or the deck has
/// the keys, nor while the pointer stays where it was when a note opened from here.
fn repoint(d: &mut Deck) {
    if !(d.open && d.wide) || d.drag.is_some() || d.menu || d.kbd.is_some() {
        return;
    }
    let mut at = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut at);
        GetWindowRect(d.hwnd, &mut r);
    }
    if !crate::win::util::pt_in(&r, at.x, at.y) || d.still.is_some_and(|p| (p.x, p.y) == (at.x, at.y)) {
        return;
    }
    let hit = hit_test(d, at.x - r.left, at.y - r.top);
    if hit != d.hover {
        point_at(d, hit);
        tip(d, hit, false);
    }
}

/// While a card's menu is up: has the pointer moved on to another card (or the + / more
/// buttons)? Then the menu and the pulled-out card go away together, and the deck is
/// live again under the pointer for whatever it reaches next.
fn leave_menu(hwnd: HWND, id: u64, pt: POINT) -> bool {
    with(|d| {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut r) };
        if !crate::win::util::pt_in(&r, pt.x, pt.y) {
            return false;
        }
        let gone = match hit_test(d, pt.x - r.left, pt.y - r.top) {
            Hit::Tab(i) => d.tabs[i].id != id,
            Hit::Plus | Hit::More => true,
            Hit::None => false,
        };
        if gone {
            d.menu = false;
            d.hover = Hit::None;
            end_preview(d);
        }
        gone
    })
    .unwrap_or(false)
}

/// The plus button and the "+N" tile say what they do; anything else lets the tooltip go.
fn tip(d: &Deck, hit: Hit, by_key: bool) {
    use crate::win::ui::tooltip;
    let (label, k) = match hit {
        Hit::Plus => (t!("menu_new_note").to_string(), app::hotkey_text(crate::common::hotkey::NEW_NOTE)),
        // It opens All Notes, so it shares that shortcut.
        Hit::More => (tn!("tip_more_notes", d.geo.more), app::hotkey_text(crate::common::hotkey::ALL_NOTES)),
        _ => {
            tooltip::leave(d.hwnd);
            return;
        }
    };
    // Beside the button: under one, it would cover the other.
    match footer_rect(d, hit) {
        Some(r) => tooltip::beside(d.hwnd, r, &label, &k, by_key),
        None => tooltip::leave(d.hwnd),
    }
}

/// The context menu is gone. If it closed in place (a choice, Esc, a click elsewhere),
/// tuck the card back and let the real pointer position decide: still over the deck,
/// stay open; elsewhere, fold after the usual grace. While the deck has the keys, the
/// menu hands them back to it and the ring carries on from its card, unless what was
/// chosen took them elsewhere (a note opening).
fn menu_closed(hwnd: HWND) {
    let front = unsafe { GetForegroundWindow() };
    let frozen = with(|d| {
        let was = d.menu;
        if was {
            d.menu = false;
            d.hover = Hit::None;
            if d.kbd.is_some() && front == hwnd {
                show_ring(d);
                render(d);
                return false;
            }
            drop_keys(d);
            end_preview(d);
        }
        was
    })
    .unwrap_or(false);
    if frozen {
        unsafe { SetTimer(hwnd, T_COLLAPSE, motion::COLLAPSE_GRACE_MS, None) };
    }
    // The menu gave the keys back to the deck, which let them go meanwhile (Ctrl+Alt+K
    // again while the menu was up): typing would go nowhere, so they go on to the window
    // that had them before.
    if front == hwnd && with(|d| d.kbd.is_none()) == Some(true) {
        unsafe { PostMessageW(hwnd, WM_PASS_ON, 0, 0) };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let (x, y) = (x_of(lp), y_of(lp));
    let action = match msg {
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        WM_MOUSEMOVE => with(|d| {
            if !d.tracking {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                unsafe { TrackMouseEvent(&mut tme) };
                d.tracking = true;
            }
            unsafe { KillTimer(hwnd, T_COLLAPSE) };
            if !d.visible {
                // Fading out as it's hidden.
                return Action::None;
            }
            if !d.open {
                // Respond on the very first movement: switch to the wide window and fan out.
                fan_out(d);
                d.hover = Hit::None;
                render(d);
                return Action::None;
            }
            let fy = y as f32;
            if d.drag.as_ref().is_some_and(|dr| dr.idx >= d.tabs.len()) {
                d.drag = None;
            }
            if let Some(dr) = d.drag.as_mut() {
                if !dr.active && (y - dr.start_y).abs() > (5.0 * d.geo.s) as i32 {
                    dr.active = true;
                }
                if dr.active {
                    let now = std::time::Instant::now();
                    dr.samples.push((now, fy));
                    if dr.samples.len() > 5 {
                        dr.samples.remove(0);
                    }
                    let (idx, grab) = (dr.idx, dr.grab);
                    let g = d.geo;
                    let spines = d.tabs.iter().filter(|t| !t.open).count();
                    let min = g.top - g.step * 0.4;
                    let max = g.slot_y(spines.saturating_sub(1)) + g.step * 0.4;
                    let raw = fy - grab;
                    // Rubber-band past the ends of the stack.
                    let band = |over: f32| over * 0.55 * g.tab_h / (g.tab_h + 0.55 * over.abs());
                    let pos = if raw < min { min + band(raw - min) } else if raw > max { max + band(raw - max) } else { raw };
                    d.tabs[idx].y.snap(pos);
                    end_preview(d);
                    // Others make room, springing to their new slots (cards that are out
                    // of the deck have no spine and take no slot).
                    let target = (((pos - g.top) / g.step).round().max(0.0) as usize).min(spines.saturating_sub(1));
                    let mut slot = 0;
                    for j in 0..d.tabs.len() {
                        if j == idx || d.tabs[j].open {
                            continue;
                        }
                        if slot == target {
                            slot += 1;
                        }
                        d.tabs[j].y.to(g.slot_y(slot));
                        slot += 1;
                    }
                    anim::wake(Client::Deck);
                    return Action::None;
                }
            }
            // A note just opened from here, and the pointer hasn't moved since: the cards
            // that slid up under it aren't what it's pointing at.
            let mut at = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut at) };
            if d.still.is_some_and(|p| (p.x, p.y) != (at.x, at.y)) {
                d.still = None;
            }
            let hit = if d.still.is_some() { Hit::None } else { hit_test(d, x, y) };
            point_at(d, hit);
            // On every move: the buttons slide as a pulled-out card tucks back.
            tip(d, hit, false);
            Action::None
        }),
        WM_MOUSELEAVE => with(|d| {
            d.tracking = false;
            d.still = None;
            crate::win::ui::tooltip::leave(hwnd);
            // The pointer "leaves" for our own context menu: hold still until it closes.
            // While the deck has the keys it stays open, lit where the ring is.
            if d.kbd.is_some() && !d.menu {
                show_ring(d);
            } else if d.drag.is_none() && !d.menu {
                d.hover = Hit::None;
                unsafe {
                    KillTimer(hwnd, T_PREVIEW);
                    SetTimer(hwnd, T_COLLAPSE, motion::COLLAPSE_GRACE_MS, None);
                }
                anim::wake(Client::Deck);
            }
            Action::None
        }),
        WM_LBUTTONDOWN => with(|d| {
            crate::win::ui::tooltip::mute();
            unsafe { SetCapture(hwnd) };
            let hit = hit_test(d, x, y);
            d.pressed = hit;
            if let Hit::Tab(i) = hit {
                d.tabs[i].press.to(0.97);
                if d.tabs[i].pull.value < 0.5 {
                    let grab = y as f32 - d.tabs[i].y.value;
                    d.drag = Some(Drag { idx: i, grab, start_y: y, active: false, samples: Vec::new() });
                }
            }
            anim::wake(Client::Deck);
            Action::None
        }),
        WM_LBUTTONUP => with(|d| {
            unsafe { ReleaseCapture() };
            for t in d.tabs.iter_mut() {
                t.press.to(1.0);
            }
            let pressed = std::mem::replace(&mut d.pressed, Hit::None);
            anim::wake(Client::Deck);
            if let Some(dr) = d.drag.take().filter(|dr| dr.idx < d.tabs.len())
                && dr.active {
                    // Hand the pointer's velocity to the settling spring.
                    let v = match (dr.samples.first(), dr.samples.last()) {
                        (Some(a), Some(b)) if b.0 > a.0 => (b.1 - a.1) / (b.0 - a.0).as_secs_f32(),
                        _ => 0.0,
                    };
                    let g = d.geo;
                    let pos = d.tabs[dr.idx].y.value;
                    let dragged = d.tabs[dr.idx].id;
                    let mut spines: Vec<u64> = d.tabs.iter().filter(|t| !t.open && t.id != dragged).map(|t| t.id).collect();
                    let target = (((pos - g.top) / g.step).round().max(0.0) as usize).min(spines.len());
                    spines.insert(target, dragged);
                    // Settle into the new slot carrying the pointer's velocity; sync() will
                    // retarget everyone else after the store is reordered.
                    let t = &mut d.tabs[dr.idx];
                    t.y.retarget(g.slot_y(target), motion::MOMENTUM);
                    t.y.velocity = v.clamp(-4000.0, 4000.0);
                    // New order: reordered spines fill the spine positions; open / overflow
                    // notes keep their places.
                    let mut next = spines.into_iter();
                    let in_stack: Vec<u64> = d.tabs.iter().filter(|t| !t.open).map(|t| t.id).collect();
                    let ids: Vec<u64> = app::store(|s| s.active_ids())
                        .into_iter()
                        .map(|x| if in_stack.contains(&x) { next.next().unwrap_or(x) } else { x })
                        .collect();
                    return Action::Reorder(ids);
                }
            let hit = hit_test(d, x, y);
            if hit != pressed {
                return Action::None;
            }
            match hit {
                Hit::Tab(i) => Action::Open(d.tabs[i].id),
                Hit::Plus => Action::New,
                Hit::More => Action::More,
                Hit::None => Action::None,
            }
        }),
        WM_RBUTTONUP => with(|d| {
            let hit = hit_test(d, x, y);
            menu_for(d, hit, false)
        }),
        WM_KEYDOWN | WM_SYSKEYDOWN => with(|d| on_key(d, wp as u16)).flatten(),
        WM_CHAR => with(|d| {
            let ch = char::from_u32(wp as u32).filter(|c| !c.is_control())?;
            on_char(d, ch);
            Some(Action::None)
        })
        .flatten(),
        WM_ACTIVATE => {
            // The keys went elsewhere: a click in another window, a note opening. Not for
            // the card's own menu, which hands them back when it closes.
            if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                with(|d| {
                    if !d.menu {
                        drop_keys(d);
                    }
                });
            } else if with(|d| d.kbd.is_none() && !d.menu) == Some(true) {
                // Active without the keys (the note it opened has closed, and Windows gives
                // the activation back to the deck): typing would go nowhere, so it goes
                // on to the window that had the keys before Ctrl+Alt+K.
                unsafe { PostMessageW(hwnd, WM_PASS_ON, 0, 0) };
            }
            None
        }
        WM_PASS_ON => {
            unsafe {
                let front = GetForegroundWindow();
                // wp 1: a note in front was closed or archived (`pass_on_later`), and
                // Windows may have given the keys to the desktop rather than to the deck.
                let stray = front == hwnd || (wp == 1 && (front.is_null() || is_desktop(front)));
                if stray && with(|d| d.kbd.is_none()) == Some(true) {
                    let back = PASS_TO.replace(0) as HWND;
                    let to = if !back.is_null() && back != hwnd && IsWindow(back) != 0 && IsWindowVisible(back) != 0 {
                        back
                    } else if front == hwnd {
                        GetShellWindow()
                    } else {
                        null_mut()
                    };
                    if !to.is_null() {
                        SetForegroundWindow(to);
                    }
                }
            }
            Some(Action::None)
        }
        // Alt+F4 while the deck has the keys is Esc: the deck itself never closes. Nor is
        // there a menu bar for F10 or Alt to go to.
        WM_CLOSE if with(|d| d.kbd.is_some()) == Some(true) => Some(Action::GiveBack),
        WM_SYSCOMMAND if (wp & 0xFFF0) as u32 == SC_KEYMENU => Some(Action::None),
        WM_CAPTURECHANGED => with(|d| {
            if let Some(dr) = d.drag.take()
                && dr.active {
                    sync(d);
                }
            Action::None
        }),
        WM_TIMER => with(|d| {
            unsafe { KillTimer(hwnd, wp) };
            if d.menu {
                return Action::None;
            }
            match wp {
                T_PREVIEW => {
                    if let (Hit::Tab(i), true, None) = (d.hover, d.open, d.drag.as_ref()) {
                        if d.tabs.get(i).is_some_and(|t| t.reveal.value > 0.85) {
                            begin_preview(d, i);
                        } else {
                            // Still fanning out: look again shortly rather than never.
                            unsafe { SetTimer(hwnd, T_PREVIEW, 60, None) };
                        }
                    }
                }
                // The deck has the keys: it stays open until it gives them back.
                T_COLLAPSE if d.kbd.is_some() => {}
                T_COLLAPSE => {
                    let mut pt = POINT { x: 0, y: 0 };
                    let mut r: RECT = unsafe { std::mem::zeroed() };
                    unsafe {
                        GetCursorPos(&mut pt);
                        GetWindowRect(hwnd, &mut r);
                    }
                    // Moving/resizing the window under a still cursor cancels hover tracking,
                    // so decide by where the cursor really is: the tab column or a card.
                    let (lx, ly) = (pt.x - r.left, pt.y - r.top);
                    let col = ((TAB_W + BULGE + 10.0) * d.geo.s) as i32;
                    let in_col = if d.geo.left { lx < col } else { lx >= (r.right - r.left) - col };
                    let inside = crate::win::util::pt_in(&r, pt.x, pt.y) && (in_col || hit_test(d, lx, ly) != Hit::None);
                    if inside {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        unsafe { TrackMouseEvent(&mut tme) };
                        d.tracking = true;
                    } else if d.drag.is_none() {
                        set_open(d, false, false);
                    }
                }
                _ => {}
            }
            Action::None
        }),
        WM_APPBAR => with(|d| {
            if wp as u32 == ABN_FULLSCREENAPP {
                // Sent for a full-screen window on any monitor, coming or going: the deck
                // only steps aside while one covers its own, whatever the others do.
                check_fullscreen(d);
            }
            Action::None
        }),
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => with(|d| {
            // A display, a work area or a scale changed: the deck's display is worked
            // out again (and is back on the chosen one if it was just reconnected).
            crate::win::ui::displays::forget();
            sync(d);
            render(d);
            // A full-screen window may cover the deck's display now, or no longer.
            if msg != WM_SETTINGCHANGE {
                check_fullscreen(d);
            }
            Action::None
        }),
        _ => None,
    };
    match action {
        Some(Action::Open(id)) => {
            hold_still();
            app::toggle_note(id)
        }
        Some(Action::New) => {
            hold_still();
            app::new_note()
        }
        Some(Action::More) => app::show_all_notes(false),
        Some(Action::TabMenu(id, at)) => {
            // Beside the pulled-out card, never over it, at the height you clicked (from
            // the keyboard, near the card's top).
            let mut p = at.unwrap_or_else(|| {
                let mut p = POINT { x: 0, y: 0 };
                unsafe { GetCursorPos(&mut p) };
                p
            });
            let left = with(|d| {
                let reach = ((CARD_W + PULL_GAP + 10.0) * d.geo.s) as i32;
                p.x = if d.geo.left { d.geo.work.left + reach } else { d.geo.work.right - reach };
                d.geo.left
            })
            .unwrap_or(false);
            // Opened from the keyboard, the menu stays put wherever the pointer happens to
            // rest: it wasn't the pointer that brought it.
            let leave: Option<Box<dyn Fn(POINT) -> bool>> = if at.is_some() { None } else { Some(Box::new(move |pt| leave_menu(hwnd, id, pt))) };
            app::note_menu_beside(id, p, !left, leave);
            menu_closed(hwnd);
        }
        Some(Action::DeckMenu(at)) => {
            app::tray_menu(at);
            menu_closed(hwnd);
        }
        Some(Action::GiveBack) => give_keys_back(hwnd),
        Some(Action::Reorder(ids)) => {
            app::store_mut(|s| s.reorder(&ids));
            app::save();
            app::refresh();
        }
        Some(Action::None) => {}
        None => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    0
}
