// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Undo HUD: a dark capsule that rises from the bottom of the screen with a countdown
//! ring. Click it within ten seconds to restore what was deleted (or archived). The countdown waits
//! while the pointer is on the toast, so reaching for Undo never races the clock.

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::util::{rect, wide};

const CLASS: &str = "PeekABooToast";
const SECONDS: f32 = 10.0;

struct Toast {
    hwnd: HWND,
    count: usize,
    /// Archived rather than deleted.
    archived: bool,
    /// A plain message instead (notes can't be saved): nothing to undo, so no button and
    /// no ring; it simply goes after its time.
    notice: Option<String>,
    /// Seconds of countdown left.
    left: f32,
    rise: Spring,
    /// The Undo button lights up while the pointer is on the toast (all of it undoes).
    hot: Spring,
    /// Mouse button held down on the toast: the button sinks a little.
    pressed: bool,
    alive: bool,
    canvas: Option<Canvas>,
    /// The work area it rises in (the monitor in use when it was raised) and that
    /// monitor's scale.
    work: RECT,
    s: f32,
    body: Font,
    bold: Font,
    tiny: Font,
}

thread_local! {
    static TOAST: RefCell<Option<Toast>> = const { RefCell::new(None) };
}

pub fn show(count: usize, archived: bool) {
    raise(count, archived, None);
}

/// Tell the user something that can't be undone or acted on here, for a few seconds.
pub fn notice(text: String) {
    raise(0, false, Some(text));
}

fn raise(count: usize, archived: bool, notice: Option<String>) {
    let (work, s) = crate::win::ui::screen::active();
    TOAST.with(|t| {
        let mut t = t.borrow_mut();
        if t.is_none() {
            app::register_class(CLASS, Some(wndproc), IDC_HAND);
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    wide(CLASS).as_ptr(),
                    wide(t!("window_undo")).as_ptr(),
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
            let (body, bold, tiny) = fonts(s);
            *t = Some(Toast {
                hwnd,
                count,
                archived,
                notice: None,
                left: SECONDS,
                rise: Spring::unit(0.0, theme::motion::MOMENTUM),
                hot: Spring::unit(0.0, (1.0, 0.12)),
                pressed: false,
                alive: true,
                canvas: None,
                work,
                s,
                body,
                bold,
                tiny,
            });
        }
        let t = t.as_mut().unwrap();
        // Raised again while showing, perhaps from another monitor: it moves there.
        t.work = work;
        if t.s != s {
            (t.body, t.bold, t.tiny) = fonts(s);
            t.s = s;
        }
        t.count = count;
        t.archived = archived;
        t.notice = notice;
        t.left = SECONDS;
        t.alive = true;
        t.rise.to(1.0);
    });
    anim::wake(Client::Toast);
}

/// The toast's (body, bold, ring digit) fonts at scale `s`.
fn fonts(s: f32) -> (Font, Font, Font) {
    (
        Font::new(13.5 * s, FW_NORMAL, theme::FACE_TEXT),
        Font::new(13.5 * s, FW_SEMIBOLD, theme::FACE_TEXT),
        Font::new(9.5 * s, FW_SEMIBOLD, theme::FACE_SMALL),
    )
}

pub fn hide() {
    TOAST.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.alive = false;
            t.rise.to(0.0);
        }
    });
    anim::wake(Client::Toast);
}

/// Gone at once, without the slide out (quitting).
pub fn close() {
    if let Some(t) = TOAST.with(|t| t.borrow_mut().take()) {
        unsafe { DestroyWindow(t.hwnd) };
    }
}

pub fn frame(dt: f32) -> bool {
    let mut expire = false;
    let keep = TOAST.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(t) = slot.as_mut() else { return false };
        let moving = t.rise.step(dt);
        let over = hovered(t.hwnd);
        t.pressed &= over;
        t.hot.to(if over && t.alive && t.notice.is_none() { 1.0 } else { 0.0 });
        t.hot.step(dt);
        if t.alive && !over {
            t.left -= dt;
        }
        if t.alive && t.left <= 0.0 {
            if t.notice.is_some() {
                // Nothing pending behind a notice: it just goes.
                t.alive = false;
                t.rise.to(0.0);
            } else {
                expire = true;
            }
        }
        if !t.alive && !moving && t.rise.value <= 0.01 {
            unsafe { DestroyWindow(t.hwnd) };
            *slot = None;
            return false;
        }
        render(t);
        true
    });
    if expire {
        app::finalize_delete();
    }
    keep
}

