// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures that fit the note: narrowed to the text column when the note is narrower than
//! them, and back to their own size when it's wide again.
//!
//! RichEdit keeps a picture's shown size only in its RTF (`\picwgoal`, `\pichgoal`), so a
//! picture is read and, when its size changes, written back through RTF. It keeps nothing
//! else about it either (it writes its own scale, properties and crop), so what a picture
//! was narrowed from is remembered here, by editor, and the note stores that size.

use super::*;

/// The picture character in the editor's text.
const OBJECT: u16 = 0xFFFC;
/// A picture is shown at most this wide or tall (DIPs, as attachments are made), and never
/// narrowed below this; in twips (15 a DIP).
const MOST: i32 = 240 * 15;
const LEAST: i32 = 32 * 15;
/// A change smaller than this (two DIPs) isn't worth writing a picture again for.
const SLACK: i32 = 30;

/// A picture as its RTF has it: the size it's shown at (twips) and its own size in pixels,
/// where its data says.
#[derive(Debug, PartialEq)]
struct Pic {
    shown: (i32, i32),
    pixels: Option<(u32, u32)>,
}

/// The size a picture whose own size is `natural` (twips) is shown at in `room` twips: as
/// it is when it fits, else as wide as the room, its proportions kept.
pub(super) fn fitted(natural: (i32, i32), room: i32) -> (i32, i32) {
    let room = room.max(LEAST);
    if natural.0 <= room {
        return natural;
    }
    (room, ((natural.1 as i64 * room as i64) / natural.0.max(1) as i64).max(1) as i32)
}

/// A picture's own size in twips, from its pixels, as attachments are made: twice the
/// pixels of the size shown (at most the original's) and at most `MOST` either way.
pub(super) fn natural(px: (u32, u32)) -> (i32, i32) {
    let (w, h) = (px.0.max(1) as f32, px.1.max(1) as f32);
    let s = (240.0 / w).min(240.0 / h).min(1.0);
    (((w * s * 15.0).round() as i32).min(MOST), ((h * s * 15.0).round() as i32).min(MOST))
}

/// A picture the editor shows narrower than it is, because the note is narrow: which
/// picture (its attachment's link, and which of the pictures with that link), the size it
/// was narrowed to, and its own size, to grow back to and to store.
struct Narrowed {
    key: (String, usize),
    shown: (i32, i32),
    own: (i32, i32),
}

thread_local! {
    /// By editor: the pictures `fit_picture_widths` narrowed. Only ever as many as an open
    /// note has pictures, and an editor's go with it (`forget_pictures`).
    static NARROWED: std::cell::RefCell<std::collections::HashMap<isize, Vec<Narrowed>>> = std::cell::RefCell::new(std::collections::HashMap::new());
    /// A press on a selected picture, which may resize it.
    static PRESSED: std::cell::Cell<Option<Press>> = const { std::cell::Cell::new(None) };
}

/// (editor, the picture's position, its size when pressed).
type Press = (isize, i32, (i32, i32));

/// Editor `h` is going, or has new content: what it narrowed is no more.
pub fn forget_pictures(h: HWND) {
    NARROWED.with(|n| n.borrow_mut().remove(&(h as isize)));
}

/// The pictures at `cps` in `h`, each as its attachment's link and which of the pictures
/// with that link it is (a picture without one has an empty link).
fn keys(h: HWND, t: &[u16], cps: &[i32]) -> Vec<(String, usize)> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    cps.iter()
        .map(|&cp| {
            let url = item_link(h, t, cp as usize).map(|l| l.2).unwrap_or_default();
            let n = seen.entry(url.clone()).or_insert(0);
            *n += 1;
            (url, *n - 1)
        })
        .collect()
}

