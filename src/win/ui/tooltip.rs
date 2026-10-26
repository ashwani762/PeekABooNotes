// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Tooltips for icon-only buttons, drawn like our menus: a small rounded card with a soft
//! shadow, the action and (dimmer) its shortcut. It waits a moment before the first one
//! shows; once one is up, moving to the next button swaps it at once.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ScreenToClient,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::app;
use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::util::{rect, wide};

const CLASS: &str = "PeekABooTip";
const T_SHOW: usize = 1;
/// Resting on a button this long brings the first tooltip up.
const DELAY_MS: u32 = 550;
/// A tooltip that went away less than this long ago comes back without the wait.
const WARM_MS: u128 = 900;

struct Tip {
    hwnd: HWND,
    /// The button it's for: its window and rect there (client coordinates).
    owner: HWND,
    anchor: RECT,
    label: String,
    keys: String,
    /// Waiting out the delay.
    pending: bool,
    /// Brought up by keyboard focus, not the pointer: it shows wherever the pointer is.
    by_key: bool,
    /// Beside the button rather than under it (one of a column of buttons, which it would
    /// cover).
    beside: bool,
    /// Clicked: stay away until the pointer moves to another button.
    muted: bool,
    shown: bool,
    hidden_at: Option<Instant>,
    fade: Spring,
    s: f32,
    fonts: Option<(Font, Font)>,
    canvas: Option<Canvas>,
    origin: (i32, i32),
    size: (i32, i32),
    margin: i32,
    above: bool,
}

thread_local! {
    static TIP: RefCell<Option<Tip>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Tip) -> R) -> Option<R> {
    TIP.with(|t| {
        let mut t = t.try_borrow_mut().ok()?;
        if t.is_none() {
            app::register_class(CLASS, Some(wndproc), IDC_ARROW);
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    wide(CLASS).as_ptr(),
                    wide("Tip").as_ptr(),
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
            if hwnd.is_null() {
                return None;
            }
            *t = Some(Tip {
                hwnd,
                owner: null_mut(),
                anchor: rect(0, 0, 0, 0),
                label: String::new(),
                keys: String::new(),
                pending: false,
                by_key: false,
                beside: false,
                muted: false,
                shown: false,
                hidden_at: None,
                fade: Spring::unit(0.0, (1.0, 0.12)),
                s: 1.0,
                fonts: None,
                canvas: None,
                origin: (0, 0),
                size: (0, 0),
                margin: 0,
                above: false,
            });
        }
        t.as_mut().map(f)
    })
}

/// The pointer rests on a button of `owner` at `anchor` (client coordinates).
/// Call it on every move over the button; repeats are free.
pub fn hover(owner: HWND, anchor: RECT, label: &str, keys: &str) {
    rest(owner, anchor, label, keys, false, false);
}

/// Keyboard focus reached a button of `owner` at `anchor`: its tooltip comes up as for
/// the pointer, after the same wait.
pub fn focus(owner: HWND, anchor: RECT, label: &str, keys: &str) {
    rest(owner, anchor, label, keys, true, false);
}

/// `hover` or (`by_key`) `focus`, the tip beside the button, on whichever side has room:
/// for a button in a column of them (the deck's foot), which a tip under it would cover.
pub fn beside(owner: HWND, anchor: RECT, label: &str, keys: &str, by_key: bool) {
    rest(owner, anchor, label, keys, by_key, true);
}

fn rest(owner: HWND, anchor: RECT, label: &str, keys: &str, by_key: bool, beside: bool) {
    with(|t| {
        let same = t.owner == owner && t.label == label && t.keys == keys && (t.anchor.left, t.anchor.top) == (anchor.left, anchor.top);
        if same && (t.pending || t.shown || t.muted) {
            return;
        }
        t.owner = owner;
        t.anchor = anchor;
        t.label = label.to_string();
        t.keys = keys.to_string();
        t.by_key = by_key;
        t.beside = beside;
        t.muted = false;
        let warm = t.shown || t.hidden_at.is_some_and(|h| h.elapsed().as_millis() < WARM_MS);
        if warm {
            t.pending = false;
            unsafe { KillTimer(t.hwnd, T_SHOW) };
            show(t, true);
        } else {
            t.pending = true;
            unsafe { SetTimer(t.hwnd, T_SHOW, DELAY_MS, None) };
        }
    });
}

/// The pointer left `owner`'s button (or the window), or `owner` is going away. A tip
/// for a window inside it (a note's text in All Notes) goes too: it would otherwise
/// outlive its window, over whatever is there.
pub fn leave(owner: HWND) {
    let live = TIP.with(|t| t.try_borrow().map(|t| t.is_some()).unwrap_or(false));
    if live {
        with(|t| {
            if t.owner == owner || (!t.owner.is_null() && unsafe { IsChild(owner, t.owner) } != 0) {
                t.owner = null_mut();
                t.muted = false;
                go_away(t);
            }
        });
    }
}

