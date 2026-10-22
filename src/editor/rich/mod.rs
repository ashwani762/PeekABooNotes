// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! RichEdit (msftedit.dll) bindings and helpers.
//!
//! windows-sys no longer ships the RichEdit headers, so the handful of messages and
//! structs we use are declared here. richedit.h uses 4-byte packing on Win32.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

mod format;
mod lists;
mod markdown;
pub use format::*;
pub use lists::*;
pub use markdown::*;

pub const CLASS: &str = "RICHEDIT50W";

const WM_USER: u32 = 0x0400;
pub const EM_GETSEL: u32 = 0x00B0;
pub const EM_SETSEL: u32 = 0x00B1;
pub const EM_REPLACESEL: u32 = 0x00C2;
pub const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
pub const EM_POSFROMCHAR: u32 = 0x00D6;
pub const EM_CHARFROMPOS: u32 = 0x00D7;
pub const EM_REDO: u32 = 0x0454;
pub const EM_GETCHARFORMAT: u32 = WM_USER + 58;
pub const EM_GETPARAFORMAT: u32 = WM_USER + 61;
pub const EM_HIDESELECTION: u32 = WM_USER + 63;
pub const EM_PASTESPECIAL: u32 = WM_USER + 64;
pub const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
pub const EM_SETCHARFORMAT: u32 = WM_USER + 68;
pub const EM_SETEVENTMASK: u32 = WM_USER + 69;
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
pub const CFM_SIZE: u32 = 0x8000_0000;
pub const CFM_COLOR: u32 = 0x4000_0000;
pub const CFM_FACE: u32 = 0x2000_0000;
pub const CFM_CHARSET: u32 = 0x0800_0000;
pub const SCF_DEFAULT: usize = 0x0000;
pub const SCF_SELECTION: usize = 0x0001;
pub const SCF_ALL: usize = 0x0004;

pub const PFM_STARTINDENT: u32 = 0x0001;
pub const PFM_OFFSET: u32 = 0x0004;
pub const PFM_NUMBERING: u32 = 0x0020;
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
const TO_ADVANCEDTYPOGRAPHY: usize = 1;
const CF_UNICODETEXT: usize = 13;
const GT_DEFAULT: u32 = 0;
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
    send(h, EM_STREAMOUT, SF_RTF as usize, &mut es as *mut _ as isize);
    String::from_utf8_lossy(&out).into_owned()
}

pub fn set_rtf(h: HWND, rtf: &str) {
    let rtf = strip_weights(rtf);
    let mut cur = InCursor { data: rtf.as_bytes(), pos: 0 };
    let mut es = EditStream { cookie: &mut cur as *mut _ as usize, error: 0, callback: in_cb };
    send(h, EM_STREAMIN, SF_RTF as usize, &mut es as *mut _ as isize);
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

/// Full text with bare `\r` paragraph marks; indices match RichEdit character positions.
pub fn text(h: HWND) -> Vec<u16> {
    let gtl = GetTextLengthEx { flags: GTL_PRECISE | GTL_NUMCHARS, codepage: CP_UTF16 };
    let len = send(h, EM_GETTEXTLENGTHEX, &gtl as *const _ as usize, 0).max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    let gt = GetTextEx {
        cb: ((len + 1) * 2) as u32,
        flags: GT_DEFAULT,
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

pub fn sel(h: HWND) -> (i32, i32) {
    let (mut a, mut b) = (0u32, 0u32);
    send(h, EM_GETSEL, &mut a as *mut _ as usize, &mut b as *mut _ as isize);
    (a as i32, b as i32)
}

pub fn set_sel(h: HWND, a: i32, b: i32) {
    send(h, EM_SETSEL, a as usize, b as isize);
}

pub fn replace_sel(h: HWND, s: &str) {
    let w = crate::platform::util::wide(s);
    send(h, EM_REPLACESEL, 1, w.as_ptr() as isize);
}

/// Replace the selection with text, bypassing the subclass/undo tricks (used for "\r" inserts).
pub fn insert_text(h: HWND, s: &str) {
    let w = crate::platform::util::wide(s);
    let st = SetTextEx { flags: ST_SELECTION | ST_DEFAULT, codepage: CP_UTF16 };
    send(h, EM_SETTEXTEX, &st as *const _ as usize, w.as_ptr() as isize);
}

pub fn char_from_pos(h: HWND, x: i32, y: i32) -> i32 {
    let p = POINT { x, y };
    send(h, EM_CHARFROMPOS, 0, &p as *const _ as isize) as i32
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

pub fn para_start(t: &[u16], cp: i32) -> i32 {
    let cp = (cp.max(0) as usize).min(t.len());
    t[..cp].iter().rposition(|&c| c == CR).map(|i| i as i32 + 1).unwrap_or(0)
}

pub fn para_end(t: &[u16], cp: i32) -> i32 {
    let cp = (cp.max(0) as usize).min(t.len());
    t[cp..].iter().position(|&c| c == CR).map(|i| (cp + i) as i32).unwrap_or(t.len() as i32)
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
    send(h, EM_AUTOURLDETECT, 1, 0);
    send(h, EM_SETTYPOGRAPHYOPTIONS, TO_ADVANCEDTYPOGRAPHY, TO_ADVANCEDTYPOGRAPHY as isize);
}

const CFM_WEIGHT: u32 = 0x0040_0000;

// ---------------------------------------------------------------- note typography

/// Text column of checklist items from the left margin, in twips: room for the circle
/// (a little wider than bullets, for the larger circle).
const LIST_TEXT: i32 = 345;
/// The checklist marker glyph: tiny, so it never makes its line taller than prose.
const MARK_SIZE: i32 = 180; // 9pt

/// One list nesting step, in twips, and the deepest level.
const LEVEL: i32 = 320;
const MAX_LEVEL: i32 = 5;

pub fn paste_plain(h: HWND) {
    send(h, EM_PASTESPECIAL, CF_UNICODETEXT, 0);
}

fn is_task(c: u16) -> bool {
    c == BOX || c == CHECKED
}

