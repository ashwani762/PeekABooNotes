// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Character formatting: note typography, the title, body text and checklist styling.

use super::*;

/// Apply face, size and text colour to everything (and to newly typed text).
/// Every paragraph's reading rhythm: line spacing in 1/20ths of a line (1.2 lines), and
/// the space after it in twips.
pub(super) const LINE_MULT: i32 = 24;
pub(super) const PARA_AFTER: i32 = 50;

/// Give the paragraphs from `a` to `b` (-1: the end) the rhythm.
pub(super) fn base_spacing(h: HWND, a: i32, b: i32) {
    set_sel(h, a, b);
    let mut pf = ParaFormat2::new();
    pf.mask = PFM_LINESPACING | PFM_SPACEAFTER;
    pf.line_spacing_rule = 5; // multiple of single, in 1/20ths
    pf.line_spacing = LINE_MULT;
    pf.space_after = PARA_AFTER;
    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
}

pub fn apply_base_format(h: HWND, text_rgb: u32, bg_rgb: u32) {
    bulk(h, || base_format_now(h, text_rgb, bg_rgb));
}

fn base_format_now(h: HWND, text_rgb: u32, bg_rgb: u32) {
    let mut cf = CharFormat2::new();
    // Face, size and colour for everything; weight is left alone so bold survives.
    cf.mask = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_CHARSET;
    cf.height = body_size();
    cf.color = crate::win::util::cref(text_rgb);
    cf.charset = 1; // DEFAULT_CHARSET
    set_face(&mut cf, body_face());
    send(h, EM_SETCHARFORMAT, SCF_ALL, &cf as *const _ as isize);
    // No CFM_BOLD/CFM_WEIGHT here: RichEdit pushes default-format effects onto
    // existing text too, which un-bolded every note as it opened.
    send(h, EM_SETCHARFORMAT, SCF_DEFAULT, &cf as *const _ as isize);
    send(h, EM_SETBKGNDCOLOR, 0, crate::win::util::cref(bg_rgb) as isize);

    // Comfortable reading rhythm for every paragraph, without moving the caret.
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    base_spacing(h, 0, -1);
    normalize_lists(h);
    fit_pictures(h);
    // The body font just went over everything, file chips' paperclips included.
    let len = text(h).len() as i32;
    style_clips(h, 0, len, crate::win::ui::note::icon_face());
    set_sel(h, a, b);
    send(h, EM_HIDESELECTION, 0, 0);
    style_boxes(h);
}

thread_local! {
    /// (font, size) choices from Settings.
    static TYPE: std::cell::Cell<(u8, u8)> = const { std::cell::Cell::new((0, 1)) };
    /// Width of a checklist marker (box glyph + space) in twips, for the current type.
    static MARK_W: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    /// A body line's height in twips, for a DPI: (dpi, twips).
    static LINE_H: std::cell::Cell<Option<(i32, (i32, i32))>> = const { std::cell::Cell::new(None) };
}

/// Choose the notes' typeface (0 system, 1 serif, 2 mono, 3 handwriting) and size
/// (0 small, 1 medium, 2 large). Re-apply to open editors with `apply_base_format`.
pub fn set_typography(font: u8, size: u8) {
    TYPE.with(|t| t.set((font.min(3), size.min(2))));
    MARK_W.with(|m| m.set(None));
    LINE_H.with(|m| m.set(None));
}

/// Body size in twips.
pub fn body_size() -> i32 {
    [210, 230, 260][TYPE.with(|t| t.get().1) as usize]
}

/// Title size in twips: about 1.3x the body.
pub fn title_size() -> i32 {
    [270, 300, 340][TYPE.with(|t| t.get().1) as usize]
}

/// Which typeface is chosen (for caches keyed by it).
pub fn font_index() -> u8 {
    TYPE.with(|t| t.get().0)
}

