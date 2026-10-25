// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures that fit the note: narrowed to the text column when the note is narrower than
//! them, and back to their own size when it's wide again.
//!
//! RichEdit keeps a picture's shown size only in its RTF (`\picwgoal`, `\pichgoal`), so a
//! picture is read and, when its size changes, written back through RTF.

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

/// Narrow pictures wider than the room they have, and widen narrowed ones again, in `h`.
/// Like refitting blocks, it's no step for Undo and no change to the note.
pub fn fit_picture_widths(h: HWND) {
    let t = text(h);
    if !t.contains(&OBJECT) {
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
    for cp in (0..t.len()).filter(|&i| t[i] == OBJECT) {
        let cp = cp as i32;
        let Some(pic) = read(h, cp) else { continue };
        // A picture of a kind whose size can't be read is only ever narrowed, and so is one
        // shown larger than its pixels make it (it came from elsewhere, sized there).
        let own = pic.pixels.map(natural).filter(|n| n.0 >= pic.shown.0).unwrap_or(pic.shown);
        let want = fitted(own, room_at(h, t, cp, column));
        if (want.0 - pic.shown.0).abs() >= SLACK {
            resize(h, cp, want);
        }
    }
    set_sel(h, a, b);
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

/// A picture from (the start of) its RTF.
fn parse(rtf: &[u8]) -> Option<Pic> {
    let s = String::from_utf8_lossy(rtf);
    let i = s.find("\\pict")?;
    let p = &s[i..];
    let (gw, gh) = (number(p, "\\picwgoal")?, number(p, "\\pichgoal")?);
    let scale = |w: &str| number(p, w).unwrap_or(100);
    let shown = (gw * scale("\\picscalex") / 100, gh * scale("\\picscaley") / 100);
    Some(Pic { shown, pixels: pixels(&data(p)) })
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
    let mut group = s[a..b].to_string();
    // HIMETRIC for the size in `\picw` and `\pich`, as RichEdit writes them.
    let himetric = |tw: i32| (tw as i64 * 2540 / 1440) as i32;
    for (word, v) in [("\\picwgoal", size.0), ("\\pichgoal", size.1), ("\\picscalex", 100), ("\\picscaley", 100), ("\\picw", himetric(size.0)), ("\\pich", himetric(size.1))] {
        group = set_number(&group, word, v);
    }
    let before = text_len(h);
    set_sel(h, cp, cp + 1);
    insert_rtf(h, &format!("{{\\rtf1 {group}}}"));
    debug_assert_eq!(text_len(h), before, "a picture written again is still one character");
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
        // Stored and opened again: as it was last shown.
        unsafe { MoveWindow(h, 0, 0, 150, 300, 0) };
        fit_picture_widths(h);
        let rtf = get_rtf(h);
        let h2 = unsafe { CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut()) };
        setup(h2);
        set_rtf(h2, &rtf);
        assert_eq!(read(h2, cp).unwrap().shown, narrow.shown);
        fit_picture_widths(h2);
        assert_eq!(read(h2, cp).unwrap().shown, (3600, 1800));
    }
}