/// Narrow pictures wider than the room they have, and widen the ones it narrowed again,
/// in `h`. Like refitting blocks, it's no step for Undo and no change to the note: the
/// note keeps each picture's own size (`own_sizes`), which is the size it was made at, or
/// the one the user dragged it to. A picture is never shown wider than that.
pub fn fit_picture_widths(h: HWND) {
    let t = text(h);
    if !t.contains(&OBJECT) {
        forget_pictures(h);
        return;
    }
    let mask = send(h, EM_GETEVENTMASK, 0, 0);
    send(h, EM_SETEVENTMASK, 0, 0);
    still(h, || unrecorded(h, || fit_now(h, &t)));
    send(h, EM_SETEVENTMASK, 0, mask);
}

fn fit_now(h: HWND, t: &[u16]) {
    let (a, b) = sel(h);
    let column = column_twips(h);
    let cps: Vec<i32> = (0..t.len()).filter(|&i| t[i] == OBJECT).map(|i| i as i32).collect();
    let before = NARROWED.with(|n| n.borrow_mut().remove(&(h as isize))).unwrap_or_default();
    let keys = keys(h, t, &cps);
    // A picture narrowed here and then deleted can come back narrowed with Ctrl+Z: what it
    // was narrowed from is kept for it.
    let mut narrowed: Vec<Narrowed> = before.iter().filter(|n| !keys.contains(&n.key)).map(|n| Narrowed { key: n.key.clone(), shown: n.shown, own: n.own }).collect();
    for (cp, key) in cps.iter().copied().zip(keys) {
        let Some(pic) = read(h, cp) else { continue };
        // Narrowed here before, and still as it was left: its own size is what was kept.
        // Anything else (as made, as dragged to, as an undo put it back) is its own size.
        let own = before.iter().find(|n| n.key == key && n.shown == pic.shown).map_or(pic.shown, |n| n.own);
        let want = fitted(own, room_at(h, t, cp, column));
        let shown = if (want.0 - pic.shown.0).abs() >= SLACK {
            resize(h, cp, want);
            want
        } else {
            pic.shown
        };
        if shown != own {
            narrowed.push(Narrowed { key, shown, own });
        }
    }
    if !narrowed.is_empty() {
        NARROWED.with(|n| {
            let mut n = n.borrow_mut();
            // Editors destroyed without `forget_pictures` (scratch ones) go here.
            n.retain(|&w, _| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(w as HWND) } != 0);
            n.insert(h as isize, narrowed);
        });
    }
    set_sel(h, a, b);
}

/// `rtf`, editor `h`'s content, with the pictures `fit_picture_widths` narrowed at their
/// own size again: what the note stores, so a picture narrowed in a narrow note isn't
/// narrow for good.
pub(super) fn own_sizes(h: HWND, rtf: String) -> String {
    let Some(narrowed) = NARROWED.with(|n| n.borrow().get(&(h as isize)).map(|v| v.iter().map(|n| (n.key.clone(), n.shown, n.own)).collect::<Vec<_>>())) else {
        return rtf;
    };
    let t = text(h);
    let cps: Vec<i32> = (0..t.len()).filter(|&i| t[i] == OBJECT).map(|i| i as i32).collect();
    let keys = keys(h, &t, &cps);
    let groups = pict_groups(&rtf);
    // Each picture character is one picture group, in order; when that's not so, the
    // shown sizes are stored rather than any being put on the wrong picture.
    if groups.len() != keys.len() {
        return rtf;
    }
    let mut out = String::with_capacity(rtf.len());
    let mut at = 0;
    for ((a, b), key) in groups.into_iter().zip(keys) {
        let group = &rtf[a..b];
        let own = shown_of(group).and_then(|shown| narrowed.iter().find(|n| n.0 == key && n.1 == shown)).map(|n| n.2);
        out.push_str(&rtf[at..a]);
        match own {
            Some(own) => out.push_str(&sized(group, own)),
            None => out.push_str(group),
        }
        at = b;
    }
    out.push_str(&rtf[at..]);
    out
}

