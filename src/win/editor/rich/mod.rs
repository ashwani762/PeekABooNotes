// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! RichEdit (msftedit.dll) bindings and helpers.
//!
//! windows-sys no longer ships the RichEdit headers, so the handful of messages and
//! structs we use are declared here. richedit.h uses 4-byte packing on Win32.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, SendMessageW};

mod blocks;
mod format;
mod lists;
mod links;
mod markdown;
mod pictures;
mod selectors;
pub use blocks::*;
pub use format::*;
pub use links::*;
pub use lists::*;
pub use markdown::*;
pub use pictures::{fit_picture_widths, forget_pictures, picture_dragged, picture_pressed, picture_shown, pictures_changed, refit_picture_widths, widen_stored_pictures};
pub use selectors::{has_selectors, hide_marks, marker_for, style_emoji, unmark, unmark_str};

pub const CLASS: &str = "RICHEDIT50W";

const WM_USER: u32 = 0x0400;
pub const EM_GETSEL: u32 = 0x00B0;
pub const EM_SETSEL: u32 = 0x00B1;
pub const EM_REPLACESEL: u32 = 0x00C2;
pub const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
pub const EM_POSFROMCHAR: u32 = 0x00D6;
pub const EM_CHARFROMPOS: u32 = 0x00D7;
pub const EM_REDO: u32 = 0x0454;
pub const EM_SETUNDOLIMIT: u32 = WM_USER + 82;
pub const EM_GETCHARFORMAT: u32 = WM_USER + 58;
pub const EM_GETPARAFORMAT: u32 = WM_USER + 61;
pub const EM_HIDESELECTION: u32 = WM_USER + 63;
pub const EM_PASTESPECIAL: u32 = WM_USER + 64;
pub const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
pub const EM_SETCHARFORMAT: u32 = WM_USER + 68;
pub const EM_SETEVENTMASK: u32 = WM_USER + 69;
const EM_GETEVENTMASK: u32 = WM_USER + 59;
const EM_EXLIMITTEXT: u32 = WM_USER + 53;
pub const EM_SETPARAFORMAT: u32 = WM_USER + 71;
pub const EM_STREAMIN: u32 = WM_USER + 73;
pub const EM_STREAMOUT: u32 = WM_USER + 74;
pub const EM_GETTEXTRANGE: u32 = WM_USER + 75;
pub const EM_SETOPTIONS: u32 = WM_USER + 77;
pub const EM_AUTOURLDETECT: u32 = WM_USER + 91;
pub const EM_GETTEXTEX: u32 = WM_USER + 94;
pub const EM_GETTEXTLENGTHEX: u32 = WM_USER + 95;
pub const EM_SETTEXTEX: u32 = WM_USER + 97;
pub const EM_SETTYPOGRAPHYOPTIONS: u32 = WM_USER + 202;
pub const EM_GETSCROLLPOS: u32 = WM_USER + 221;
pub const EM_SETSCROLLPOS: u32 = WM_USER + 222;

pub const EN_CHANGE: u32 = 0x0300;
pub const EN_LINK: u32 = 0x070B;
pub const ENM_CHANGE: u32 = 0x0000_0001;
pub const ENM_LINK: u32 = 0x0400_0000;

const SF_RTF: u32 = 0x0002;

pub const CFM_BOLD: u32 = 0x0001;
pub const CFM_ITALIC: u32 = 0x0002;
pub const CFM_UNDERLINE: u32 = 0x0004;
pub const CFM_STRIKEOUT: u32 = 0x0008;
pub const CFM_HIDDEN: u32 = 0x0100;
pub const CFM_SIZE: u32 = 0x8000_0000;
pub const CFM_COLOR: u32 = 0x4000_0000;
pub const CFM_FACE: u32 = 0x2000_0000;
pub const CFM_CHARSET: u32 = 0x0800_0000;
pub const SCF_DEFAULT: usize = 0x0000;
pub const SCF_SELECTION: usize = 0x0001;
pub const SCF_ALL: usize = 0x0004;

