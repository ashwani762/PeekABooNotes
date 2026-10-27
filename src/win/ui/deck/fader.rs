// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck as it was, fading out where it was, while the deck itself fades in somewhere
//! else (the other edge, another display): a window of its own that takes no input,
//! holding the deck's last picture, gone once that has faded to nothing.

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::app;
use crate::win::render::anim::FadeOut;
use crate::win::render::gfx::Canvas;
use crate::win::util::{rect, wide};

const CLASS: &str = "PeekABooDeckFade";
/// As long as the pill takes to fade in at its new place, about.
const OUT_S: f32 = 0.2;

struct Fader {
    hwnd: HWND,
    canvas: Canvas,
    /// The deck's picture, faded afresh each frame.
    px: Vec<u32>,
    at: (i32, i32),
    out: FadeOut,
    /// The last frame showed nothing: it can go.
    clear: bool,
}

thread_local! {
    static FADER: RefCell<Option<Fader>> = const { RefCell::new(None) };
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCHITTEST => HTTRANSPARENT as LRESULT,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

/// Fade out the deck's picture `canvas`, shown by window `from`, where that window is now.
pub(super) fn start(from: HWND, canvas: &mut Canvas) {
    close();
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { IsWindowVisible(from) } == 0 || unsafe { GetWindowRect(from, &mut r) } == 0 {
        return;
    }
    let (w, h) = (canvas.w, canvas.h);
    let px = canvas.copy_out(rect(0, 0, w, h));
    app::register_class(CLASS, Some(proc_), IDC_ARROW);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            wide(CLASS).as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            r.left,
            r.top,
            w,
            h,
            null_mut(),
            null_mut(),
            app::hinst(),
            null_mut(),
        )
    };
    if hwnd.is_null() {
        return;
    }
    crate::win::util::no_transitions(hwnd);
    let mut c = Canvas::new(w, h);
    c.copy_in(rect(0, 0, w, h), &px);
    c.present(hwnd, r.left, r.top);
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    FADER.with(|f| *f.borrow_mut() = Some(Fader { hwnd, canvas: c, px, at: (r.left, r.top), out: FadeOut::new(1.0, OUT_S), clear: false }));
}

/// One frame of the fade: whether it goes on.
pub(super) fn frame(dt: f32) -> bool {
    let gone = FADER.with(|f| {
        let mut f = f.borrow_mut();
        let fd = f.as_mut()?;
        if fd.clear {
            return Some(true);
        }
        let a = fd.out.step(dt);
        let (w, h) = (fd.canvas.w, fd.canvas.h);
        fd.canvas.copy_in(rect(0, 0, w, h), &fd.px);
        fd.canvas.scale_alpha(a);
        fd.canvas.present(fd.hwnd, fd.at.0, fd.at.1);
        // Gone after a frame of nothing has shown, never cut from a faint one.
        fd.clear = fd.out.done();
        Some(false)
    });
    match gone {
        Some(true) => {
            close();
            false
        }
        Some(false) => true,
        None => false,
    }
}

/// Gone at once (quitting, or another fade starting).
pub(super) fn close() {
    if let Some(f) = FADER.with(|f| f.borrow_mut().take()) {
        unsafe { DestroyWindow(f.hwnd) };
    }
}