/// A click: the tooltip gets out of the way and stays away while the pointer is on
/// that button.
pub fn mute() {
    let live = TIP.with(|t| t.try_borrow().map(|t| t.is_some()).unwrap_or(false));
    if live {
        with(|t| {
            t.muted = true;
            go_away(t);
            // At once, not faded: a click may open a dialog or menu whose own message
            // loop would hold the fade half-way, a ghost over it.
            t.fade.snap(0.0);
            t.canvas = None;
            unsafe { ShowWindow(t.hwnd, SW_HIDE) };
            // A click is not a "just saw a tooltip" moment: the next one waits again.
            t.hidden_at = None;
        });
    }
}

/// A key went to `owner` (typing, say): its tooltip, if it has one up or coming, goes as
/// for a click, and stays away while the pointer rests where it is.
pub fn mute_for(owner: HWND) {
    let mine = TIP.with(|t| t.try_borrow().map(|t| t.as_ref().is_some_and(|t| t.owner == owner && (t.shown || t.pending))).unwrap_or(false));
    if mine {
        mute();
    }
}

fn go_away(t: &mut Tip) {
    unsafe { KillTimer(t.hwnd, T_SHOW) };
    t.pending = false;
    if t.shown {
        t.shown = false;
        t.hidden_at = Some(Instant::now());
        t.fade.to(0.0);
        anim::wake(Client::Tip);
    }
}

