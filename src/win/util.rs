// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Win32 helpers shared by the Windows modules: UTF-16 strings, COLORREFs, message
//! parameters, keys and RECTs. The portable helpers are re-exported from `common::util`.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

pub use crate::common::util::*;

/// Put `text` on the clipboard. False if it couldn't.
pub fn set_clipboard_text(text: &str) -> bool {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    let w = wide(text);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2);
        let mut ok = false;
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                GlobalUnlock(h);
                // The clipboard owns the memory once this succeeds.
                ok = !SetClipboardData(CF_UNICODETEXT, h as _).is_null();
            }
        }
        CloseClipboard();
        ok
    }
}

/// Whether a note's Paste has something to take: text, or files copied in Explorer.
pub fn clipboard_can_paste() -> bool {
    use windows_sys::Win32::System::DataExchange::IsClipboardFormatAvailable;
    const CF_UNICODETEXT: u32 = 13;
    const CF_HDROP: u32 = 15;
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) != 0 || IsClipboardFormatAvailable(CF_HDROP) != 0 }
}

/// The clipboard's text, if it holds any.
pub fn clipboard_text() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let mut out = None;
        if !h.is_null() {
            let p = GlobalLock(h as _) as *const u16;
            if !p.is_null() {
                let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
                GlobalUnlock(h as _);
            }
        }
        CloseClipboard();
        out
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn from_wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

/// 0xRRGGBB -> COLORREF (0x00BBGGRR)
pub const fn cref(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

pub fn x_of(lp: isize) -> i32 {
    (lp & 0xFFFF) as u16 as i16 as i32
}

pub fn y_of(lp: isize) -> i32 {
    ((lp >> 16) & 0xFFFF) as u16 as i16 as i32
}

pub fn key_down(vk: u16) -> bool {
    unsafe { (GetKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn pt_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// Not exported by windows-sys' WindowsAndMessaging module.
pub const WM_MOUSELEAVE: u32 = 0x02A3;
