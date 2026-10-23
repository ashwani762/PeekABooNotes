// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Named links ("our roadmap" pointing at https://...), through RichEdit's text object
//! model: the URL lives in the character format, so the RTF we store keeps it.

use windows::Win32::UI::Controls::RichEdit::{ITextDocument2, ITextRange2};
use windows::core::{BSTR, IUnknown, Interface};

use super::*;

const EM_GETOLEINTERFACE: u32 = WM_USER + 60;

fn doc(h: HWND) -> Option<ITextDocument2> {
    let mut p: *mut core::ffi::c_void = null_mut();
    send(h, EM_GETOLEINTERFACE, 0, &mut p as *mut _ as isize);
    if p.is_null() {
        return None;
    }
    // EM_GETOLEINTERFACE hands over a reference: `from_raw` takes it.
    let unk = unsafe { IUnknown::from_raw(p) };
    unk.cast().ok()
}

fn range(h: HWND, a: i32, b: i32) -> Option<ITextRange2> {
    unsafe { doc(h)?.Range2(b, a).ok() }
}

/// Make `a..b` a link to `url`.
pub fn set_link(h: HWND, a: i32, b: i32, url: &str) -> bool {
    let Some(r) = range(h, a, b) else { return false };
    unsafe { r.SetURL(&BSTR::from(format!("\"{url}\""))).is_ok() }
}

/// Plain text again.
pub fn remove_link(h: HWND, a: i32, b: i32) {
    if let Some(r) = range(h, a, b) {
        unsafe {
            let _ = r.SetURL(&BSTR::new());
        }
    }
}

fn is_link(h: HWND, cp: i32) -> bool {
    range(h, cp, cp + 1).and_then(|r| unsafe { r.GetFont2().ok()?.GetLinkType().ok() }).is_some_and(|t| t != 0)
}

/// The link around char `cp` (or just before it, for a caret at a link's end):
/// its extent and address.
pub fn link_at(h: HWND, cp: i32) -> Option<(i32, i32, String)> {
    let len = text(h).len() as i32;
    let cp = if cp < len && is_link(h, cp) {
        cp
    } else if cp > 0 && is_link(h, cp - 1) {
        cp - 1
    } else {
        return None;
    };
    let mut a = cp;
    while a > 0 && cp - a < 2048 && is_link(h, a - 1) {
        a -= 1;
    }
    let mut b = cp + 1;
    while b < len && b - cp < 2048 && is_link(h, b) {
        b += 1;
    }
    let url = url_of(h, a, b);
    Some((a, b, url))
}

/// The address of the link covering `a..b`: its own URL when it was given a name,
/// else its text (an address typed out, which RichEdit spots by itself).
pub fn url_of(h: HWND, a: i32, b: i32) -> String {
    let named = range(h, a, b).and_then(|r| unsafe { r.GetURL().ok() }).map(|s| s.to_string()).unwrap_or_default();
    let named = named.trim().trim_matches('"').to_string();
    if !named.is_empty() {
        return named;
    }
    String::from_utf16_lossy(&range_text(h, a, b)).trim().to_string()
}

const FIELD: &str = "HYPERLINK \"";

/// The hidden `HYPERLINK "url"` instructions in `t[a..b]`: (start, where the name
/// begins, url).
pub fn instructions(t: &[u16], a: usize, b: usize) -> Vec<(usize, usize, String)> {
    let key: Vec<u16> = FIELD.encode_utf16().collect();
    let quote = b'"' as u16;
    let mut out = Vec::new();
    let mut i = a;
    while i + key.len() <= b {
        if t[i..i + key.len()] == key[..]
            && let Some(q) = t[i + key.len()..b].iter().position(|&c| c == quote)
        {
            let url = String::from_utf16_lossy(&t[i + key.len()..i + key.len() + q]);
            let name = i + key.len() + q + 1;
            out.push((i, name, url));
            i = name;
            continue;
        }
        i += 1;
    }
    out
}

