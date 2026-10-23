// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The link popover (Ctrl+K): one field for the address, under the text it links.
//! Enter applies, Esc or clicking elsewhere cancels; an emptied field removes the link.

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETSEL, MARGINS};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape, gdi_font};
use crate::win::theme;
use crate::win::util::{cref, rect, wide};

const CLASS: &str = "PeekABooLinkBox";

struct Box_ {
    hwnd: HWND,
    edit: HWND,
    s: f32,
    font: HFONT,
    brush: HBRUSH,
    icons: Font,
    key: Font,
    /// Some(text) once Enter was pressed; Some(None) when cancelled.
    result: Option<Option<String>>,
}

thread_local! {
    static BOX: RefCell<Option<Box_>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Box_) -> R) -> Option<R> {
    BOX.with(|b| b.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(f)))
}

/// (surface, text, secondary), the menu's palette.
fn pal() -> (u32, u32, u32) {
    if app::dark() { (0x2C2C2E, 0xF2F2F7, 0x98989D) } else { (0xF7F7F9, 0x1D1D1F, 0x86868B) }
}

const W: f32 = 380.0;
const H: f32 = 44.0;

/// Ask for an address, showing the box with its top-left near `at` (screen), over
/// `owner`. Returns the text typed (empty to remove), or None when cancelled.
pub fn ask(owner: HWND, at: POINT, current: &str) -> Option<String> {
    if with(|_| ()).is_some() {
        return None;
    }
    let (s, work) = unsafe {
        let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (dx.max(96) as f32 / 96.0, mi.rcWork)
    };
    let mut owner_rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner, &mut owner_rc) };
    let narrow = owner_rc.right - owner_rc.left - (16.0 * s) as i32;
    let (w, h) = (((W * s) as i32).min(narrow.max((240.0 * s) as i32)), (H * s) as i32);
    // Within the note where it fits, and always on the screen.
    let x = at.x.min(owner_rc.right - w - (8.0 * s) as i32).max(owner_rc.left + (8.0 * s) as i32);
    let x = x.clamp(work.left + 8, (work.right - w - 8).max(work.left));
    // Below the text, or above it when there's no room.
    let y = if at.y + h > work.bottom - 8 { at.y - h - (30.0 * s) as i32 } else { at.y };
    let y = y.clamp(work.top + 8, (work.bottom - h - 8).max(work.top));
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let hwnd = unsafe {
        CreateWindowExW(WS_EX_TOOLWINDOW, wide(CLASS).as_ptr(), wide("Link").as_ptr(), WS_POPUP | WS_CAPTION, x, y, w, h, owner, null_mut(), app::hinst(), null_mut())
    };
    if hwnd.is_null() {
        return None;
    }
    let (surface, text, _) = pal();
    let font = gdi_font((14.0 * s) as i32, FW_NORMAL as i32, theme::FACE_TEXT);
    let edit = unsafe {
        CreateWindowExW(0, wide("EDIT").as_ptr(), wide(current).as_ptr(), WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32, 0, 0, 10, 10, hwnd, null_mut(), app::hinst(), null_mut())
    };
    unsafe {
        SendMessageW(edit, WM_SETFONT, font as usize, 0);
        SendMessageW(edit, EM_SETCUEBANNER, 1, wide(t!("link_placeholder")).as_ptr() as isize);
        SetWindowSubclass(edit, Some(edit_proc), 1, 0);
    }
    BOX.with(|b| {
        *b.borrow_mut() = Some(Box_ {
            hwnd,
            edit,
            s,
            font,
            brush: unsafe { CreateSolidBrush(cref(surface)) },
            icons: Font::new(15.0 * s, FW_NORMAL, crate::win::ui::note::icon_face()),
            key: Font::new(11.0 * s, FW_SEMIBOLD, theme::FACE_SMALL),
            result: None,
        })
    });
    let _ = text;
    unsafe {
        // A DWM frame gives the system shadow and rounded corners.
        let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        DwmExtendFrameIntoClientArea(hwnd, &m);
        let off: i32 = 1;
        DwmSetWindowAttribute(hwnd, 3 /* DWMWA_TRANSITIONS_FORCEDISABLED */, &off as *const i32 as _, 4);
        theme::set_dark_titlebar(hwnd, app::dark());
        theme::set_round_corners(hwnd, false);
        SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        layout(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(edit);
        SendMessageW(edit, EM_SETSEL, 0, -1);
    }
    loop {
        if with(|b| b.result.is_some()).unwrap_or(true) {
            break;
        }
        if !app::pump_once() {
            break;
        }
    }
    let b = BOX.with(|b| b.borrow_mut().take())?;
    unsafe {
        RemoveWindowSubclass(b.edit, Some(edit_proc), 1);
        DestroyWindow(b.hwnd);
        DeleteObject(b.font);
        DeleteObject(b.brush);
        SetForegroundWindow(owner);
    }
    b.result.flatten()
}

