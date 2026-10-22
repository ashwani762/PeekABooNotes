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
    FW_NORMAL, FW_SEMIBOLD, GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::Shell::{ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA, SHAppBarMessage};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::render::anim::{self, Client, Spring, clamp01, lerp};
use crate::app;
use crate::render::gfx::{Canvas, Font, Shape};
use crate::store::{body_of, title_of};
use crate::theme::{self, motion};
use crate::platform::util::{WM_MOUSELEAVE, rect, wide, x_of, y_of};

mod layout;
mod draw;
use layout::*;
use draw::*;

const CLASS: &str = "PeekABooDeck";
const T_PREVIEW: usize = 2;
const T_COLLAPSE: usize = 3;
const WM_APPBAR: u32 = WM_APP + 20;

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
            reveal: Spring::unit(0.0, motion::SHINGLE),
            delay: 0.0,
            reveal_goal: 0.0,
            bulge: Spring::px(0.0, motion::SNAPPY),
            lean: Spring::unit(LEAN, motion::DEFAULT),
            y: Spring::px(y, motion::DEFAULT),
            press: Spring::unit(1.0, motion::SNAPPY),
            pull: Spring::unit(0.0, motion::DEFAULT),
            fitted: None,
            body_h: None,
            slot: 0,
        }
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
    geo: Geo,
    fonts: HashMap<(u8, i32, i32, u8), Font>,
    font_scale: f32,
    canvas: Option<Canvas>,
    /// The hovered tab whose spine label is too long scrolls: (note, since).
    marquee: Option<(u64, std::time::Instant)>,
    /// A label scrolled in the last frame, so the frame clock keeps running.
    marquee_live: bool,
}

thread_local! {
    static DECK: RefCell<Option<Deck>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Deck) -> R) -> Option<R> {
    DECK.with(|d| d.try_borrow_mut().ok().and_then(|mut d| d.as_mut().map(f)))
}

pub fn monitor_geo() -> (RECT, f32) {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (mi.rcWork, dx.max(96) as f32 / 96.0)
    }
}

pub fn create() {
    app::register_class(CLASS, Some(wndproc), IDC_HAND);
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    let hwnd = unsafe {
        CreateWindowExW(ex, wide(CLASS).as_ptr(), wide(t!("app_name")).as_ptr(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), app::hinst(), null_mut())
    };
    DECK.with(|d| {
        *d.borrow_mut() = Some(Deck {
            hwnd,
            tabs: Vec::new(),
            open: false,
            wide: false,
            previewing: false,
            pill: Spring::unit(1.0, motion::DEFAULT),
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
            geo: Geo::default(),
            fonts: HashMap::new(),
            font_scale: 0.0,
            canvas: None,
            marquee: None,
            marquee_live: false,
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
        update_visibility(d);
        sync(d);
        render(d);
    });
}

pub fn frame(dt: f32) -> bool {
    with(|d| {
        let moving = step(d, dt);
        if !d.open && !moving && d.wide {
            d.wide = false; // fully folded: shrink back to the small pill window
        }
        d.marquee_live = false;
        render(d);
        moving || d.marquee_live
    })
    .unwrap_or(false)
}

fn update_visibility(d: &mut Deck) {
    let (hidden, over_fs) = app::store(|s| (s.d.settings.deck_hidden, s.d.settings.over_fullscreen));
    let show = !hidden && (!d.fullscreen || over_fs);
    if show != d.visible {
        d.visible = show;
        if !show {
            unsafe { ShowWindow(d.hwnd, SW_HIDE) };
        }
        set_open(d, false, true);
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
pub fn card_image(id: u64) -> Option<crate::render::gfx::Image> {
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
    TabMenu(u64),
    DeckMenu,
    Reorder(Vec<u64>),
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

/// While a card's menu is up: has the pointer moved on to another card (or the + / more
/// buttons)? Then the menu and the pulled-out card go away together, and the deck is
/// live again under the pointer for whatever it reaches next.
fn leave_menu(hwnd: HWND, id: u64, pt: POINT) -> bool {
    with(|d| {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(hwnd, &mut r) };
        if !crate::platform::util::pt_in(&r, pt.x, pt.y) {
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

/// The context menu is gone. If it closed in place (a choice, Esc, a click elsewhere),
/// tuck the card back and let the real pointer position decide: still over the deck,
/// stay open; elsewhere, fold after the usual grace.
fn menu_closed(hwnd: HWND) {
    let frozen = with(|d| {
        let was = d.menu;
        if was {
            d.menu = false;
            d.hover = Hit::None;
            end_preview(d);
        }
        was
    })
    .unwrap_or(false);
    if frozen {
        unsafe { SetTimer(hwnd, T_COLLAPSE, motion::COLLAPSE_GRACE_MS, None) };
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
            if !d.open {
                // Respond on the very first movement: switch to the wide window and fan out.
                set_open(d, true, false);
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
            let hit = hit_test(d, x, y);
            if hit != d.hover {
                d.hover = hit;
                unsafe { KillTimer(hwnd, T_PREVIEW) };
                match hit {
                    Hit::Tab(i) if d.previewing => begin_preview(d, i), // scrub between cards instantly
                    Hit::Tab(_) => unsafe {
                        SetTimer(hwnd, T_PREVIEW, motion::PREVIEW_DWELL_MS, None);
                    },
                    _ => {
                        if d.previewing {
                            end_preview(d);
                        }
                    }
                }
                anim::wake(Client::Deck);
            }
            Action::None
        }),
        WM_MOUSELEAVE => with(|d| {
            d.tracking = false;
            // The pointer "leaves" for our own context menu: hold still until it closes.
            if d.drag.is_none() && !d.menu {
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
            // The menu takes the stage: tuck any pulled-out card away at once, but keep
            // the deck open and the clicked card lit, so it's clear what the menu acts on.
            unsafe {
                KillTimer(hwnd, T_PREVIEW);
                KillTimer(hwnd, T_COLLAPSE);
            }
            let hit = hit_test(d, x, y);
            if let Hit::Tab(i) = hit {
                // The card comes (or stays) out, so you can see the note you're acting on.
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
            match hit {
                Hit::Tab(i) => Action::TabMenu(d.tabs[i].id),
                _ => Action::DeckMenu,
            }
        }),
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
                    let inside = crate::platform::util::pt_in(&r, pt.x, pt.y) && (in_col || hit_test(d, lx, ly) != Hit::None);
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
                d.fullscreen = lp != 0;
                update_visibility(d);
                render(d);
            }
            Action::None
        }),
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => with(|d| {
            sync(d);
            render(d);
            Action::None
        }),
        _ => None,
    };
    match action {
        Some(Action::Open(id)) => app::toggle_note(id),
        Some(Action::New) => app::new_note(),
        Some(Action::More) => app::show_all_notes(false),
        Some(Action::TabMenu(id)) => {
            // Beside the pulled-out card, never over it, at the height you clicked.
            let mut p = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut p) };
            let left = with(|d| {
                let reach = ((CARD_W + PULL_GAP + 10.0) * d.geo.s) as i32;
                p.x = if d.geo.left { d.geo.work.left + reach } else { d.geo.work.right - reach };
                d.geo.left
            })
            .unwrap_or(false);
            app::note_menu_beside(id, p, !left, Box::new(move |pt| leave_menu(hwnd, id, pt)));
            menu_closed(hwnd);
        }
        Some(Action::DeckMenu) => {
            app::tray_menu();
            menu_closed(hwnd);
        }
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