pub const PFM_STARTINDENT: u32 = 0x0001;
pub const PFM_ALIGNMENT: u32 = 0x0008;
pub const PFA_LEFT: u16 = 1;
pub const PFA_RIGHT: u16 = 2;
pub const PFA_CENTER: u16 = 3;
pub const PFM_OFFSET: u32 = 0x0004;
pub const PFM_NUMBERING: u32 = 0x0020;
pub const PFM_SPACEBEFORE: u32 = 0x0040;
pub const PFM_SPACEAFTER: u32 = 0x0080;
pub const PFM_LINESPACING: u32 = 0x0100;
pub const PFM_NUMBERINGSTYLE: u32 = 0x2000;
pub const PFM_NUMBERINGTAB: u32 = 0x4000;
pub const PFM_NUMBERINGSTART: u32 = 0x8000;
pub const PFN_BULLET: u16 = 1;
pub const PFN_ARABIC: u16 = 2;
const PFN_LCLETTER: u16 = 3;
const PFN_LCROMAN: u16 = 5;
const PFNS_PERIOD: u16 = 0x0200;

const ECOOP_OR: usize = 2;
const ECO_AUTOVSCROLL: isize = 0x0040;
const ECO_SELECTIONBAR: isize = 0x0100_0000;
/// How far the selection bar insets the text on the left, in DIPs: RichEdit makes it 225
/// HIMETRIC wide (8 px at 96 DPI) and scales it with the window's DPI. Nothing insets the
/// right side, so a window wanting even margins adds this on its right.
pub const SELECTION_BAR: f32 = 225.0 * 96.0 / 2540.0;
const TO_ADVANCEDTYPOGRAPHY: usize = 1;
const CF_UNICODETEXT: usize = 13;
/// The text as RichEdit holds it: a table keeps its row and cell marks (by default they
/// come out as tabs, and every position after one is off).
const GT_RAWTEXT: u32 = 4;
const GTL_PRECISE: u32 = 2;
const GTL_NUMCHARS: u32 = 8;
const ST_DEFAULT: u32 = 0;
const ST_SELECTION: u32 = 2;
const CP_UTF16: u32 = 1200;

pub const BOX: u16 = 0x2610; // ☐
pub const CHECKED: u16 = 0x2611; // ☑
pub const CR: u16 = 13;

type StreamCb = unsafe extern "system" fn(usize, *mut u8, i32, *mut i32) -> u32;