fn layout(h: HWND) {
    with(|b| unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let s = b.s;
        let x = (42.0 * s) as i32;
        let eh = (20.0 * s) as i32;
        let right = rc.right - (64.0 * s) as i32;
        MoveWindow(b.edit, x, (rc.bottom - eh) / 2, (right - x).max(10), eh, 1);
    });
}

fn paint(h: HWND) {
    let (surface, _, sub) = pal();
    let dark = app::dark();
    with(|b| unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(h, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let s = b.s;
        let mut c = Canvas::new(rc.right.max(1), rc.bottom.max(1));
        c.clear(0xFF00_0000 | surface);
        c.begin_batch();
        c.text_ct("\u{E71B}", &b.icons, rect((12.0 * s) as i32, 0, (24.0 * s) as i32, rc.bottom), None, sub, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        // The Enter keycap, as in tooltips: what applies it.
        let label = t!("key_enter");
        let kw = b.key.measure(label).0 + (11.0 * s) as i32;
        let (kh, ky) = (19.0 * s, (rc.bottom as f32 - 19.0 * s) / 2.0);
        let kx = rc.right as f32 - 12.0 * s - kw as f32;
        let edge = if dark { 0xFFFFFF } else { 0x000000 };
        let cap = Shape::new(kx, ky, kw as f32, kh, 5.0 * s);
        c.shape(&cap, edge, if dark { 0.10 } else { 0.06 }, 1.0, 0.0);
        c.shape(&cap, edge, if dark { 0.10 } else { 0.08 }, 1.0, 1.0);
        let ink = if dark { sub } else { 0x6E6E73 };
        c.text_ct(label, &b.key, rect(kx as i32, ky.round() as i32, kw, kh.round() as i32), None, ink, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        c.flush();
        c.make_opaque();
        c.blit(dc, 0, 0);
        EndPaint(h, &ps);
    });
}

fn finish(r: Option<String>) {
    with(|b| {
        if b.result.is_none() {
            b.result = Some(r);
        }
    });
}

unsafe extern "system" fn edit_proc(e: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN if wp as u16 == VK_RETURN => {
                let n = GetWindowTextLengthW(e);
                let mut buf = vec![0u16; n as usize + 1];
                GetWindowTextW(e, buf.as_mut_ptr(), n + 1);
                finish(Some(String::from_utf16_lossy(&buf[..n as usize])));
                return 0;
            }
            WM_KEYDOWN if wp as u16 == VK_ESCAPE => {
                finish(None);
                return 0;
            }
            // No beep for the Enter and Esc characters.
            WM_CHAR if wp == 0x0D || wp == 0x1B => return 0,
            _ => {}
        }
        DefSubclassProc(e, msg, wp, lp)
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCALCSIZE if wp != 0 => return 0,
            WM_NCACTIVATE => return DefWindowProcW(h, msg, wp, -1),
            WM_NCHITTEST => return HTCLIENT as LRESULT,
            WM_PAINT => {
                paint(h);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_CTLCOLOREDIT => {
                let (surface, text, _) = pal();
                let dc = wp as HDC;
                SetTextColor(dc, cref(text));
                SetBkColor(dc, cref(surface));
                return with(|b| b.brush as LRESULT).unwrap_or(0);
            }
            WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => finish(None),
            WM_LBUTTONDOWN => {
                if let Some(e) = with(|b| b.edit) {
                    SetFocus(e);
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