/// Note fonts for custom-drawn previews (All Notes card, deck preview, ghost), matching what
/// the editor shows: (title, body) at scale `s`.
pub fn preview_fonts(s: f32) -> (crate::win::render::dw::Font, crate::win::render::dw::Font) {
    let px = |twips: i32| twips as f32 / 20.0 * 96.0 / 72.0 * s;
    let title = match font_index() {
        0 => crate::win::render::dw::Font::new(px(title_size()), 600, crate::win::theme::FACE_DISPLAY),
        3 => crate::win::render::dw::Font::new(px(title_size()), 400, "Ink Free"),
        _ => crate::win::render::dw::Font::new(px(title_size()), 700, body_face()),
    };
    (title, crate::win::render::dw::Font::new(px(body_size()), 400, body_face()))
}

pub fn body_face() -> &'static str {
    match TYPE.with(|t| t.get().0) {
        1 => "Georgia",
        2 => "Consolas",
        3 => "Ink Free",
        _ => crate::win::theme::FACE_TEXT,
    }
}

/// Title face, and whether it needs synthetic bold (the system face has a named semibold).
pub fn title_face() -> (&'static str, bool) {
    match TYPE.with(|t| t.get().0) {
        0 => (crate::win::theme::FACE_TITLE, false),
        3 => ("Ink Free", false),
        _ => (body_face(), true),
    }
}

pub(super) fn mark_w() -> i32 {
    if let Some(w) = MARK_W.with(|m| m.get()) {
        return w;
    }
    let px = |twips: i32| twips as f32 / 20.0 * 96.0 / 72.0;
    let glyph = crate::win::render::dw::Font::new(px(MARK_SIZE), 400, "Segoe UI Symbol").measure("\u{2610}").0;
    let space = crate::win::render::dw::Font::new(px(body_size()), 400, body_face()).measure(" ").0;
    let w = (glyph + space) * 15; // px at 96 dpi -> twips
    MARK_W.with(|m| m.set(Some(w)));
    w
}

/// A body line in twips, as note editors lay it out: (single, at the reading rhythm).
/// Measured in a hidden editor set up like them (font metrics alone don't say). `dpi`
/// keys the cache; twips hardly depend on it.
pub(super) fn body_line_h(dpi: i32) -> (i32, i32) {
    if let Some((d, h)) = LINE_H.with(|m| m.get())
        && d == dpi
    {
        return h;
    }
    let single = measure_body_line(0, 0).unwrap_or(body_size() * 6 / 5);
    let rhythm = measure_body_line(5, LINE_MULT).unwrap_or(single * LINE_MULT / 20);
    LINE_H.with(|m| m.set(Some((dpi, (single, rhythm)))));
    (single, rhythm)
}

/// Two body lines in a scratch editor with line spacing `rule`/`value` and no space
/// after: how far apart they are, in twips.
fn measure_body_line(rule: u8, value: i32) -> Option<i32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};
    let h = unsafe { CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 200, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
    if h.is_null() {
        return None;
    }
    setup(h);
    let mut cf = CharFormat2::new();
    cf.mask = CFM_FACE | CFM_SIZE | CFM_CHARSET;
    cf.height = body_size();
    cf.charset = 1; // DEFAULT_CHARSET
    set_face(&mut cf, body_face());
    send(h, EM_SETCHARFORMAT, SCF_ALL, &cf as *const _ as isize);
    insert_text(h, "a\rb");
    set_sel(h, 0, -1);
    let mut pf = ParaFormat2::new();
    pf.mask = PFM_LINESPACING | PFM_SPACEAFTER;
    pf.line_spacing_rule = rule;
    pf.line_spacing = value;
    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    let px = pos_from_char(h, 2).y - pos_from_char(h, 0).y;
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) }.max(96) as i32;
    unsafe { DestroyWindow(h) };
    (px > 0).then(|| px * 1440 / dpi)
}

pub(super) fn set_face(cf: &mut CharFormat2, face: &str) {
    cf.face = [0; 32];
    for (i, c) in face.encode_utf16().take(31).enumerate() {
        cf.face[i] = c;
    }
}

pub(super) fn face_is(cf: &CharFormat2, face: &str) -> bool {
    crate::win::util::from_wide(&cf.face).eq_ignore_ascii_case(face)
}