fn show(t: &mut Tip, swap: bool) {
    if t.owner.is_null() || unsafe { IsWindowVisible(t.owner) } == 0 {
        return;
    }
    // The button's rect on screen, and the monitor it's on.
    let mut tl = POINT { x: t.anchor.left, y: t.anchor.top };
    let mut br = POINT { x: t.anchor.right, y: t.anchor.bottom };
    unsafe {
        ClientToScreen(t.owner, &mut tl);
        ClientToScreen(t.owner, &mut br);
    }
    let (s, work) = unsafe {
        let mon = MonitorFromPoint(POINT { x: (tl.x + br.x) / 2, y: br.y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (dx.max(96) as f32 / 96.0, mi.rcWork)
    };
    if t.fonts.is_none() || t.s != s {
        t.s = s;
        t.fonts = Some((Font::new(12.5 * s, FW_NORMAL, theme::FACE_TEXT), Font::new(11.0 * s, FW_SEMIBOLD, theme::FACE_SMALL)));
    }
    let (w, h) = measure(t);
    // Centred under the button; above it when there's no room below.
    let gap = (6.0 * s) as i32;
    let mut x = (tl.x + br.x) / 2 - w / 2;
    let mut y = br.y + gap;
    t.above = y + h > work.bottom;
    if t.beside {
        // Level with the button, on the side with room (left of one at the right edge).
        y = (tl.y + br.y) / 2 - h / 2;
        x = if br.x + gap + w <= work.right { br.x + gap } else { tl.x - gap - w };
        t.above = false;
    } else if t.above {
        y = tl.y - gap - h;
    }
    x = x.clamp(work.left + gap, (work.right - gap - w).max(work.left));
    y = y.clamp(work.top, (work.bottom - h).max(work.top));
    t.margin = (26.0 * s) as i32;
    t.size = (w, h);
    t.origin = (x - t.margin, y - t.margin);
    t.shown = true;
    if !swap || t.fade.value < 0.05 {
        // Fresh: fade in with a short settle toward the button.
        t.fade.snap(0.0);
    }
    t.fade.to(1.0);
    render(t);
    // Above everything, pinned notes included.
    unsafe { SetWindowPos(t.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW) };
    anim::wake(Client::Tip);
}

// Metrics (device-independent px).
const H: f32 = 30.0;
const PAD_L: f32 = 11.0;
/// Keycaps sit closer to the edge: their own fill already pads them.
const PAD_R: f32 = 6.0;
const CAP_H: f32 = 19.0;
const CAP_PAD: f32 = 5.5;
const CAP_GAP: f32 = 3.0;
const LABEL_CAPS: f32 = 10.0;

/// The shortcut's keys, one keycap each ("Ctrl", "Shift", "A").
fn caps(t: &Tip) -> Vec<(&str, i32)> {
    let Some((_, key_f)) = t.fonts.as_ref() else { return Vec::new() };
    let min = (CAP_H * t.s) as i32;
    t.keys
        .split('+')
        .filter(|k| !k.is_empty())
        .map(|k| (k, (key_f.measure(k).0 + (2.0 * CAP_PAD * t.s) as i32).max(min)))
        .collect()
}

fn measure(t: &Tip) -> (i32, i32) {
    let s = t.s;
    let Some((label_f, _)) = t.fonts.as_ref() else { return (0, 0) };
    let lw = label_f.measure(&t.label).0;
    let caps = caps(t);
    let w = if caps.is_empty() {
        lw + (2.0 * PAD_L * s) as i32
    } else {
        let cw: i32 = caps.iter().map(|c| c.1).sum::<i32>() + (CAP_GAP * s) as i32 * (caps.len() as i32 - 1);
        lw + cw + ((PAD_L + LABEL_CAPS + PAD_R) * s) as i32
    };
    (w, (H * s) as i32)
}

pub fn frame(dt: f32) -> bool {
    with(|t| {
        let moving = t.fade.step(dt);
        if !t.shown && t.fade.value < 0.02 {
            unsafe { ShowWindow(t.hwnd, SW_HIDE) };
            t.canvas = None; // nothing to keep while hidden
            return false;
        }
        render(t);
        moving
    })
    .unwrap_or(false)
}

fn render(t: &mut Tip) {
    let Some((label_f, key_f)) = t.fonts.as_ref() else { return };
    let dark = app::dark();
    // The menu's palette: (surface, edge, text, secondary).
    let (surface, edge, text, sub) = if dark { (0x2C2C2E, 0xFFFFFF, 0xF2F2F7, 0x98989D) } else { (0xF7F7F9, 0x000000, 0x1D1D1F, 0x86868B) };
    let s = t.s;
    let (w, h) = t.size;
    let (cw, ch) = (w + 2 * t.margin, h + 2 * t.margin);
    if t.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
        t.canvas = Some(Canvas::new(cw, ch));
    }
    let mut c = t.canvas.take().unwrap();
    c.clear(0);
    let a = clamp01(t.fade.value);
    let settle = (1.0 - t.fade.value) * 3.0 * s * if t.above { 1.0 } else { -1.0 };
    let (ox, oy) = (t.margin as f32, t.margin as f32 + settle);
    let card = Shape::new(ox, oy, w as f32, h as f32, 8.0 * s);
    c.card(&card, surface, 1.0, edge, if dark { 0.14 } else { 0.09 }, 1.3, s);
    if dark {
        // Light catching the top edge.
        c.rrect(ox + 7.0 * s, oy + 1.0, w as f32 - 14.0 * s, 1.0, 0.0, 0xFFFFFF, 0.07);
    }
    c.begin_batch();
    let y0 = oy.round() as i32;
    let x0 = ox as i32 + (PAD_L * s) as i32;
    let lw = label_f.measure(&t.label).0;
    c.text_ct(&t.label, label_f, rect(x0, y0, lw + 2, h), None, text, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    let (cap_h, cap_y) = (CAP_H * s, oy + (h as f32 - CAP_H * s) / 2.0);
    // A touch darker than secondary text in light mode, where the caps read faint.
    let key_ink = if dark { sub } else { 0x6E6E73 };
    let mut kx = (x0 + lw) as f32 + LABEL_CAPS * s;
    for (k, kw) in caps(t) {
        let cap = Shape::new(kx, cap_y, kw as f32, cap_h, 5.0 * s);
        c.shape(&cap, edge, if dark { 0.10 } else { 0.06 }, 1.0, 0.0);
        c.shape(&cap, edge, if dark { 0.10 } else { 0.08 }, 1.0, 1.0);
        c.text_ct(k, key_f, rect(kx as i32, cap_y.round() as i32, kw, cap_h.round() as i32), None, key_ink, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        kx += kw as f32 + CAP_GAP * s;
    }
    c.flush();
    c.make_opaque_rect(rect((ox + 3.0 * s) as i32, (oy + 3.0 * s) as i32, w - (6.0 * s) as i32, h - (6.0 * s) as i32));
    if a < 0.999 {
        c.scale_alpha(a);
    }
    c.present(t.hwnd, t.origin.0, t.origin.1);
    t.canvas = Some(c);
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp == T_SHOW => {
                KillTimer(h, T_SHOW);
                with(|t| {
                    if t.pending {
                        t.pending = false;
                        // Only while the pointer is still over the button (or it has focus).
                        let mut p = POINT { x: 0, y: 0 };
                        GetCursorPos(&mut p);
                        let mut q = p;
                        if !t.owner.is_null() && (t.by_key || (ScreenToClient(t.owner, &mut q) != 0 && crate::win::util::pt_in(&t.anchor, q.x, q.y))) {
                            show(t, false);
                        }
                    }
                });
                return 0;
            }
            WM_NCHITTEST => return HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