/// Notes stored before pictures kept their own size have them as last shown, narrowed
/// or not: each narrower than its pixels make it is put back at that size (as the editor
/// used to widen them whenever there was room). None when none was narrower.
pub fn widen_stored_pictures(rtf: &str) -> Option<String> {
    let mut out = String::with_capacity(rtf.len());
    let mut at = 0;
    let mut changed = false;
    for (a, b) in pict_groups(rtf) {
        let group = &rtf[a..b];
        out.push_str(&rtf[at..a]);
        let want = parse(group.as_bytes()).and_then(|pic| pic.pixels.map(natural).filter(|n| n.0 - pic.shown.0 >= SLACK));
        match want {
            Some(own) => {
                out.push_str(&sized(group, own));
                changed = true;
            }
            None => out.push_str(group),
        }
        at = b;
    }
    out.push_str(&rtf[at..]);
    changed.then_some(out)
}

/// A press in editor `h`: when it's on the picture already selected, it may be on one of
/// its handles, so its size is noted (see `picture_dragged`).
pub fn picture_pressed(h: HWND) {
    let (a, b) = sel(h);
    let press = (b == a + 1 && text(h).get(a as usize) == Some(&OBJECT)).then(|| read(h, a).map(|p| (h as isize, a, p.shown))).flatten();
    PRESSED.with(|p| p.set(press));
}

/// After a press on a selected picture (`picture_pressed`) and again when the mouse is let
/// go: if the user dragged the picture to a new size, that's an edit like any other, and
/// true is returned for the note to be saved. One Ctrl+Z takes the new size back to the
/// one before, and Ctrl+Y brings it again.
pub fn picture_dragged(h: HWND, released: bool) -> bool {
    let Some((w, cp, old)) = PRESSED.with(|p| p.get()) else { return false };
    if w != h as isize {
        return false;
    }
    if released {
        PRESSED.with(|p| p.set(None));
    }
    let Some(new) = (text(h).get(cp as usize) == Some(&OBJECT)).then(|| read(h, cp)).flatten().map(|p| p.shown) else { return false };
    if new == old {
        return false;
    }
    PRESSED.with(|p| p.set(None));
    const EM_UNDO: u32 = 0x00C7;
    let mask = send(h, EM_GETEVENTMASK, 0, 0);
    send(h, EM_SETEVENTMASK, 0, 0);
    still_as(h, false, || {
        // RichEdit may have made the drag a step of its own. If one undo puts the old size
        // back (and nothing else), it has, and redoing it leaves the step for Ctrl+Z.
        // Otherwise that undo (of something else, or nothing) is redone, and the change
        // is made a step here: back to the old size outside the history, then to the new.
        let before = text(h);
        let undid = send(h, EM_UNDO, 0, 0) != 0;
        let theirs = undid && text(h) == before && read(h, cp).map(|p| p.shown) == Some(old);
        if undid {
            send(h, EM_REDO, 0, 0);
        }
        if !theirs || read(h, cp).map(|p| p.shown) != Some(new) {
            unrecorded(h, || resize(h, cp, old));
            resize(h, cp, new);
        }
        set_sel(h, cp, cp + 1);
    });
    send(h, EM_SETEVENTMASK, 0, mask);
    end_group();
    true
}

/// The width a picture at `cp` can have: the text column (or its cell's), less its
/// paragraph's indents (a checklist item's text column, say).
fn room_at(h: HWND, t: &[u16], cp: i32, column: i32) -> i32 {
    set_sel(h, cp, cp);
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    let width = if t.contains(&ROW_START) { cell_text_width(h, t, cp).unwrap_or(column) } else { column };
    width - pf.start_indent.max(0) - pf.offset.max(0) - pf.right_indent.max(0)
}

