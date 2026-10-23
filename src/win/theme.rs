// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows' side of the design system: whether the system is dark and animating, dark
//! menus, and the title bar, corners and border DWM draws around our windows. The colours
//! and tokens themselves are portable (`common::theme`, re-exported here).

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

pub use crate::common::theme::*;
use crate::win::util::wide;

/// Honour Windows' "Animation effects" setting (Settings › Accessibility › Visual effects).
pub fn animations_enabled() -> bool {
    let mut on: i32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, &mut on as *mut i32 as _, 0) };
    on != 0
}

pub fn system_is_dark() -> bool {
    // Developer override for testing both appearances: PEEKABOO_THEME=light|dark.
    match std::env::var("PEEKABOO_THEME").as_deref() {
        Ok("light") => return false,
        Ok("dark") => return true,
        _ => {}
    }
    unsafe {
        let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
        let val = wide("AppsUseLightTheme");
        let mut v: u32 = 1;
        let mut sz = 4u32;
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut v as *mut u32 as _,
            &mut sz,
        );
        r == 0 && v == 0
    }
}

/// Let popup menus follow the system theme (undocumented uxtheme ordinals, harmless if absent).
pub fn allow_dark_menus() {
    unsafe {
        let ux = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if ux.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(ux, 135usize as *const u8) {
            let set_preferred_app_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_preferred_app_mode(1); // AllowDark
        }
        if let Some(f) = GetProcAddress(ux, 136usize as *const u8) {
            let flush_menu_themes: unsafe extern "system" fn() = std::mem::transmute(f);
            flush_menu_themes();
        }
    }
}

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
const DWMWA_BORDER_COLOR: u32 = 34;

pub fn set_dark_titlebar(hwnd: HWND, dark: bool) {
    let v: i32 = dark as i32;
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const i32 as _, 4);
    }
}

pub fn set_round_corners(hwnd: HWND, small: bool) {
    let v: i32 = if small { 3 } else { 2 };
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &v as *const i32 as _, 4);
    }
}

pub fn set_border_color(hwnd: HWND, rgb: u32) {
    let v = crate::win::util::cref(rgb);
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &v as *const u32 as _, 4);
    }
}