#[repr(C, packed(4))]
struct EditStream {
    cookie: usize,
    error: u32,
    callback: StreamCb,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CharFormat2 {
    pub cb_size: u32,
    pub mask: u32,
    pub effects: u32,
    pub height: i32,
    pub offset: i32,
    pub color: u32,
    pub charset: u8,
    pub pitch_family: u8,
    pub face: [u16; 32],
    pub weight: u16,
    pub spacing: i16,
    pub back_color: u32,
    pub lcid: u32,
    pub reserved: u32,
    pub style: i16,
    pub kerning: u16,
    pub underline_type: u8,
    pub animation: u8,
    pub rev_author: u8,
    pub underline_color: u8,
}

impl CharFormat2 {
    pub fn new() -> Self {
        let mut c: Self = unsafe { std::mem::zeroed() };
        c.cb_size = size_of::<Self>() as u32;
        c
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParaFormat2 {
    pub cb_size: u32,
    pub mask: u32,
    pub numbering: u16,
    pub effects: u16,
    pub start_indent: i32,
    pub right_indent: i32,
    pub offset: i32,
    pub alignment: u16,
    pub tab_count: i16,
    pub tabs: [i32; 32],
    pub space_before: i32,
    pub space_after: i32,
    pub line_spacing: i32,
    pub style: i16,
    pub line_spacing_rule: u8,
    pub outline_level: u8,
    pub shading_weight: u16,
    pub shading_style: u16,
    pub numbering_start: u16,
    pub numbering_style: u16,
    pub numbering_tab: u16,
    pub border_space: u16,
    pub border_width: u16,
    pub borders: u16,
}

impl ParaFormat2 {
    pub fn new() -> Self {
        let mut p: Self = unsafe { std::mem::zeroed() };
        p.cb_size = size_of::<Self>() as u32;
        p
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CharRange {
    pub min: i32,
    pub max: i32,
}

#[repr(C, packed(4))]
pub struct EnLink {
    pub hdr: windows_sys::Win32::UI::Controls::NMHDR,
    pub msg: u32,
    pub wparam: WPARAM,
    pub lparam: LPARAM,
    pub chrg: CharRange,
}

#[repr(C)]
struct TextRange {
    chrg: CharRange,
    text: *mut u16,
}

#[repr(C, packed(4))]
struct GetTextEx {
    cb: u32,
    flags: u32,
    codepage: u32,
    default_char: *const u8,
    used_default: *mut i32,
}

#[repr(C)]
struct GetTextLengthEx {
    flags: u32,
    codepage: u32,
}

#[repr(C)]
struct SetTextEx {
    flags: u32,
    codepage: u32,
}

#[inline]
pub fn send(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(h, msg, wp, lp) }
}

unsafe extern "system" fn out_cb(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    unsafe {
        let v = &mut *(cookie as *mut Vec<u8>);
        v.extend_from_slice(std::slice::from_raw_parts(buf, cb as usize));
        *pcb = cb;
    }
    0
}

struct InCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "system" fn in_cb(cookie: usize, buf: *mut u8, cb: i32, pcb: *mut i32) -> u32 {
    unsafe {
        let c = &mut *(cookie as *mut InCursor);
        let n = (c.data.len() - c.pos).min(cb as usize);
        std::ptr::copy_nonoverlapping(c.data.as_ptr().add(c.pos), buf, n);
        c.pos += n;
        *pcb = n as i32;
    }
    0
}

pub fn get_rtf(h: HWND) -> String {
    let mut out: Vec<u8> = Vec::new();
    let mut es = EditStream { cookie: &mut out as *mut _ as usize, error: 0, callback: out_cb };
    selectors::symbols_saved_whole(h, || send(h, EM_STREAMOUT, SF_RTF as usize, &mut es as *mut _ as isize));
    // Pictures narrowed to fit the note go out at their own size.
    pictures::own_sizes(h, String::from_utf8_lossy(&out).into_owned())
}

pub fn set_rtf(h: HWND, rtf: &str) {
    let rtf = selectors::rtf_to_marks(&strip_weights(rtf));
    let mut cur = InCursor { data: rtf.as_bytes(), pos: 0 };
    let mut es = EditStream { cookie: &mut cur as *mut _ as usize, error: 0, callback: in_cb };
    send(h, EM_STREAMIN, SF_RTF as usize, &mut es as *mut _ as isize);
    pictures::forget_pictures(h);
    hide_marks(h);
}

/// Drop explicit `\fweightN` words. An explicit weight wins over `\b` when RichEdit
/// reads a document back, which made bold text come back regular; without them, bold
/// comes from `\b` alone (titles get their weight again from `style_title`).
fn strip_weights(rtf: &str) -> String {
    const WORD: &str = "\\fweight";
    let mut out = String::with_capacity(rtf.len());
    let mut rest = rtf;
    while let Some(i) = rest.find(WORD) {
        out.push_str(&rest[..i]);
        let tail = &rest[i + WORD.len()..];
        let digits = tail.bytes().take_while(|b| b.is_ascii_digit()).count();
        rest = &tail[digits..];
        // A control word's delimiting space belongs to it.
        if digits > 0 {
            rest = rest.strip_prefix(' ').unwrap_or(rest);
        } else {
            out.push_str(WORD);
        }
    }
    out.push_str(rest);
    out
}

/// How many characters `h` holds (as `text` counts them), without copying them.
pub fn text_len(h: HWND) -> usize {
    let gtl = GetTextLengthEx { flags: GTL_PRECISE | GTL_NUMCHARS, codepage: CP_UTF16 };
    send(h, EM_GETTEXTLENGTHEX, &gtl as *const _ as usize, 0).max(0) as usize
}

/// Full text with bare `\r` paragraph marks; indices match RichEdit character positions.
pub fn text(h: HWND) -> Vec<u16> {
    let gtl = GetTextLengthEx { flags: GTL_PRECISE | GTL_NUMCHARS, codepage: CP_UTF16 };
    let len = send(h, EM_GETTEXTLENGTHEX, &gtl as *const _ as usize, 0).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let gt = GetTextEx {
        cb: ((len + 1) * 2) as u32,
        flags: GT_RAWTEXT,
        codepage: CP_UTF16,
        default_char: std::ptr::null(),
        used_default: null_mut(),
    };
    let n = send(h, EM_GETTEXTEX, &gt as *const _ as usize, buf.as_mut_ptr() as isize).max(0) as usize;
    buf.truncate(n);
    buf
}

pub fn range_text(h: HWND, a: i32, b: i32) -> Vec<u16> {
    if b <= a {
        return Vec::new();
    }
    let mut buf = vec![0u16; (b - a + 1) as usize];
    let mut tr = TextRange { chrg: CharRange { min: a, max: b }, text: buf.as_mut_ptr() };
    let n = send(h, EM_GETTEXTRANGE, 0, &mut tr as *mut _ as isize).max(0) as usize;
    buf.truncate(n);
    buf
}

/// Some of an editor's text, as `text` has it: `t[i]` is character `from + i`, of `len`
/// in all. What the pointer, a key or a paint needs is a paragraph or a screenful, not a
/// copy of the whole note (2 MB a pointer move in a long one).
pub struct Span {
    pub from: usize,
    pub t: Vec<u16>,
    pub len: usize,
}

impl Span {
    /// Character `cp`, when it's in the span.
    pub fn get(&self, cp: usize) -> Option<u16> {
        cp.checked_sub(self.from).and_then(|i| self.t.get(i)).copied()
    }

    /// Just past the span's last character.
    #[cfg(test)]
    fn end(&self) -> usize {
        self.from + self.t.len()
    }
}

/// Characters `a..b` as `text` has them. EM_GETTEXTRANGE (`range_text`) leaves a table's
/// row marks out, so every position after one would be off; the text object model's
/// range gives them as they are.
pub fn raw_text(h: HWND, a: usize, b: usize) -> Vec<u16> {
    if b <= a {
        return Vec::new();
    }
    range(h, a as i32, b as i32).and_then(|r| unsafe { r.GetText() }.ok()).map_or_else(Vec::new, |s| s.to_vec())
}

/// The paragraphs from the one holding `a` to the one holding `b`, with the paragraph mark
/// before them (when there is one) and the one ending them: their text and their marks'
/// neighbours, as positions in the whole text (`text`) have them.
pub fn paras_around(h: HWND, a: usize, b: usize) -> Span {
    let len = text_len(h);
    let (a, b) = (a.min(len), b.max(a).min(len));
    // Back to the mark before `a`, and on to the one at or after `b`, a widening piece at
    // a time.
    let mut from = 0;
    let mut back = 256;
    loop {
        let lo = a.saturating_sub(back);
        if let Some(i) = raw_text(h, lo, a).iter().rposition(|&c| c == CR) {
            from = lo + i;
            break;
        }
        if lo == 0 {
            break;
        }
        back *= 4;
    }
    let mut end = len;
    let mut ahead = 256;
    loop {
        let hi = (b + ahead).min(len);
        if let Some(i) = raw_text(h, b, hi).iter().position(|&c| c == CR) {
            end = b + i + 1;
            break;
        }
        if hi == len {
            break;
        }
        ahead *= 4;
    }
    Span { from, t: raw_text(h, from, end), len }
}

pub fn sel(h: HWND) -> (i32, i32) {
    let (mut a, mut b) = (0u32, 0u32);
    send(h, EM_GETSEL, &mut a as *mut _ as usize, &mut b as *mut _ as isize);
    (a as i32, b as i32)
}

/// Whether the selection is all inside the first paragraph, the note's title.
pub fn sel_in_title(h: HWND) -> bool {
    let (a, b) = sel(h);
    !range_text(h, 0, a.max(b)).contains(&CR)
}

pub fn set_sel(h: HWND, a: i32, b: i32) {
    send(h, EM_SETSEL, a as usize, b as isize);
}

thread_local! {
    /// Editors whose selection stays hidden (a picked picture, framed instead of drawn as
    /// RichEdit's colour negative): the changes that hide it while they work don't show it
    /// again after.
    static HIDDEN: std::cell::RefCell<Vec<isize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Keep `h`'s selection hidden (`on`) until told otherwise, or show it again.
pub fn hold_selection_hidden(h: HWND, on: bool) {
    HIDDEN.with(|v| {
        let mut v = v.borrow_mut();
        v.retain(|&w| w != h as isize);
        if on {
            v.push(h as isize);
        }
    });
    send(h, EM_HIDESELECTION, on as usize, 0);
}

/// Whether `h`'s selection is held hidden (`hold_selection_hidden`).
pub fn selection_held_hidden(h: HWND) -> bool {
    HIDDEN.with(|v| v.borrow().contains(&(h as isize)))
}

/// Show `h`'s selection again after a change that hid it while it worked, unless it's held
/// hidden.
pub fn show_selection(h: HWND) {
    if !selection_held_hidden(h) {
        send(h, EM_HIDESELECTION, 0, 0);
    }
}

/// Run `f`, which only reads `h` but moves its selection about to do it (Markdown out asks
/// each paragraph and run through the selection), with the editor frozen (`bulk`): unfrozen,
/// RichEdit lays out and draws at every move, which made a 100,000-character note's
/// Markdown take 250 ms rather than 37. The selection and the view are as they were after.
pub fn reading<R>(h: HWND, f: impl FnOnce() -> R) -> R {
    let (a, b) = sel(h);
    let y = scroll_y(h);
    let r = bulk(h, || {
        let r = f();
        set_sel(h, a, b);
        r
    });
    if scroll_y(h) != y {
        set_scroll_y(h, y);
    }
    r
}

pub fn replace_sel(h: HWND, s: &str) {
    let w = crate::win::util::wide(&selectors::to_marks(s));
    send(h, EM_REPLACESEL, 1, w.as_ptr() as isize);
    if s.contains(['\u{FE0F}', '\u{FE0E}']) {
        hide_marks(h);
    }
}

/// Replace the selection with an RTF fragment ("{\rtf1 ...}"), undoably.
pub fn insert_rtf(h: HWND, rtf: &str) {
    const ST_KEEPUNDO: u32 = 1;
    let mut bytes = rtf.as_bytes().to_vec();
    bytes.push(0);
    let st = SetTextEx { flags: ST_SELECTION | ST_KEEPUNDO, codepage: 0 };
    send(h, EM_SETTEXTEX, &st as *const _ as usize, bytes.as_ptr() as isize);
}

/// Replace the selection with text, bypassing the subclass/undo tricks (used for "\r" inserts).
pub fn insert_text(h: HWND, s: &str) {
    let w = crate::win::util::wide(&selectors::to_marks(s));
    let st = SetTextEx { flags: ST_SELECTION | ST_DEFAULT, codepage: CP_UTF16 };
    send(h, EM_SETTEXTEX, &st as *const _ as usize, w.as_ptr() as isize);
    if s.contains(['\u{FE0F}', '\u{FE0E}']) {
        hide_marks(h);
    }
}

/// Typing and pasting stop at the note limit, or where the note already is when it was
/// saved longer (it keeps all of that, and just can't grow). The limit is on what the
/// note shows: RichEdit counts every character it holds, so what it holds out of sight
/// is let through on top. That changes as links and tables come and go, so this is asked
/// again after each burst of edits.
pub fn limit_length(h: HWND) {
    limit_length_of(h, &text(h));
}

/// `limit_length`, with the editor's text already in hand.
pub fn limit_length_of(h: HWND, t: &[u16]) {
    send(h, EM_EXLIMITTEXT, 0, length_limit(t).min(i32::MAX as usize) as isize);
}

/// Whether `h` holds more than a note may: its text, links' hidden addresses left out,
/// as typing counts it (`limit_length`).
pub fn too_long(h: HWND) -> bool {
    let t = text(h);
    t.len() > crate::common::store::MAX_NOTE_CHARS + hidden_len_with(&t, &instructions(&t, 0, t.len()))
}

/// The most characters RichEdit may hold, given what it holds now (`t`).
fn length_limit(t: &[u16]) -> usize {
    length_limit_with(t, &instructions(t, 0, t.len()))
}

/// `length_limit`, with `t`'s link instructions in hand.
fn length_limit_with(t: &[u16], links: &[(usize, usize, String)]) -> usize {
    t.len().max(crate::common::store::MAX_NOTE_CHARS + hidden_len_with(t, links))
}

/// What follows from the text after a burst of edits: the title's look, the links'
/// colour, a checklist indent left behind, and the length limit (see `limit_length`).
/// Each read the text for itself, and the links twice: done together, they read it
/// once. True when the text has emoji variation selectors to hide (see `selectors`).
pub fn restyle(h: HWND) -> bool {
    let t = text(h);
    style_title_in(h, &t);
    let links = instructions(&t, 0, t.len());
    color_links_listed(h, &t, &links);
    fix_caret_indent(h, &t);
    style_emoji(h, &t);
    // None of these moves or changes a character: `t` is still the text.
    send(h, EM_EXLIMITTEXT, 0, length_limit_with(&t, &links).min(i32::MAX as usize) as isize);
    has_selectors(&t)
}

/// How many characters of `t` (the editor's text) never show: named links' hidden
/// `HYPERLINK "url"` instructions, and the marks (each with its paragraph mark) that
/// start and end a table row.
#[cfg(test)]
pub fn hidden_len(t: &[u16]) -> usize {
    hidden_len_with(t, &instructions(t, 0, t.len()))
}

/// `hidden_len`, with `t`'s link instructions in hand.
fn hidden_len_with(t: &[u16], links: &[(usize, usize, String)]) -> usize {
    let links: usize = links.iter().map(|(at, name, _)| name - at).sum();
    let mut rows = 0;
    for (i, &c) in t.iter().enumerate() {
        if c == ROW_START || c == ROW_END {
            rows += if t.get(i + 1) == Some(&CR) { 2 } else { 1 };
        }
    }
    links + rows
}

pub fn char_from_pos(h: HWND, x: i32, y: i32) -> i32 {
    let p = POINT { x, y };
    send(h, EM_CHARFROMPOS, 0, &p as *const _ as isize) as i32
}

/// The text on screen, as a range of `t` (the editor's text): from the start of the first
/// paragraph showing to just past the last character showing. Overlays that draw over
/// the text only need to look there, not through a whole long note. (They read just that
/// much of it: `screen_text`.)
#[cfg(test)]
pub fn on_screen(h: HWND, t: &[u16]) -> (usize, usize) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let first = char_from_pos(h, 0, 0).clamp(0, t.len() as i32);
    let last = char_from_pos(h, rc.right, rc.bottom).clamp(0, t.len() as i32);
    let a = para_start(t, first) as usize;
    let b = t[last as usize..].iter().position(|&c| c == CR).map_or(t.len(), |i| last as usize + i);
    (a.min(b), b)
}

/// The paragraphs on screen (`paras_around` them), and the range of them `on_screen`
/// gives: what the overlays draw over, read without copying the rest of a long note.
pub fn screen_text(h: HWND) -> (Span, usize, usize) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let len = text_len(h) as i32;
    let first = char_from_pos(h, 0, 0).clamp(0, len) as usize;
    let last = char_from_pos(h, rc.right, rc.bottom).clamp(0, len) as usize;
    let (first, last) = (first.min(last), last.max(first));
    // No further than this either side of what shows: a paragraph that starts that far
    // above the top has its first line (and its box) out of sight, and one paragraph of a
    // whole long note was read, and searched for emoji, at every key.
    const AROUND: usize = 512;
    let (lo, hi) = (first.saturating_sub(AROUND), (last + AROUND).min(len as usize));
    let s = Span { from: lo, t: raw_text(h, lo, hi), len: len as usize };
    // As `on_screen` has them: from the start of the paragraph (or table cell) showing
    // first, to the end of the one showing last.
    let a = s.from + para_start(&s.t, (first - s.from) as i32) as usize;
    let b = s.t[(last - s.from).min(s.t.len())..].iter().position(|&c| c == CR).map_or(if hi == len as usize { s.len } else { hi }, |i| last + i);
    (s, a.min(b), b)
}

pub fn pos_from_char(h: HWND, cp: i32) -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    send(h, EM_POSFROMCHAR, &mut p as *mut _ as usize, cp as isize);
    p
}

/// How far the content is scrolled down, in px.
pub fn scroll_y(h: HWND) -> i32 {
    let mut p = POINT { x: 0, y: 0 };
    send(h, EM_GETSCROLLPOS, 0, &mut p as *mut _ as isize);
    p.y
}

pub fn set_scroll_y(h: HWND, y: i32) {
    let p = POINT { x: 0, y: y.max(0) };
    send(h, EM_SETSCROLLPOS, 0, &p as *const _ as isize);
}

/// Where the paragraph holding `cp` starts. The end of a table cell ends a paragraph too.
pub fn para_start(t: &[u16], cp: i32) -> i32 {
    let cp = (cp.max(0) as usize).min(t.len());
    t[..cp].iter().rposition(|&c| c == CR || c == CELL).map(|i| i as i32 + 1).unwrap_or(0)
}

pub fn para_end(t: &[u16], cp: i32) -> i32 {
    let cp = (cp.max(0) as usize).min(t.len());
    t[cp..].iter().position(|&c| c == CR || c == CELL).map(|i| (cp + i) as i32).unwrap_or(t.len() as i32)
}

pub fn char_format(h: HWND, scope: usize) -> CharFormat2 {
    let mut cf = CharFormat2::new();
    send(h, EM_GETCHARFORMAT, scope, &mut cf as *mut _ as isize);
    cf
}

pub fn set_effect(h: HWND, mask: u32, on: bool) {
    let mut cf = CharFormat2::new();
    cf.mask = mask;
    cf.effects = if on { mask } else { 0 };
    send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
}

/// Align the paragraphs in the selection: `PFA_LEFT`, `PFA_CENTER` or `PFA_RIGHT`.
pub fn set_alignment(h: HWND, align: u16) {
    let mut pf = ParaFormat2::new();
    pf.mask = PFM_ALIGNMENT;
    pf.alignment = align;
    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
}

pub fn toggle_effect(h: HWND, mask: u32) {
    let cf = char_format(h, SCF_SELECTION);
    let on = cf.mask & mask != 0 && cf.effects & mask != 0;
    set_effect(h, mask, !on);
}

/// Bullets and numbers: the marker a hair in from the margin, the text in a column a
/// short step after it, wrapped lines aligned with the text.
/// Twips; 20 per point.
const BULLET_TEXT: i32 = 300;
const BULLET_START: i32 = 40;

/// Base look for an editor: font, colours, spacing, link detection.
pub fn setup(h: HWND) {
    send(h, EM_SETOPTIONS, ECOOP_OR, ECO_AUTOVSCROLL | ECO_SELECTIONBAR);
    // Off: we make addresses links ourselves (see `link_addresses`).
    send(h, EM_AUTOURLDETECT, 0, 0);
    // RichEdit takes typing only up to 32,767 characters unless told otherwise (a long note
    // silently stopped accepting keys). No limit while a note loads; then `limit_length`.
    send(h, EM_EXLIMITTEXT, 0, i32::MAX as isize);
    send(h, EM_SETTYPOGRAPHYOPTIONS, TO_ADVANCEDTYPOGRAPHY, TO_ADVANCEDTYPOGRAPHY as isize);
}

const CFM_WEIGHT: u32 = 0x0040_0000;

// ---------------------------------------------------------------- note typography

/// Text column of checklist items from the left margin, in twips: room for the circle
/// (a little wider than bullets, for the larger circle).
const LIST_TEXT: i32 = 345;
/// The checklist marker glyph: tiny, so it never makes its line taller than prose.
const MARK_SIZE: i32 = 180; // 9pt

/// One list nesting step, in twips, for the first `FULL_LEVELS` levels; the levels past
/// them step in by `DEEP_LEVEL`, half as far, so the deepest item still leaves a narrow
/// note room for its text. The deepest level (0 is at the margin): nine in all, as in Word.
const LEVEL: i32 = 320;
pub(crate) const FULL_LEVELS: i32 = 5;
const DEEP_LEVEL: i32 = 160;
const MAX_LEVEL: i32 = crate::common::store::MAX_LIST_LEVEL as i32;

/// How far a list paragraph at nesting `level` starts in from one at level 0, in twips.
fn level_indent(level: i32) -> i32 {
    let l = level.clamp(0, MAX_LEVEL);
    l.min(FULL_LEVELS) * LEVEL + (l - FULL_LEVELS).max(0) * DEEP_LEVEL
}

/// The nesting level whose indent is nearest `dx` twips in from level 0 (half way between
/// two, the deeper one, as rounding had it when every step was the same).
fn level_at(dx: i32) -> i32 {
    let mut best = 0;
    for l in 1..=MAX_LEVEL {
        if (level_indent(l) - dx).abs() <= (level_indent(best) - dx).abs() {
            best = l;
        }
    }
    best
}

/// Paste the clipboard's text, plain (through `replace_sel`, so selectors are kept out
/// and it's undoable).
pub fn paste_plain(h: HWND) {
    match crate::win::util::clipboard_text() {
        Some(t) if t.contains(['\u{FE0F}', '\u{FE0E}']) => replace_sel(h, &t.replace("\r\n", "\r").replace('\n', "\r")),
        _ => {
            send(h, EM_PASTESPECIAL, CF_UNICODETEXT, 0);
        }
    }
}

fn is_task(c: u16) -> bool {
    c == BOX || c == CHECKED
}

/// For tests: an RTF picture group of a grey `px_w` x `px_h` PNG, shown `w` x `h` twips.
#[cfg(test)]
pub fn test_picture(px_w: u32, px_h: u32, w: i32, h: i32) -> String {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10];
    let mut ihdr = px_w.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&px_h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grey
    chunk(&mut png, b"IHDR", &ihdr);
    // Each row: its filter byte, then its pixels; in stored (uncompressed) deflate blocks.
    let mut raw = Vec::new();
    for _ in 0..px_h {
        raw.push(0u8);
        raw.extend(std::iter::repeat_n(0x80u8, px_w as usize));
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, part) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        let n = part.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(part);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    let hex: String = png.iter().map(|b| format!("{b:02x}")).collect();
    format!("{{\\pict\\pngblip\\picw{px_w}\\pich{px_h}\\picwgoal{w}\\pichgoal{h} {hex}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};

    fn editor() -> HWND {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        h
    }

    /// A span is exactly the whole text's characters there, table marks, hidden link
    /// instructions and emoji marks included, from the paragraph mark before to the one
    /// after, however long the paragraphs.
    #[test]
    fn spans_are_the_text_around() {
        crate::common::i18n::init("en");
        let h = editor();
        let long = "word ".repeat(700);
        let md = format!(
            "Title\nplain line\n- [ ] task \u{1F389} party\n| a | b |\n| --- | --- |\n| 1 | 2 |\n> quoted\n```\ncode\n```\nsee [the doc](https://example.com/x) now\n{long}\n\nlast \u{2764}\u{FE0F}"
        );
        load_markdown(h, &md);
        let t = text(h);
        let n = t.len();
        assert!(t.contains(&CELL) && t.contains(&ROW_START), "the note has a table");
        assert_eq!(raw_text(h, 0, n), t);
        let mark_before = |a: usize| t[..a].iter().rposition(|&c| c == CR).unwrap_or(0);
        let mark_after = |b: usize| t[b..].iter().position(|&c| c == CR).map_or(n, |i| b + i + 1);
        for a in (0..=n).step_by(7).chain([0, n, n.saturating_sub(1)]) {
            for b in [a, a + 1, a + 40, a + 2000] {
                let s = paras_around(h, a, b);
                let b = b.min(n);
                assert_eq!((s.from, s.end(), s.len), (mark_before(a), mark_after(b), n), "around {a}..{b}");
                assert_eq!(s.t, t[s.from..s.end()], "around {a}..{b}");
                assert_eq!(s.get(a), t.get(a).copied().filter(|_| a < s.end()));
            }
        }
        // What's on screen, wherever the note is scrolled to.
        for y in [0, 40, 300, 2000, 100_000] {
            set_scroll_y(h, y);
            let (s, a, b) = screen_text(h);
            // As the whole paragraphs on screen are, but for a long one, which is cut a
            // little past what shows.
            let (pa, pb) = on_screen(h, &t);
            let mut rc: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(h, &mut rc) };
            let (first, last) = (char_from_pos(h, 0, 0) as usize, char_from_pos(h, rc.right, rc.bottom) as usize);
            assert!(pa <= a && a <= first.max(pa) && last.min(pb) <= b && b <= pb, "scrolled to {y}: {a}..{b} in {pa}..{pb}");
            assert!((a, b) == (pa, pb) || pb - pa > 512, "scrolled to {y}: short paragraphs are whole");
            assert_eq!(s.t, t[s.from..s.end()]);
            assert!(s.from < a.max(1) && b <= s.end());
        }
        unsafe { DestroyWindow(h) };
    }

    /// In one long paragraph (a 100,000-character note of a single line), what's on screen
    /// is read with a little either side, not the whole paragraph at every key.
    #[test]
    fn screen_text_of_a_long_paragraph_is_what_shows() {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 300, 200, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        for body in [format!("T\r{}\rend", "word ".repeat(20_000)), format!("{}\rend", "word ".repeat(20_000))] {
            set_sel(h, 0, -1);
            insert_text(h, &body);
            let t = text(h);
            let (s, a, b) = screen_text(h);
            let (first, last) = (char_from_pos(h, 0, 0) as usize, char_from_pos(h, 300, 200) as usize);
            assert!(a <= first && b >= last, "{a}..{b} holds {first}..{last}");
            assert!(s.t.len() < last - first + 1100, "{} read for {}", s.t.len(), last - first);
            assert_eq!(s.t, t[s.from..s.end()]);
            // The note's first character starts its first paragraph, its box's place.
            assert_eq!(a, 0);
        }
        unsafe { DestroyWindow(h) };
    }
}