/// The RTF of `a..b`, stopping once `cap` bytes are out (a picture's data can run to
/// hundreds of kilobytes; its size is at the start).
fn rtf_of(h: HWND, a: i32, b: i32, cap: usize) -> Vec<u8> {
    const SFF_SELECTION: usize = 0x8000;
    struct Capped {
        out: Vec<u8>,
        cap: usize,
    }
    unsafe extern "system" fn cb(cookie: usize, buf: *mut u8, n: i32, done: *mut i32) -> u32 {
        unsafe {
            let c = &mut *(cookie as *mut Capped);
            c.out.extend_from_slice(std::slice::from_raw_parts(buf, n as usize));
            *done = n;
            // Anything but 0 ends the stream.
            (c.out.len() >= c.cap) as u32
        }
    }
    let mut c = Capped { out: Vec::new(), cap };
    let mut es = EditStream { cookie: &mut c as *mut _ as usize, error: 0, callback: cb };
    set_sel(h, a, b);
    send(h, EM_STREAMOUT, SF_RTF as usize | SFF_SELECTION, &mut es as *mut _ as isize);
    c.out
}

/// The picture at `cp`, if that's one.
fn read(h: HWND, cp: i32) -> Option<Pic> {
    parse(&rtf_of(h, cp, cp + 1, 16 * 1024))
}