/// Make the first paragraph the title (Display face, semibold, larger). Only touches
/// formatting when it is not already styled, so typing in the title adds no undo steps.
pub fn style_title(h: HWND) {
    // Follows from the text: no step for Ctrl+Z (which would otherwise first take back
    // the title's look, then the letter typed).
    unrecorded(h, || style_title_now(h));
}

fn style_title_now(h: HWND) {
    let t = text(h);
    let end = t.iter().position(|&c| c == CR).unwrap_or(t.len());
    if end == 0 || t[..end].iter().any(|&c| is_task(c)) {
        return;
    }
    let (face, bold) = title_face();
    // Already the title: leave the selection be, so typing stays one undo step.
    if font_is(h, 0, end as i32, face, title_size()) {
        return;
    }
    let (a, b) = sel(h);
    // Moving the selection forgets a pending caret format (Ctrl+B, then type), so keep it.
    let pending = (a == b).then(|| char_format(h, SCF_SELECTION));
    send(h, EM_HIDESELECTION, 1, 0);
    set_sel(h, 0, end as i32);
    let cf = char_format(h, SCF_SELECTION);
    let want = CFM_FACE | CFM_SIZE;
    let styled = cf.mask & want == want && cf.height == title_size() && face_is(&cf, face);
    if !styled && numbering(h) == 0 {
        let mut n = CharFormat2::new();
        n.mask = CFM_FACE | CFM_SIZE | CFM_WEIGHT | CFM_BOLD;
        n.height = title_size();
        n.weight = if bold { 700 } else { 400 };
        n.effects = if bold { CFM_BOLD } else { 0 };
        set_face(&mut n, face);
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &n as *const _ as isize);
    }
    set_sel(h, a, b);
    if let Some(p) = pending.filter(|_| styled || a as usize > end) {
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &p as *const _ as isize);
    }
    send(h, EM_HIDESELECTION, 0, 0);
}

/// Format for text typed at the caret / in the selection: regular body text.
pub fn set_body_format(h: HWND) {
    let mut n = CharFormat2::new();
    n.mask = CFM_FACE | CFM_SIZE | CFM_BOLD;
    n.height = body_size();
    set_face(&mut n, body_face());
    send(h, EM_SETCHARFORMAT, SCF_SELECTION, &n as *const _ as isize);
}

