// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Emoji variation selectors (U+FE0F after ❤ ☀ ✌ and many more, U+FE0E) kept out of
//! RichEdit. Once one is in a RichEdit, the control stops drawing every bullet and number
//! in the document, and stays that way even after it's deleted. So inside the editor each
//! is a private-use marker, formatted hidden (no width, nothing drawn); everything that
//! leaves the editor (text, Markdown, search, export, MCP) has the real selector back.

use super::*;

pub const VS16: u16 = 0xFE0F;
pub const VS15: u16 = 0xFE0E;
const VS16_MARK: u16 = 0xF8FE;
const VS15_MARK: u16 = 0xF8FD;
const CFM_HIDDEN: u32 = 0x0100;

fn mark(c: u16) -> u16 {
    match c {
        VS16 => VS16_MARK,
        VS15 => VS15_MARK,
        c => c,
    }
}

/// The marker standing in for selector `c` inside the editor; None for other characters.
pub fn marker_for(c: u16) -> Option<u16> {
    (c == VS16 || c == VS15).then(|| mark(c))
}

/// The real selector for a marker (anything else as it is).
pub fn unmark(c: u16) -> u16 {
    match c {
        VS16_MARK => VS16,
        VS15_MARK => VS15,
        c => c,
    }
}

/// Editor text as it really reads.
pub fn unmark_str(units: &[u16]) -> String {
    let real: Vec<u16> = units.iter().map(|&c| unmark(c)).collect();
    String::from_utf16_lossy(&real)
}

/// Text on its way into the editor.
pub fn to_marks(s: &str) -> String {
    if !s.contains(['\u{FE0F}', '\u{FE0E}']) {
        return s.to_string();
    }
    let units: Vec<u16> = s.encode_utf16().map(mark).collect();
    String::from_utf16_lossy(&units)
}

/// RTF on its way into the editor (RichEdit writes them as \u-497? and \u-498?).
pub fn rtf_to_marks(rtf: &str) -> String {
    if !rtf.contains("\\u-497") && !rtf.contains("\\u-498") && !rtf.contains("\\u65039") && !rtf.contains("\\u65038") {
        return rtf.to_string();
    }
    rtf.replace("\\u-497?", "\\u-1794?").replace("\\u65039?", "\\u-1794?").replace("\\u-498?", "\\u-1795?").replace("\\u65038?", "\\u-1795?")
}

/// Whether real selectors got in (from the emoji panel or an input method, which put
/// text straight into the control).
pub fn has_selectors(t: &[u16]) -> bool {
    t.iter().any(|&c| c == VS16 || c == VS15)
}

/// Format every marker hidden, leaving the selection where it was.
pub fn hide_marks(h: HWND) {
    let t = text(h);
    if !t.iter().any(|&c| c == VS16_MARK || c == VS15_MARK) {
        return;
    }
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    for (i, _) in t.iter().enumerate().filter(|&(_, &c)| c == VS16_MARK || c == VS15_MARK) {
        set_sel(h, i as i32, i as i32 + 1);
        let mut cf = CharFormat2::new();
        cf.mask = CFM_HIDDEN;
        cf.effects = CFM_HIDDEN;
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
    }
    set_sel(h, a, b);
    send(h, EM_HIDESELECTION, 0, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = "love \u{2764}\u{FE0F} and \u{263A}\u{FE0E}";
        let m = to_marks(s);
        assert!(!m.contains('\u{FE0F}') && !m.contains('\u{FE0E}'));
        let units: Vec<u16> = m.encode_utf16().collect();
        assert_eq!(unmark_str(&units), s);
        assert_eq!(m.encode_utf16().count(), s.encode_utf16().count());
        assert_eq!(rtf_to_marks(r"xဈ4?\u-497? y"), r"xဈ4?\u-1794? y");
    }
}