/// The number after control word `word` (`\picwgoal`) in `s`.
fn number(s: &str, word: &str) -> Option<i32> {
    let mut from = 0;
    while let Some(i) = s[from..].find(word) {
        let rest = &s[from + i + word.len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        // `\picw` is also the start of `\picwgoal`: only a number right after it counts.
        if !digits.is_empty() {
            return digits.parse().ok();
        }
        from += i + word.len();
    }
    None
}

/// Where the `{\pict ...}` group starts in `s` and where it ends (past its brace).
fn pict_group(s: &str) -> Option<(usize, usize)> {
    let i = s.find("{\\pict")?;
    let mut depth = 0;
    for (j, c) in s[i..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((i, i + j + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `{\pict ...}` group in `s`, in order.
fn pict_groups(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((a, b)) = pict_group(&s[from..]) {
        out.push((from + a, from + b));
        from += b;
    }
    out
}

/// A picture from (the start of) its RTF.
fn parse(rtf: &[u8]) -> Option<Pic> {
    let s = String::from_utf8_lossy(rtf);
    let i = s.find("\\pict")?;
    let p = &s[i..];
    Some(Pic { shown: shown_of(p)?, pixels: pixels(&data(p)) })
}

/// The size a picture is shown at, from its RTF (starting at `\pict`).
fn shown_of(p: &str) -> Option<(i32, i32)> {
    let (gw, gh) = (number(p, "\\picwgoal")?, number(p, "\\pichgoal")?);
    let scale = |w: &str| number(p, w).unwrap_or(100);
    Some((gw * scale("\\picscalex") / 100, gh * scale("\\picscaley") / 100))
}

/// The picture's data, from its group's text: what comes after its control words (and
/// the groups among them), as bytes.
fn data(p: &str) -> Vec<u8> {
    let b = p.as_bytes();
    let mut i = "\\pict".len();
    loop {
        match b.get(i) {
            Some(b'{') => {
                // A group of properties: skip it whole.
                let mut depth = 0;
                while let Some(&c) = b.get(i) {
                    i += 1;
                    if c == b'{' {
                        depth += 1;
                    } else if c == b'}' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            Some(b'\\') => {
                i += 1;
                while b.get(i).is_some_and(|c| c.is_ascii_alphabetic() || c.is_ascii_digit() || *c == b'-') {
                    i += 1;
                }
            }
            Some(c) if c.is_ascii_whitespace() => i += 1,
            _ => break,
        }
    }
    let hex: Vec<u8> = b[i..].iter().copied().take_while(|c| c.is_ascii_hexdigit() || c.is_ascii_whitespace()).filter(u8::is_ascii_hexdigit).collect();
    hex.as_chunks::<2>().0.iter().filter_map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()).collect()
}

/// A PNG's or JPEG's size in pixels, from its first bytes.
fn pixels(d: &[u8]) -> Option<(u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]) as u32);
    let be32 = |i: usize| Some(u32::from_be_bytes([*d.get(i)?, *d.get(i + 1)?, *d.get(i + 2)?, *d.get(i + 3)?]));
    if d.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some((be32(16)?, be32(20)?));
    }
    if !d.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    // JPEG: segment by segment to the frame header, which has the size.
    let mut i = 2;
    while d.get(i) == Some(&0xFF) {
        let m = *d.get(i + 1)?;
        if (0xC0..=0xCF).contains(&m) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            return Some((be16(i + 7)?, be16(i + 5)?));
        }
        i += 2 + be16(i + 2)? as usize;
    }
    None
}

/// Show the picture at `cp` at `size` twips: its RTF again, with that size.
fn resize(h: HWND, cp: i32, size: (i32, i32)) {
    let full = rtf_of(h, cp, cp + 1, usize::MAX);
    let s = String::from_utf8_lossy(&full);
    let Some((a, b)) = pict_group(&s) else { return };
    let group = sized(&s[a..b], size);
    let before = text_len(h);
    set_sel(h, cp, cp + 1);
    insert_rtf(h, &format!("{{\\rtf1 {group}}}"));
    debug_assert_eq!(text_len(h), before, "a picture written again is still one character");
}

/// A picture's group with its size set to `size` twips.
fn sized(group: &str, size: (i32, i32)) -> String {
    // HIMETRIC for the size in `\picw` and `\pich`, as RichEdit writes them.
    let himetric = |tw: i32| (tw as i64 * 2540 / 1440) as i32;
    let mut group = group.to_string();
    for (word, v) in [("\\picwgoal", size.0), ("\\pichgoal", size.1), ("\\picscalex", 100), ("\\picscaley", 100), ("\\picw", himetric(size.0)), ("\\pich", himetric(size.1))] {
        group = set_number(&group, word, v);
    }
    group
}

/// `s` with the number after control word `word` set to `v` (where it has one).
fn set_number(s: &str, word: &str, v: i32) -> String {
    let mut from = 0;
    while let Some(i) = s[from..].find(word) {
        let at = from + i + word.len();
        let n = s[at..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').count();
        if n > 0 {
            return format!("{}{v}{}", &s[..at], &s[at + n..]);
        }
        from = at;
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, MoveWindow, WS_POPUP};

    #[test]
    fn sizes() {
        // Fits: as it is; too wide: as wide as the room, proportions kept.
        assert_eq!(fitted((3600, 1800), 5000), (3600, 1800));
        assert_eq!(fitted((3600, 1800), 2000), (2000, 1000));
        assert_eq!(fitted((3600, 1800), 10), (LEAST, LEAST / 2));
        // Its own size, as attachments are made from pixels.
        assert_eq!(natural((480, 240)), (3600, 1800));
        assert_eq!(natural((100, 50)), (1500, 750));
        assert_eq!(natural((300, 600)), (1800, 3600));
        assert_eq!(set_number("\\picw10\\picwgoal20", "\\picw", 5), "\\picw5\\picwgoal20");
        assert_eq!(set_number("\\picwgoal20\\picw10", "\\picw", 5), "\\picwgoal20\\picw5");
    }

    /// A picture in a note made narrower than it is narrowed to fit, and comes back to its
    /// own size when the note is wide again; neither is a step for Undo, and the text
    /// around it stays as it was.
    #[test]
    fn pictures_follow_the_width() {
        const EM_CANUNDO: u32 = 0x00C6;
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        insert_text(h, "T\rbefore\r\rafter");
        let t = text(h);
        let at = t.iter().position(|&c| c == b'a' as u16).unwrap() as i32 - 1;
        set_sel(h, at, at);
        // As an attachment is: the picture, then its link.
        let link = "{\\field{\\*\\fldinst{HYPERLINK \"attachment:p.png\"}}{\\fldrslt{\\u8203?}}}";
        insert_rtf(h, &format!("{{\\rtf1 {}{link}}}", test_picture(480, 240, 3600, 1800)));
        let cp = text(h).iter().position(|&c| c == OBJECT).unwrap() as i32;
        assert!(item_link(h, &text(h), cp as usize).is_some());
        let pic = read(h, cp).unwrap();
        assert_eq!(pic, Pic { shown: (3600, 1800), pixels: Some((480, 240)) });
        send(h, EM_EMPTYUNDOBUFFER, 0, 0);
        let text_before = text(h);
        fit_picture_widths(h);
        assert_eq!(read(h, cp).unwrap().shown, (3600, 1800), "it fits: left alone");
        // 150 px wide: a column of 2,220 twips.
        unsafe { MoveWindow(h, 0, 0, 150, 300, 0) };
        fit_picture_widths(h);
        let narrow = read(h, cp).unwrap();
        assert_eq!(narrow.shown, (column_twips(h), column_twips(h) / 2));
        assert_eq!(narrow.pixels, Some((480, 240)), "the picture itself is kept");
        assert_eq!(text(h), text_before);
        assert_eq!(item_link(h, &text(h), cp as usize).map(|l| l.2), Some("attachment:p.png".to_string()), "and its link");
        assert_eq!(send(h, EM_CANUNDO, 0, 0), 0, "no step for Undo");
        unsafe { MoveWindow(h, 0, 0, 400, 300, 0) };
        fit_picture_widths(h);
        assert_eq!(read(h, cp).unwrap().shown, (3600, 1800), "back to its own size");
        assert_eq!(text(h), text_before);
        assert_eq!(send(h, EM_CANUNDO, 0, 0), 0);
        // Stored while narrowed: at its own size, which a wide note opens it at.
        unsafe { MoveWindow(h, 0, 0, 150, 300, 0) };
        fit_picture_widths(h);
        assert_eq!(read(h, cp).unwrap().shown, narrow.shown);
        let rtf = get_rtf(h);
        let h2 = editor(400);
        set_rtf(h2, &rtf);
        assert_eq!(read(h2, cp).unwrap().shown, (3600, 1800));
        fit_picture_widths(h2);
        assert_eq!(read(h2, cp).unwrap().shown, (3600, 1800));
        assert_eq!(set_text_of(h2), set_text_of(h), "nothing else changes on the way");
    }

    fn editor(width: i32) -> HWND {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, width, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        h
    }

    /// The text of `h` as plain characters, for comparing two editors.
    fn set_text_of(h: HWND) -> String {
        String::from_utf16_lossy(&text(h))
    }

    /// A note with a 480 x 240 px picture (and its link) shown at its own 3600 x 1800
    /// twips, and the picture's position, with nothing to undo.
    fn with_picture(width: i32) -> (HWND, i32) {
        let h = editor(width);
        insert_text(h, "T\rbefore\r\rafter");
        let at = text(h).iter().position(|&c| c == b'a' as u16).unwrap() as i32 - 1;
        set_sel(h, at, at);
        let link = "{\\field{\\*\\fldinst{HYPERLINK \"attachment:p.png\"}}{\\fldrslt{\\u8203?}}}";
        insert_rtf(h, &format!("{{\\rtf1 {}{link}}}", test_picture(480, 240, 3600, 1800)));
        send(h, EM_EMPTYUNDOBUFFER, 0, 0);
        (h, text(h).iter().position(|&c| c == OBJECT).unwrap() as i32)
    }

    /// The user drags the selected picture at `cp` to `size`, as RichEdit would: with its
    /// own undo step (`recorded`) or without.
    fn drag(h: HWND, cp: i32, size: (i32, i32), recorded: bool) -> bool {
        set_sel(h, cp, cp + 1);
        picture_pressed(h);
        if recorded {
            resize(h, cp, size);
        } else {
            unrecorded(h, || resize(h, cp, size));
        }
        set_sel(h, cp, cp + 1);
        picture_dragged(h, true)
    }

    /// A picture the user made narrower keeps that size: a wider note doesn't widen it,
    /// a narrower one narrows it and gives it back, and the note stores it.
    #[test]
    fn a_dragged_size_stays() {
        let (h, cp) = with_picture(400);
        assert!(drag(h, cp, (2000, 1000), false), "a new size is an edit");
        fit_picture_widths(h);
        assert_eq!(read(h, cp).unwrap().shown, (2000, 1000), "room for more: kept as chosen");
        unsafe { MoveWindow(h, 0, 0, 120, 300, 0) };
        fit_picture_widths(h);
        let narrow = read(h, cp).unwrap().shown;
        assert!(narrow.0 < 2000, "{narrow:?}");
        let h2 = editor(400);
        set_rtf(h2, &get_rtf(h));
        assert_eq!(read(h2, cp).unwrap().shown, (2000, 1000), "stored at the size chosen");
        unsafe { MoveWindow(h, 0, 0, 400, 300, 0) };
        fit_picture_widths(h);
        assert_eq!(read(h, cp).unwrap().shown, (2000, 1000), "and back to it, not to its pixels' size");
        // A press that doesn't change the size is nothing.
        set_sel(h, cp, cp + 1);
        picture_pressed(h);
        assert!(!picture_dragged(h, true));
    }

    /// Ctrl+Z takes a drag back to the size before it (an earlier drag's, not the
    /// picture's own), in one step whether RichEdit made the drag a step or not, Ctrl+Y
    /// brings it again, and undoing typing after it leaves the size alone.
    #[test]
    fn undoing_a_drag() {
        const EM_UNDO: u32 = 0x00C7;
        for recorded in [false, true] {
            let (h, cp) = with_picture(400);
            let before = text(h);
            assert!(drag(h, cp, (2400, 1200), recorded));
            assert!(drag(h, cp, (1800, 900), recorded));
            send(h, EM_UNDO, 0, 0);
            assert_eq!(read(h, cp).unwrap().shown, (2400, 1200), "recorded by RichEdit: {recorded}");
            assert_eq!(text(h), before);
            send(h, EM_REDO, 0, 0);
            assert_eq!(read(h, cp).unwrap().shown, (1800, 900));
            fit_picture_widths(h);
            assert_eq!(read(h, cp).unwrap().shown, (1800, 900));
            // Typing, then taking it back.
            let end = text(h).len() as i32;
            set_sel(h, end, end);
            replace_sel(h, " more");
            send(h, EM_UNDO, 0, 0);
            assert_eq!(text(h), before);
            fit_picture_widths(h);
            assert_eq!(read(h, cp).unwrap().shown, (1800, 900), "typing undone, size kept");
            send(h, EM_UNDO, 0, 0);
            assert_eq!(read(h, cp).unwrap().shown, (2400, 1200));
            send(h, EM_UNDO, 0, 0);
            assert_eq!(read(h, cp).unwrap().shown, (3600, 1800));
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(h) };
        }
    }

    /// Notes stored before pictures kept their own size get it back, once; a picture at
    /// its size, or shown larger than its pixels, is left as it is.
    #[test]
    fn stored_pictures_widen_once() {
        let narrowed = format!("{{\\rtf1 a{}b}}", test_picture(480, 240, 1800, 900));
        let widened = widen_stored_pictures(&narrowed).unwrap();
        assert_eq!(parse(widened.as_bytes()).unwrap().shown, (3600, 1800));
        assert!(widened.starts_with("{\\rtf1 a") && widened.ends_with("b}"));
        assert_eq!(widen_stored_pictures(&widened), None);
        assert_eq!(widen_stored_pictures(&format!("{{\\rtf1 {}}}", test_picture(100, 50, 3000, 1500))), None);
        assert_eq!(widen_stored_pictures("{\\rtf1 no pictures}"), None);
    }
}