thread_local! {
    /// Per-editor checklist colours: (placeholder glyph = paper, completed text, normal text).
    static BOX_COLORS: std::cell::RefCell<std::collections::HashMap<isize, (u32, u32, u32)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

pub fn set_box_colors(h: HWND, ink: u32, done: u32, text: u32) {
    BOX_COLORS.with(|m| m.borrow_mut().insert(h as isize, (ink, done, text)));
}

/// Text typed at the caret (or the selection) takes a completed item's look: struck
/// through, in the secondary colour.
pub fn set_done_format(h: HWND) {
    let done = BOX_COLORS.with(|m| m.borrow().get(&(h as isize)).copied()).map_or(0x888888, |c| c.1);
    let mut cf = CharFormat2::new();
    cf.mask = CFM_COLOR | CFM_STRIKEOUT;
    cf.effects = CFM_STRIKEOUT;
    cf.color = crate::win::util::cref(done);
    send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
}

/// Checklists. The box glyph is a placeholder painted in the
/// paper colour (`checks` draws the visible circle over it); completed items fade to the
/// secondary colour and are struck through, starting at the text, not the gap.
pub fn style_boxes(h: HWND) {
    // Each box is formatted through the selection: hold the view still meanwhile. The
    // looks follow from the text, so they're no step for Ctrl+Z (which would otherwise
    // first undo the formatting of a new item, selecting its box, before the item).
    still(h, || unrecorded(h, || style_boxes_now(h)));
}

fn style_boxes_now(h: HWND) {
    let t = text(h);
    if !t.iter().any(|&c| is_task(c)) {
        return;
    }
    let (ink, done, normal) = BOX_COLORS.with(|m| m.borrow().get(&(h as isize)).copied()).unwrap_or((0x444444, 0x888888, 0x1D1D1F));
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let mut boxf = CharFormat2::new();
    boxf.mask = CFM_SIZE | CFM_FACE | CFM_COLOR | CFM_STRIKEOUT;
    // Smaller than body text, so checklist lines keep the same line height as prose;
    // the circle drawn over it is sized from the body text, not from this glyph.
    boxf.height = MARK_SIZE;
    boxf.color = crate::win::util::cref(ink);
    // A hanging indent puts the text in the same column as bullets and numbers (and
    // wrapped lines under it), leaving room on the left for the circle.
    let mut indent = ParaFormat2::new();
    indent.mask = PFM_STARTINDENT | PFM_OFFSET | PFM_LINESPACING | PFM_SPACEAFTER;
    indent.offset = mark_w();
    indent.start_indent = (LIST_TEXT - indent.offset).max(0);
    // At least a single body line: an item that is only its box would be sized from the
    // small glyph (a trailing space, or a quote's end mark, doesn't count), and its circle
    // is body-sized. The rest of the reading rhythm goes after the line, where prose has
    // it, so the text sits in its line as prose text does (spacing that adds room above
    // the text pushed it a few px lower) and lines keep prose's pitch. At least, not
    // exactly (rule 4): a line holding a picture grows to it.
    let (single, rhythm) = body_line_h(unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) } as i32);
    indent.line_spacing_rule = 3;
    indent.line_spacing = single;
    indent.space_after = PARA_AFTER + (rhythm - single).max(0);
    set_face(&mut boxf, "Segoe UI Symbol");
    for (i, &c) in t.iter().enumerate() {
        if !is_task(c) || (i > 0 && t[i - 1] != CR) {
            continue; // only markers at the start of a paragraph
        }
        let checked = c == CHECKED;
        // Only the box itself is the (large, invisible) placeholder; everything after it
        // is body type, so text typed on the line never inherits the placeholder's size.
        set_sel(h, i as i32, i as i32 + 1);
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &boxf as *const _ as isize);
        let mut cur = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut cur as *mut _ as isize);
        // Already styled, or not styled yet (no hanging offset: imported notes are built in
        // the worker editor, which never styles them): either way the start indent carries
        // the nesting. Any other offset belongs to a former bullet or number.
        let level = if cur.offset == indent.offset || cur.offset == 0 { level_of(cur.start_indent, indent.start_indent) } else { 0 };
        let mut nested = indent;
        nested.start_indent += level * LEVEL;
        send(h, EM_SETPARAFORMAT, 0, &nested as *const _ as isize);
        let gap = if t.get(i + 1) == Some(&(b' ' as u16)) { 2 } else { 1 };
        let end = para_end(&t, i as i32);
        if end > i as i32 + 1 {
            let mut body = CharFormat2::new();
            // Strikethrough is left as it is: a word struck out in an open item stays so
            // (ticking strikes the line, unticking clears it, in `toggle_check`).
            body.mask = CFM_COLOR | CFM_SIZE | CFM_FACE;
            body.height = body_size();
            set_face(&mut body, body_face());
            body.color = crate::win::util::cref(normal);
            set_sel(h, i as i32 + 1, end);
            send(h, EM_SETCHARFORMAT, SCF_SELECTION, &body as *const _ as isize);
            // The gap after the box is never struck: the line starts at the text.
            if gap == 2 {
                let mut plain = CharFormat2::new();
                plain.mask = CFM_STRIKEOUT;
                set_sel(h, i as i32 + 1, i as i32 + 2);
                send(h, EM_SETCHARFORMAT, SCF_SELECTION, &plain as *const _ as isize);
            }
            // Completed items: secondary colour, struck through from the text (not the gap).
            if checked && end > i as i32 + gap {
                body.mask = CFM_COLOR | CFM_STRIKEOUT;
                body.color = crate::win::util::cref(done);
                body.effects = CFM_STRIKEOUT;
                set_sel(h, i as i32 + gap, end);
                send(h, EM_SETCHARFORMAT, SCF_SELECTION, &body as *const _ as isize);
            }
        }
    }
    set_sel(h, a, b);
    send(h, EM_HIDESELECTION, 0, 0);}
