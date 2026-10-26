// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which monitor a window opens on. The deck keeps to the display chosen in Settings
//! (see `displays`), and a note opened from it the first time goes beside it; All
//! Notes, Settings and the Undo toast go where the user is looking: the monitor of our
//! window in front, or else the one under the pointer.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromRect, MonitorFromWindow};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible};

/// The monitor in use: (work area, DPI scale).
pub fn active() -> (RECT, f32) {
    unsafe {
        let fg = GetForegroundWindow();
        if ours(fg) && IsWindowVisible(fg) != 0 && IsIconic(fg) == 0 {
            return info(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST));
        }
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        at(p)
    }
}

/// The monitor nearest screen point `p`: (work area, DPI scale).
pub fn at(p: POINT) -> (RECT, f32) {
    info(unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) })
}

/// The monitor window `h` is (mostly) on: (work area, DPI scale).
pub fn of(h: HWND) -> (RECT, f32) {
    info(unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) })
}

/// The monitor screen rect `r` is (mostly) on: (work area, DPI scale).
pub fn of_rect(r: &RECT) -> (RECT, f32) {
    info(unsafe { MonitorFromRect(r, MONITOR_DEFAULTTONEAREST) })
}

fn ours(h: HWND) -> bool {
    if h.is_null() {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, &mut pid);
        pid == GetCurrentProcessId()
    }
}

fn info(mon: HMONITOR) -> (RECT, f32) {
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (mi.rcWork, dx.max(96) as f32 / 96.0)
    }
}

/// Where a card with a soft shadow `margin` wide all round (a menu, a tooltip) can go in
/// `work`, shadow and all: so it never reaches onto the next monitor, nor over the deck at
/// the screen's edge. (A work area narrower than that keeps its middle.)
pub fn inset(work: RECT, margin: i32) -> RECT {
    let mx = margin.min((work.right - work.left) / 2).max(0);
    let my = margin.min((work.bottom - work.top) / 2).max(0);
    RECT { left: work.left + mx, top: work.top + my, right: work.right - mx, bottom: work.bottom - my }
}

/// A `w` x `h` window centred in `work`, made smaller where the monitor is (a 960-wide
/// All Notes on a small or very zoomed screen) so none of it starts off screen.
pub fn centred(work: RECT, w: i32, h: i32) -> RECT {
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let (w, h) = (w.min(ww).max(1), h.min(wh).max(1));
    let x = work.left + (ww - w) / 2;
    let y = work.top + (wh - h) / 2;
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

/// Top-left of a `w` x `h` surface centred along the bottom of `work`, `gap` above it.
pub fn bottom_centre(work: RECT, w: i32, h: i32, gap: i32) -> (i32, i32) {
    ((work.left + work.right) / 2 - w / 2, work.bottom - h - gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
        RECT { left: l, top: t, right: rr, bottom: b }
    }

    fn tuple(x: RECT) -> (i32, i32, i32, i32) {
        (x.left, x.top, x.right, x.bottom)
    }

    #[test]
    fn centred_on_a_monitor_left_of_the_primary() {
        // A second monitor at negative coordinates, its taskbar on top.
        let work = r(-1920, 40, 0, 1080);
        assert_eq!(tuple(centred(work, 960, 620)), (-1440, 250, -480, 870));
    }

    #[test]
    fn centred_shrinks_to_a_small_work_area() {
        let work = r(1920, 0, 2944, 728);
        assert_eq!(tuple(centred(work, 1440, 930)), (1920, 0, 2944, 728));
        // One side too big, the other fits.
        assert_eq!(tuple(centred(work, 600, 930)), (2132, 0, 2732, 728));
    }

    #[test]
    fn a_shadowed_card_keeps_its_shadow_on_its_monitor() {
        // The monitor left of the primary, and the primary beside it.
        assert_eq!(tuple(inset(r(-1920, 0, 0, 1040), 22)), (-1898, 22, -22, 1018));
        assert_eq!(tuple(inset(r(0, 40, 2560, 1080), 26)), (26, 66, 2534, 1054));
        // Too small to take the shadow off both sides: its middle.
        assert_eq!(tuple(inset(r(0, 0, 30, 30), 26)), (15, 15, 15, 15));
    }

    #[test]
    fn toast_sits_at_the_bottom_middle_of_its_monitor() {
        let work = r(2560, 0, 5120, 1400);
        assert_eq!(bottom_centre(work, 400, 100, 12), (3640, 1288));
    }
}