fn hovered(h: HWND) -> bool {
    let mut p = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(h, &mut r);
    }
    crate::win::util::pt_in(&r, p.x, p.y)
}

fn render(t: &mut Toast) {
    let s = t.s;
    let (w, h, m) = ((320.0 * s) as i32, (52.0 * s) as i32, (24.0 * s) as i32);
    let (cw, ch) = (w + 2 * m, h + 2 * m);
    if t.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
        t.canvas = Some(Canvas::new(cw, ch));
    }
    let c = t.canvas.as_mut().unwrap();
    c.clear(0);
    let a = clamp01(t.rise.value);
    let lift = (1.0 - t.rise.value) * 28.0 * s;
    let (x, y) = (m as f32, m as f32 * 0.6 + lift);
    let card = Shape::new(x, y, w as f32, h as f32, h as f32 / 2.0);
    c.soft_shadow(&card, s, 1.4 * a);
    c.shape(&card, 0x1C1C1E, 0.94 * a, 1.0, 0.0);
    c.shape(&card, 0xFFFFFF, 0.10 * a, 1.0, 1.0);
    c.begin_batch();
    if let Some(msg) = &t.notice {
        c.text(msg, &t.body, rect((x + 20.0 * s) as i32, y as i32, w - (40.0 * s) as i32, h), 0xF2F2F2, a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        c.flush();
        present(t, cw, ch);
        return;
    }
    // Countdown ring.
    let left = t.left.clamp(0.0, SECONDS);
    let (rcx, rcy, rr) = (x + 26.0 * s, y + h as f32 / 2.0, 10.0 * s);
    c.ring(rcx, rcy, rr, 2.0 * s, 1.0, 0xFFFFFF, 0.16 * a);
    c.ring(rcx, rcy, rr, 2.0 * s, left / SECONDS, theme::COLORS[0].accent, a);
    let secs = rect((rcx - rr) as i32, (rcy - rr) as i32, (2.0 * rr) as i32, (2.0 * rr) as i32);
    c.text(&format!("{}", left.ceil() as i32), &t.tiny, secs, 0xFFFFFF, 0.85 * a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let msg = if t.archived { tn!("toast_archived", t.count) } else { tn!("toast_deleted", t.count) };
    let tx = (x + 48.0 * s) as i32;
    c.text(&msg, &t.body, rect(tx, y as i32, (w as f32 * 0.6) as i32, h), 0xF2F2F2, a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    let (bw, bh) = (76.0 * s, 32.0 * s);
    let button = Shape::new(x + w as f32 - bw - 10.0 * s, y + (h as f32 - bh) / 2.0, bw, bh, bh / 2.0);
    // Brighter under the pointer; pressed, it sinks a little and brightens a touch more.
    let fill = if t.pressed { button.inset(1.5 * s) } else { button };
    let lit = 0.12 * clamp01(t.hot.value) + if t.pressed { 0.08 } else { 0.0 };
    c.shape(&fill, 0xFFFFFF, (0.14 + lit) * a, 1.0, 0.0);
    let br = rect(button.x as i32, button.y as i32, bw as i32, bh as i32);
    c.text(t!("toast_undo"), &t.bold, br, theme::COLORS[0].accent, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    c.flush();
    present(t, cw, ch);
}

/// Put the drawn toast at the bottom of the work area.
fn present(t: &mut Toast, cw: i32, ch: i32) {
    let (px, py) = crate::win::ui::screen::bottom_centre(t.work, cw, ch, (12.0 * t.s) as i32);
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
            // Shown on the press, done on the release.
            WM_LBUTTONDOWN => {
                TOAST.with(|t| {
                    if let Some(t) = t.borrow_mut().as_mut() {
                        t.pressed = true;
                    }
                });
                anim::wake(Client::Toast);
                return 0;
            }
            WM_LBUTTONUP => {
                let notice = TOAST.with(|t| {
                    t.borrow_mut().as_mut().map(|t| {
                        t.pressed = false;
                        t.notice.is_some()
                    })
                });
                // A notice has nothing to undo: a click just dismisses it.
                if notice == Some(true) { hide() } else { app::undo_delete() }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
