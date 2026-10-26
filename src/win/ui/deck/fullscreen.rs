// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a full-screen window covers the deck's display. The shell's ABN_FULLSCREENAPP
//! only says that some window, on some monitor, went full screen or stopped being so; the
//! deck looks for itself, at each one and when the displays change, so a game on the
//! other monitor never brings it back over a full-screen video on its own.

use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// A window as far as covering a display goes.
#[derive(Clone, Copy)]
pub(super) struct Shown {
    pub rect: RECT,
    /// Always on top: a small one (a video player's mini view, a pinned note) may float
    /// over a full-screen window without it ending.
    pub topmost: bool,
    /// Maximized with a title bar: an app window that fills the screen, not a full-screen one.
    pub framed: bool,
}

fn contains(outer: &RECT, inner: &RECT) -> bool {
    outer.left <= inner.left && outer.top <= inner.top && outer.right >= inner.right && outer.bottom >= inner.bottom
}

fn meets(a: &RECT, b: &RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// Going down the windows from the top (`wins`, in z-order): whether the first one on
/// monitor `mon` covers all of it. Always-on-top windows that don't cover it are passed
/// over; the first ordinary one decides.
pub(super) fn covered(wins: &[Shown], mon: &RECT) -> bool {
    for w in wins.iter().filter(|w| meets(&w.rect, mon)) {
        if !w.framed && contains(&w.rect, mon) {
            return true;
        }
        if !w.topmost {
            return false;
        }
    }
    false
}

/// The other apps' windows on screen, top first: shown, not minimized, not cloaked (on
/// another virtual desktop, or a hidden Store app), not click-through overlays, and not
/// the desktop itself.
fn windows() -> Vec<Shown> {
    unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> i32 {
        unsafe {
            let out = &mut *(lp as *mut Vec<Shown>);
            if IsWindowVisible(h) == 0 || IsIconic(h) != 0 {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            if pid == GetCurrentProcessId() {
                return 1;
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as *mut _, 4) == 0 && cloaked != 0 {
                return 1;
            }
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT != 0 {
                return 1;
            }
            let mut class = [0u16; 16];
            let n = GetClassNameW(h, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if class == "Progman" || class == "WorkerW" {
                return 1;
            }
            let mut rect: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut rect);
            if rect.right <= rect.left || rect.bottom <= rect.top {
                return 1;
            }
            let style = GetWindowLongW(h, GWL_STYLE) as u32;
            out.push(Shown { rect, topmost: ex & WS_EX_TOPMOST != 0, framed: IsZoomed(h) != 0 && style & WS_CAPTION == WS_CAPTION });
            1
        }
    }
    let mut out: Vec<Shown> = Vec::new();
    unsafe { EnumWindows(Some(each), &mut out as *mut Vec<Shown> as LPARAM) };
    out
}

/// Whether a full-screen window covers the display whose work area is `work`.
pub(super) fn on(work: &RECT) -> bool {
    let mon = unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromRect(work, MONITOR_DEFAULTTONEAREST), &mut mi);
        mi.rcMonitor
    };
    covered(&windows(), &mon)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
    const OTHER: RECT = RECT { left: 1920, top: 0, right: 4480, bottom: 1440 };

    fn win(rect: RECT) -> Shown {
        Shown { rect, topmost: false, framed: false }
    }

    #[test]
    fn a_full_screen_window_on_the_display_covers_it() {
        assert!(covered(&[win(MON)], &MON));
        // Exclusive full screen often reaches a little past the edges.
        assert!(covered(&[win(RECT { left: -1, top: -1, right: 1921, bottom: 1081 })], &MON));
        assert!(!covered(&[], &MON));
    }

    #[test]
    fn one_on_the_other_display_doesnt() {
        // A game full screen on the other monitor, an ordinary window on the deck's.
        let small = win(RECT { left: 100, top: 100, right: 900, bottom: 700 });
        assert!(!covered(&[win(OTHER), small], &MON));
        assert!(covered(&[win(OTHER), small], &OTHER));
        // Video full screen on the deck's monitor stays covered whatever the other does.
        assert!(covered(&[win(OTHER), win(MON)], &MON));
    }

    #[test]
    fn the_window_in_front_on_the_display_decides() {
        // A window in front of a full-screen one (it's no longer what's being watched).
        let small = win(RECT { left: 100, top: 100, right: 900, bottom: 700 });
        assert!(!covered(&[small, win(MON)], &MON));
        // An always-on-top mini player over it doesn't end it.
        let pip = Shown { topmost: true, ..small };
        assert!(covered(&[pip, win(MON)], &MON));
    }

    #[test]
    fn a_maximized_window_isnt_full_screen() {
        // With the taskbar hidden, a maximized window's frame reaches past the display.
        let max = Shown { framed: true, ..win(RECT { left: -8, top: -8, right: 1928, bottom: 1088 }) };
        assert!(!covered(&[max], &MON));
    }
}
