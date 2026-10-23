// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Win32 helpers shared by the Windows modules: UTF-16 strings, COLORREFs, message
//! parameters, keys and RECTs. The portable helpers are re-exported from `common::util`.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

pub use crate::common::util::*;

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
