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

thread_local! {
    static BULK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Make many changes in a row as one: drawing, layout, change notifications and undo
/// recording wait until they're all made. One at a time, the ten thousand lines of a long
/// list took RichEdit seconds. Nests (only the outermost call holds and releases).
pub fn bulk<R>(h: HWND, f: impl FnOnce() -> R) -> R {
    use windows::Win32::UI::Controls::RichEdit::{tomResume, tomSuspend};
    if BULK.with(|b| b.replace(b.get() + 1)) > 0 {
        let r = f();
        BULK.with(|b| b.set(b.get() - 1));
        return r;
    }
    let d = doc(h);
    let mask = send(h, EM_GETEVENTMASK, 0, 0);
    send(h, EM_SETEVENTMASK, 0, 0);
    if let Some(d) = &d {
        unsafe {
            let _ = d.Freeze();
            let _ = d.Undo(tomSuspend.0);
        }
    }
    let r = f();
    if let Some(d) = &d {
        unsafe {
            let _ = d.Undo(tomResume.0);
            let _ = d.Unfreeze();
        }
    }
    send(h, EM_SETEVENTMASK, 0, mask);
    BULK.with(|b| b.set(0));
    r
}

/// Run `f`, which moves the selection about to format text, with the view held still:
/// nothing is drawn until it's done, and the scroll position is put back, so the note
/// doesn't jump or flash. Unlike `bulk`, it stays undoable.
pub fn still<R>(h: HWND, f: impl FnOnce() -> R) -> R {
    if BULK.with(|b| b.get()) > 0 {
        return f();
    }
    let d = doc(h);
    let y = scroll_y(h);
    if let Some(d) = &d {
        unsafe {
            let _ = d.Freeze();
        }
    }
    let r = f();
    set_scroll_y(h, y);
    if let Some(d) = &d {
        unsafe {
            let _ = d.Unfreeze();
        }
    }
    r
}

/// The font size (points) of the character at `cp`, without touching the selection.
pub fn size_at(h: HWND, cp: i32) -> Option<f32> {
    range(h, cp, cp + 1).and_then(|r| unsafe { r.GetFont().ok()?.GetSize().ok() })
}

/// Whether all of `a..b` is in typeface `face` at `twips`, without touching the selection
/// (moving it would end RichEdit's current undo step, so each typed letter became one).
pub fn font_is(h: HWND, a: i32, b: i32, face: &str, twips: i32) -> bool {
    let Some(f) = range(h, a, b).and_then(|r| unsafe { r.GetFont().ok() }) else { return false };
    let size = unsafe { f.GetSize() }.unwrap_or(0.0);
    let name = unsafe { f.GetName() }.map(|n| n.to_string()).unwrap_or_default();
    (size * 20.0 - twips as f32).abs() < 1.0 && name.eq_ignore_ascii_case(face)
}

/// Make `a..b` a link to `url`.
pub fn set_link(h: HWND, a: i32, b: i32, url: &str) -> bool {
    let Some(r) = range(h, a, b) else { return false };
    unsafe { r.SetURL(&BSTR::from(format!("\"{url}\""))).is_ok() }
}

/// Every link's colour: a deep blue that reads on all twelve papers, dimmed or not.
const LINK_BLUE: u32 = 0x0645AD;

/// Links in their blue (they take their colour from the text otherwise). Kept out of the
/// undo history (a colour moves no text), so Ctrl+Z steps over it.
pub fn color_named_links(h: HWND) {
    use windows::Win32::UI::Controls::RichEdit::{tomResume, tomSuspend};
    let t = text(h);
    let links = instructions(&t, 0, t.len());
    if links.is_empty() {
        return;
    }
    let Some(d) = doc(h) else { return };
    let want = crate::win::util::cref(LINK_BLUE) as i32;
    let mut suspended = false;
    for (_, name, _) in links {
        // A picture's or file's link has a zero-width name: nothing to colour.
        if t.get(name) == Some(&(ITEM_LINK_NAME as u16)) {
            continue;
        }
        let Some((_, end, _)) = link_at(h, name as i32) else { continue };
        let Some(f) = range(h, name as i32, end).and_then(|r| unsafe { r.GetFont().ok() }) else { continue };
        if unsafe { f.GetForeColor() }.is_ok_and(|c| c != want) {
            if !suspended {
                let _ = unsafe { d.Undo(tomSuspend.0) };
                suspended = true;
            }
            let _ = unsafe { f.SetForeColor(want) };
        }
    }
    if suspended {
        let _ = unsafe { d.Undo(tomResume.0) };
    }
}

/// Addresses typed or pasted into `t[a..b]` made links, and links whose text is an address
/// pointed at that address (so editing one never leaves it going somewhere else).
/// RichEdit's own address spotting stays off: it picks those links' colour itself, and
/// drew them white on paper dimmed for dark mode.
pub fn link_addresses(h: HWND, a: usize, b: usize) {
    let t = text(h);
    let b = b.min(t.len());
    if a >= b {
        return;
    }
    // Hidden link instructions count as gaps between words.
    let mut gap = vec![false; t.len()];
    for (start, name, _) in instructions(&t, 0, t.len()) {
        gap[start..name].iter_mut().for_each(|g| *g = true);
    }
    let space = |i: usize| gap[i] || matches!(t[i], 0x20 | 0x09 | 0x0B | 0x0D | 0xA0);
    let lead = "([<\"'".encode_utf16().collect::<Vec<u16>>();
    let trail = ".,;:!?)]}>\"'".encode_utf16().collect::<Vec<u16>>();
    let mut words = Vec::new();
    let mut i = a;
    while i < b {
        if space(i) {
            i += 1;
            continue;
        }
        let (mut s, mut e) = (i, i);
        while e < b && !space(e) {
            e += 1;
        }
        i = e;
        while s < e && lead.contains(&t[s]) {
            s += 1;
        }
        while e > s && trail.contains(&t[e - 1]) {
            e -= 1;
        }
        if e > s {
            words.push((s, e));
        }
    }
    // From the end: each new link adds hidden text before its name.
    for (s, e) in words.into_iter().rev() {
        let word = String::from_utf16_lossy(&t[s..e]);
        if !looks_like_url(&word) {
            continue;
        }
        let url = normalize_url(&word);
        let (s, e) = (s as i32, e as i32);
        if is_link(h, s) {
            let Some((la, lb, cur)) = link_at(h, s) else { continue };
            let name = name_start(&t, la, lb);
            if name == s && lb == e {
                if cur != url {
                    set_link(h, s, e, &url);
                } else if word == url {
                    // Text that is exactly its address, read back from a saved note: RichEdit
                    // takes it for one of its own and paints it its blue, whatever colour it's
                    // given. Made again, it's ours.
                    let hidden = name - la;
                    remove_link(h, la, lb);
                    set_link(h, s - hidden, e - hidden, &url);
                }
            } else if name == s {
                // Typed on past the link's end: the link takes the whole address again.
                let hidden = name - la;
                remove_link(h, la, lb);
                set_link(h, s - hidden, e - hidden, &url);
            }
            continue;
        }
        set_link(h, s, e, &url);
    }
    color_named_links(h);
}

/// The word just before the caret (the one a space, Tab or Enter ends) as a link, when
/// it's an address.
pub fn link_word_before(h: HWND) {
    let t = text(h);
    let (j, _) = sel(h);
    let mut j = (j.max(0) as usize).min(t.len());
    let blank = |c: u16| matches!(c, 0x20 | 0x09 | 0x0B | 0x0D | 0xA0);
    while j > 0 && blank(t[j - 1]) {
        j -= 1;
    }
    let mut i = j;
    while i > 0 && !blank(t[i - 1]) {
        i -= 1;
    }
    if t[i..j].iter().any(|&c| c == b'.' as u16 || c == b':' as u16) {
        link_addresses(h, i, j);
    }
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
    // The whole link in one step; else letter by letter.
    let (a, b) = link_extent(h, cp).unwrap_or_else(|| {
        let mut a = cp;
        while a > 0 && cp - a < 2048 && is_link(h, a - 1) {
            a -= 1;
        }
        let mut b = cp + 1;
        while b < len && b - cp < 2048 && is_link(h, b) {
            b += 1;
        }
        (a, b)
    });
    let url = url_of(h, a, b);
    Some((a, b, url))
}

/// The link holding char `cp`, from TOM (hidden instructions included).
fn link_extent(h: HWND, cp: i32) -> Option<(i32, i32)> {
    use windows::Win32::UI::Controls::RichEdit::tomLink;
    let r = range(h, cp, cp + 1)?;
    unsafe {
        r.Expand(tomLink.0).ok()?;
        let (a, b) = (r.GetStart().ok()?, r.GetEnd().ok()?);
        (a <= cp && b > cp).then_some((a, b))
    }
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
    let s = unmark_str(&out);
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
    ["http://", "https://", "mailto:", "ftp://", "file://"].iter().any(|p| lower.starts_with(p)) || (lower.starts_with("www.") && lower.len() > 5) || email(s)
}

/// name@example.org: one @, a dotted domain after it, nothing odd around.
fn email(s: &str) -> bool {
    let Some((user, domain)) = s.split_once('@') else { return false };
    let ok = |c: char| c.is_alphanumeric() || "._%+-".contains(c);
    !user.is_empty()
        && user.chars().all(ok)
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '-')
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
    fn marks_sit_outside_spaces() {
        let h = editor();
        setup(h);
        // Round trip, underline included.
        let md = "# T\n**bold** _italic_ <u>under</u> ~~strike~~ **two words** plain";
        load_markdown(h, &md[2..]);
        assert_eq!(to_markdown(h, true), md);
        // Formatting that takes in the space after a word, as typing it does.
        load_markdown(h, "T\nbold italic end\nour roadmap here");
        let t = text(h);
        let at = |s: &str| String::from_utf16_lossy(&t).find(s).unwrap() as i32;
        set_sel(h, at("bold"), at("italic"));
        set_effect(h, CFM_BOLD, true);
        set_sel(h, at("italic"), at("end"));
        set_effect(h, CFM_ITALIC, true);
        assert!(set_link(h, at("our"), at("here"), "https://r.example"));
        assert_eq!(to_markdown(h, true), "# T\n**bold** _italic_ end\n[our roadmap](https://r.example) here");
        unsafe { DestroyWindow(h) };
    }

    #[test]
    fn addresses_become_links() {
        let h = editor();
        setup(h);
        load_markdown(h, "Trip\nsee www.example.com, and (https://example.org/a?b=1).\n[our plan](https://plan.example)");
        link_addresses(h, 0, usize::MAX);
        let t = text(h);
        let urls: Vec<String> = instructions(&t, 0, t.len()).into_iter().map(|l| l.2).collect();
        assert_eq!(urls, ["https://www.example.com", "https://example.org/a?b=1", "https://plan.example"]);
        // Addresses go back out as themselves; a named link keeps its form.
        assert_eq!(to_markdown(h, true), "# Trip\nsee www.example.com, and (https://example.org/a?b=1).\n[our plan](https://plan.example)");
        // Twice changes nothing.
        link_addresses(h, 0, usize::MAX);
        assert_eq!(instructions(&text(h), 0, text(h).len()).len(), 3);
        unsafe { DestroyWindow(h) };
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
    fn emoji_survive_markdown() {
        let h = editor();
        setup(h);
        let md = "Trip \u{1F680}\n- **sun \u{1F600}** and \u{1F44D}\u{1F3FD} \u{2764}\u{FE0F}";
        load_markdown(h, md);
        assert_eq!(to_markdown(h, true), format!("# {md}"));
        assert_eq!(to_markdown(h, false), "Trip \u{1F680}\n- sun \u{1F600} and \u{1F44D}\u{1F3FD} \u{2764}\u{FE0F}");
        unsafe { DestroyWindow(h) };
    }

    #[test]
    fn literal_markers_stay_text() {
        let h = editor();
        setup(h);
        load_markdown(h, "Note\n\\- not a list\n- a real one\n\\1. not numbered");
        assert_eq!(to_markdown(h, true), "# Note\n\\- not a list\n- a real one\n\\1. not numbered");
        let stored = to_markdown(h, false);
        assert_eq!(stored, "Note\n\\- not a list\n- a real one\n\\1. not numbered");
        assert_eq!(crate::common::store::body_of(&stored), "- not a list\n•  a real one\n1. not numbered");
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
