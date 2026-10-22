//! The design system: paper colours, chrome materials, type, shape and motion tokens.
//!
//! Notes are paper: they keep their pastel colour in both themes
//! (slightly dimmed in dark mode); only the chrome around them follows light/dark.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};

use crate::platform::util::{mix, wide};

pub struct NoteColor {
    pub name: &'static str,
    /// The paper.
    pub paper: u32,
    /// Saturated mark colour: capsules, swatches, the checkbox tint.
    pub accent: u32,
    /// Dark ink of the same hue: tab labels, quiet icons.
    pub ink: u32,
}

/// Stored by index, so new colours are only ever appended; `ORDER` is the display order.
pub const COLORS: [NoteColor; 12] = [
    NoteColor { name: "Yellow", paper: 0xFBE48F, accent: 0xF2C230, ink: 0x6E5200 },
    NoteColor { name: "Peach", paper: 0xF9D4BB, accent: 0xF29A5C, ink: 0x7F3E12 },
    NoteColor { name: "Pink", paper: 0xF7CBDB, accent: 0xEB76A3, ink: 0x83294E },
    NoteColor { name: "Lilac", paper: 0xDAD1F7, accent: 0xA58CF2, ink: 0x45358A },
    NoteColor { name: "Blue", paper: 0xBCD8F8, accent: 0x5BA6EE, ink: 0x1C4A80 },
    NoteColor { name: "Mint", paper: 0xBDE6D4, accent: 0x45C196, ink: 0x1B654B },
    NoteColor { name: "Lime", paper: 0xDDEEB6, accent: 0x93C944, ink: 0x45650F },
    NoteColor { name: "Graphite", paper: 0xE3E1DC, accent: 0xA8A49C, ink: 0x524E47 },
    NoteColor { name: "Coral", paper: 0xF8C7BF, accent: 0xEE6E5E, ink: 0x8A2A1F },
    NoteColor { name: "Sky", paper: 0xC4E6F4, accent: 0x4CB6DD, ink: 0x165A73 },
    NoteColor { name: "Teal", paper: 0xB7E2DC, accent: 0x2FB0A3, ink: 0x135A53 },
    NoteColor { name: "Sand", paper: 0xEDE2CB, accent: 0xC9A96E, ink: 0x5E4B26 },
];

/// Colours in hue order, for swatches, menus and Ctrl+. cycling.
pub const ORDER: [u8; 12] = [0, 11, 1, 8, 2, 3, 4, 9, 10, 5, 6, 7];

pub fn color(i: u8) -> &'static NoteColor {
    &COLORS[i as usize % COLORS.len()]
}

/// The colour after `i` in display order.
pub fn next_color(i: u8) -> u8 {
    let pos = ORDER.iter().position(|&c| c == i % COLORS.len() as u8).unwrap_or(0);
    ORDER[(pos + 1) % ORDER.len()]
}

/// The note paper; dimmed a touch in dark mode so it doesn't glare.
pub fn paper(i: u8, dark: bool) -> u32 {
    let p = color(i).paper;
    if dark { mix(p, 0x000000, 0.12) } else { p }
}

/// Body text on paper (both themes: paper is always light).
pub const PAPER_TEXT: u32 = 0x1D1D1F;
/// Secondary text on paper, as a blend factor toward the paper colour.
pub fn paper_subtext(i: u8, dark: bool) -> u32 {
    mix(PAPER_TEXT, paper(i, dark), 0.36)
}
/// Labels and quiet icons on paper.
pub fn paper_ink(i: u8, dark: bool) -> u32 {
    mix(color(i).ink, paper(i, dark), 0.22)
}

/// Translucent chrome (pill, buttons, HUDs, menus): (fill, alpha, hairline, hairline alpha).
pub struct Material {
    pub fill: u32,
    pub alpha: f32,
    pub edge: u32,
    pub edge_alpha: f32,
    pub text: u32,
    pub subtext: u32,
}

pub fn chrome(dark: bool) -> Material {
    if dark {
        Material { fill: 0x2A2A2D, alpha: 0.86, edge: 0xFFFFFF, edge_alpha: 0.10, text: 0xF2F2F4, subtext: 0xA1A1A6 }
    } else {
        Material { fill: 0xFBFBFD, alpha: 0.86, edge: 0x000000, edge_alpha: 0.09, text: 0x1D1D1F, subtext: 0x6E6E73 }
    }
}

/// Type faces (Windows' optically sized system font).
pub const FACE_DISPLAY: &str = "Segoe UI Variable Display";
/// RichEdit doesn't resolve variable-font weights, so it gets the named instance.
pub const FACE_TITLE: &str = "Segoe UI Variable Display Semib";
pub const FACE_TEXT: &str = "Segoe UI Variable Text";
pub const FACE_SMALL: &str = "Segoe UI Variable Small";

/// Spring presets as (damping ratio, response seconds).
pub mod motion {
    pub const DEFAULT: (f32, f32) = (1.0, 0.32);
    pub const SNAPPY: (f32, f32) = (1.0, 0.2);
    pub const SHINGLE: (f32, f32) = (0.82, 0.36);
    pub const MOMENTUM: (f32, f32) = (0.8, 0.3);
    pub const STAGGER_S: f32 = 0.04;
    pub const PREVIEW_DWELL_MS: u32 = 380;
    pub const COLLAPSE_GRACE_MS: u32 = 220;
}

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
    let v = crate::platform::util::cref(rgb);
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &v as *const u32 as _, 4);
    }
}