/// `t[a..b]` as it reads, without the hidden link instructions.
pub fn visible(t: &[u16], a: usize, b: usize) -> String {
    let mut out = Vec::with_capacity(b.saturating_sub(a));
    let mut i = a;
    for (at, name, _) in instructions(t, a, b) {
        out.extend_from_slice(&t[i..at]);
        i = name;
    }
    out.extend_from_slice(&t[i.min(b)..b]);
    out.retain(|&c| c != ITEM_LINK_NAME as u16);
    // A chip's paperclip only reads in the icon font: elsewhere, just the name.
    let s = String::from_utf16_lossy(&out);
    if s.contains(CLIP) { s.replace(&format!("{CLIP} "), "").replace(CLIP, "") } else { s }
}

/// Where the shown name of the link `a..b` (as `link_at` gives it) starts.
pub fn name_start(t: &[u16], a: i32, b: i32) -> i32 {
    instructions(t, a as usize, b as usize).first().map_or(a, |l| l.1 as i32)
}

/// `[name](url)` links within a line of Markdown: the line without their syntax, and
/// each link's (start, end, url) there, in UTF-16 units.
pub(super) fn md_links(s: &str) -> (String, Vec<(i32, i32, String)>) {
    let mut out = String::new();
    let mut links = Vec::new();
    let mut rest = s;
    let mut n = 0i32;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let parsed = after.find("](").and_then(|close| {
            let name = &after[..close];
            let tail = &after[close + 2..];
            let end = tail.find(')')?;
            let url = &tail[..end];
            (!name.is_empty() && !name.contains('[') && !url.is_empty() && !url.contains(char::is_whitespace)).then_some((name, url, open + 1 + close + 2 + end + 1))
        });
        match parsed {
            Some((name, url, used)) => {
                let pre = &rest[..open];
                let pre = pre.strip_suffix('!').unwrap_or(pre);
                out.push_str(pre);
                n += pre.encode_utf16().count() as i32;
                let len = name.encode_utf16().count() as i32;
                out.push_str(name);
                links.push((n, n + len, url.to_string()));
                n += len;
                rest = &rest[used..];
            }
            None => {
                out.push_str(&rest[..open + 1]);
                n += rest[..open + 1].encode_utf16().count() as i32;
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    (out, links)
}

/// An attachment sits on a line of its own: its picture, or a paperclip and its name,
/// then its link, named with a zero-width space. The link is invisible because RichEdit
/// underlines every link, and a line under a picture or a chip looks broken.
pub const ITEM_LINK_NAME: char = '\u{200B}';

/// The paperclip in front of a file's name (in the icon font, which has it at this
/// private-use code point; text elsewhere leaves it out).
pub const CLIP: char = '\u{E723}';

/// Show the paperclips in `a..b` in the icon font `face`.
pub fn style_clips(h: HWND, a: i32, b: i32, face: &str) {
    let t = text(h);
    for i in (a.max(0) as usize..(b as usize).min(t.len())).filter(|&i| t[i] == CLIP as u16) {
        set_sel(h, i as i32, i as i32 + 1);
        let mut cf = CharFormat2::new();
        cf.mask = CFM_FACE;
        set_face(&mut cf, face);
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
    }
}

/// The paragraph around `cp`: (start, end).
fn para(t: &[u16], cp: usize) -> (usize, usize) {
    let cp = cp.min(t.len());
    let start = t[..cp].iter().rposition(|&c| c == 13).map_or(0, |i| i + 1);
    let end = t[cp..].iter().position(|&c| c == 13).map_or(t.len(), |i| cp + i);
    (start, end)
}

/// When `cp` is on an attachment's line, before its link: that link.
pub fn item_link(h: HWND, t: &[u16], cp: usize) -> Option<(i32, i32, String)> {
    let (start, end) = para(t, cp);
    let last = instructions(t, start, end).pop()?;
    let ends_line = last.1 + 1 == end && t.get(last.1) == Some(&(ITEM_LINK_NAME as u16));
    (ends_line && cp < last.0).then(|| link_at(h, last.1 as i32)).flatten()
}

/// What an attachment's line shows before its link ("📎 plan.pdf", or a space for a
/// picture): (start, end).
pub fn item_shown(t: &[u16], cp: usize) -> (usize, usize) {
    let (start, end) = para(t, cp);
    let link = instructions(t, start, end).pop().map_or(end, |l| l.0);
    (start, link)
}

/// Lines holding just a picture: single spacing, since the text's 1.2x would add a fifth
/// of the picture's height under it. Moves the selection.
pub fn fit_pictures(h: HWND) {
    let t = text(h);
    let mut start = 0usize;
    while start < t.len() {
        let end = t[start..].iter().position(|&c| c == 13).map_or(t.len(), |i| start + i);
        if end > start && item_link(h, &t, start).is_some() && visible(&t, start, end).trim().is_empty() {
            set_sel(h, start as i32, start as i32);
            let mut pf = ParaFormat2::new();
            pf.mask = PFM_LINESPACING;
            pf.line_spacing_rule = 0;
            send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
        }
        start = end + 1;
    }
}

/// Whether `s` reads as a web address (or mail link) on its own.
pub fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://", "file://"].iter().any(|p| lower.starts_with(p)) || (lower.starts_with("www.") && lower.len() > 5)
}

/// An address as typed, made something a browser opens: "example.com" -> "https://example.com".
pub fn normalize_url(s: &str) -> String {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("mailto:") {
        s.to_string()
    } else if s.contains('@') && !s.contains('/') {
        format!("mailto:{s}")
    } else {
        format!("https://{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};

    fn editor() -> HWND {
        unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 300, 200, null_mut(), null_mut(), null_mut(), null_mut())
        }
    }

    #[test]
    fn named_links_keep_text_and_url() {
        let h = editor();
        assert!(!h.is_null());
        setup(h);
        insert_text(h, "See our roadmap today");
        assert!(set_link(h, 8, 15, "https://example.com/plan"));
        // RichEdit keeps the address as hidden text before the name; what we show,
        // store and search is the text without it.
        let t = text(h);
        assert_eq!(visible(&t, 0, t.len()), "See our roadmap today");
        let found = instructions(&t, 0, t.len());
        assert_eq!(found.len(), 1);
        let (at, name, url) = found[0].clone();
        assert_eq!((at, url.as_str()), (8, "https://example.com/plan"));
        let end = name as i32 + 7;
        assert_eq!(link_at(h, name as i32 + 2), Some((8, end, "https://example.com/plan".to_string())));
        assert_eq!(link_at(h, end).map(|l| l.1), Some(end));
        assert_eq!(link_at(h, 3), None);
        remove_link(h, 8, end);
        let t2 = text(h);
        assert_eq!((String::from_utf16_lossy(&t2).as_str(), link_at(h, 9)), ("See our roadmap today", None));
        assert!(set_link(h, 8, 15, "https://example.com/plan"));
        assert_eq!(to_markdown(h, true), "# See our [roadmap](https://example.com/plan) today");
        // The RTF keeps it.
        let rtf = get_rtf(h);
        set_rtf(h, &rtf);
        assert_eq!(link_at(h, name as i32 + 2).map(|l| l.2), Some("https://example.com/plan".to_string()));
        // And Markdown brings it back.
        load_markdown(h, "Plan\nRead **the [design doc](https://x.org/d)** and [notes](https://x.org/n).");
        assert_eq!(to_markdown(h, false), "Plan\nRead the design doc and notes.");
        assert_eq!(to_markdown(h, true), "# Plan\nRead **the [design doc](https://x.org/d)** and [notes](https://x.org/n).");
        unsafe { DestroyWindow(h) };
    }

    #[test]
    fn addresses() {
        assert!(looks_like_url("https://x.org/a?b=1"));
        assert!(looks_like_url("www.example.com"));
        assert!(!looks_like_url("hello world"));
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("me@x.org"), "mailto:me@x.org");
        assert_eq!(normalize_url("http://a.b"), "http://a.b");
    }
}
