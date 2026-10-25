// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The design system: paper colours, chrome materials, type, shape and motion tokens.
//!
//! Notes are paper: they keep their pastel colour in both themes
//! (slightly dimmed in dark mode); only the chrome around them follows light/dark.

use crate::common::util::mix;

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

/// Colour `i`'s name, in the interface language (swatch tooltips).
pub fn color_name(i: u8) -> &'static str {
    const KEYS: [&str; 12] = [
        "color_yellow", "color_peach", "color_pink", "color_lilac", "color_blue", "color_mint",
        "color_lime", "color_graphite", "color_coral", "color_sky", "color_teal", "color_sand",
    ];
    crate::common::i18n::get(KEYS[i as usize % KEYS.len()])
}

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
/// Secondary text on paper, as a blend factor toward the paper colour: as light as it
/// goes while still reading at 4.5:1 on every paper in both themes (dimmed coral is the
/// darkest paper, and sets it).
pub fn paper_subtext(i: u8, dark: bool) -> u32 {
    mix(PAPER_TEXT, paper(i, dark), 0.28)
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

/// WCAG 2 contrast ratio between two 0xRRGGBB colours (1 to 21).
#[cfg(test)]
pub fn contrast(a: u32, b: u32) -> f32 {
    let lum = |c: u32| {
        let f = |v: u32| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f((c >> 16) & 0xFF) + 0.7152 * f((c >> 8) & 0xFF) + 0.0722 * f(c & 0xFF)
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_matches_wcag() {
        assert!((contrast(0x000000, 0xFFFFFF) - 21.0).abs() < 0.01);
        assert!((contrast(0x777777, 0xFFFFFF) - 4.48).abs() < 0.01);
    }

    #[test]
    fn preview_text_reads_on_every_paper() {
        for i in 0..COLORS.len() as u8 {
            for dark in [false, true] {
                let p = paper(i, dark);
                let c = contrast(paper_subtext(i, dark), p);
                assert!(c >= 4.5, "{} paper (dark: {dark}): secondary text at {c:.2}:1", COLORS[i as usize].name);
                assert!(contrast(PAPER_TEXT, p) >= 7.0);
            }
        }
    }
}
