//! Undo HUD: a dark capsule that rises from the bottom of the screen with a countdown
//! ring. Click it within ten seconds to restore what was deleted. The countdown waits
//! while the pointer is on the toast, so reaching for Undo never races the clock.

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, FW_SEMIBOLD};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::render::anim::{self, Client, Spring, clamp01};
use crate::app;
use crate::render::gfx::{Canvas, Font, Shape};
use crate::theme;
use crate::platform::util::{rect, wide};

const CLASS: &str = "PeekabooToast";
const SECONDS: f32 = 10.0;

struct Toast {
    hwnd: HWND,
    count: usize,
    /// Seconds of countdown left.
    left: f32,
    rise: Spring,
    alive: bool,
    canvas: Option<Canvas>,
    s: f32,
    body: Font,
    bold: Font,
    tiny: Font,
}

thread_local! {
    static TOAST: RefCell<Option<Toast>> = const { RefCell::new(None) };
}

pub fn show(count: usize) {
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
            let (_, s) = crate::ui::deck::monitor_geo();
            *t = Some(Toast {
                hwnd,
                count,
                left: SECONDS,
                rise: Spring::unit(0.0, theme::motion::MOMENTUM),
                alive: true,
                canvas: None,
                s,
                body: Font::new(13.5 * s, FW_NORMAL, theme::FACE_TEXT),
                bold: Font::new(13.5 * s, FW_SEMIBOLD, theme::FACE_TEXT),
                tiny: Font::new(9.5 * s, FW_SEMIBOLD, theme::FACE_SMALL),
            });
        }
        let t = t.as_mut().unwrap();
        t.count = count;
        t.left = SECONDS;
        t.alive = true;
        t.rise.to(1.0);
    });
    anim::wake(Client::Toast);
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

pub fn frame(dt: f32) -> bool {
    let mut expire = false;
    let keep = TOAST.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(t) = slot.as_mut() else { return false };
        let moving = t.rise.step(dt);
        if t.alive && !hovered(t.hwnd) {
            t.left -= dt;
        }
        if t.alive && t.left <= 0.0 {
            expire = true;
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
    crate::platform::util::pt_in(&r, p.x, p.y)
}

fn render(t: &mut Toast) {
    let (work, _) = crate::ui::deck::monitor_geo();
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
    // Countdown ring.
    let left = t.left.clamp(0.0, SECONDS);
    let (rcx, rcy, rr) = (x + 26.0 * s, y + h as f32 / 2.0, 10.0 * s);
    c.ring(rcx, rcy, rr, 2.0 * s, 1.0, 0xFFFFFF, 0.16 * a);
    c.ring(rcx, rcy, rr, 2.0 * s, left / SECONDS, theme::COLORS[0].accent, a);
    let secs = rect((rcx - rr) as i32, (rcy - rr) as i32, (2.0 * rr) as i32, (2.0 * rr) as i32);
    c.text(&format!("{}", left.ceil() as i32), &t.tiny, secs, 0xFFFFFF, 0.85 * a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    let msg = tn!("toast_deleted", t.count);
    let tx = (x + 48.0 * s) as i32;
    c.text(&msg, &t.body, rect(tx, y as i32, (w as f32 * 0.6) as i32, h), 0xF2F2F2, a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    let (bw, bh) = (76.0 * s, 32.0 * s);
    let button = Shape::new(x + w as f32 - bw - 10.0 * s, y + (h as f32 - bh) / 2.0, bw, bh, bh / 2.0);
    c.shape(&button, 0xFFFFFF, 0.14 * a, 1.0, 0.0);
    let br = rect(button.x as i32, button.y as i32, bw as i32, bh as i32);
    c.text(t!("toast_undo"), &t.bold, br, theme::COLORS[0].accent, a, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    c.flush();
    let px = (work.left + work.right) / 2 - cw / 2;
    let py = work.bottom - ch - (12.0 * s) as i32;
    c.present(t.hwnd, px, py);
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
            WM_LBUTTONUP => {
                app::undo_delete();
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
