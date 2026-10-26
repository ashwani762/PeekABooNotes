// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Paragraph lists: bullets, numbers, checklist indents and nesting levels.

use super::*;

pub fn numbering(h: HWND) -> u16 {
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    pf.numbering
}

pub fn set_numbering(h: HWND, kind: u16) {
    let mut pf = ParaFormat2::new();
    pf.mask = PFM_NUMBERING | PFM_OFFSET | PFM_STARTINDENT | PFM_NUMBERINGSTYLE | PFM_NUMBERINGSTART | PFM_NUMBERINGTAB;
    pf.numbering = kind;
    list_indent(&mut pf, kind != 0);
    pf.numbering_start = 1;
    pf.numbering_style = PFNS_PERIOD;
    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
}

/// The caret's paragraph, a bullet or number, about to become a checklist item: no
/// longer numbered, and indented as an item of its nesting level, which `style_boxes`
/// then keeps (dropping the numbering alone would take it back to the margin).
pub fn numbering_to_task(h: HWND) {
    let mut cur = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut cur as *mut _ as isize);
    if cur.numbering == 0 {
        return;
    }
    let level = level_of(cur.start_indent, BULLET_START);
    set_numbering(h, 0);
    let mut pf = ParaFormat2::new();
    pf.mask = PFM_STARTINDENT | PFM_OFFSET;
    pf.start_indent = list_base(true) + level * LEVEL;
    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
}

pub(super) fn list_indent(pf: &mut ParaFormat2, on: bool) {
    pf.offset = if on { BULLET_TEXT - BULLET_START } else { 0 };
    pf.start_indent = if on { BULLET_START } else { 0 };
    pf.numbering_tab = (BULLET_TEXT - BULLET_START) as u16;
}

/// Bring every bulleted / numbered paragraph to the current list indents (notes saved
/// by older versions used a wider column).
pub(super) fn normalize_lists(h: HWND) {
    let t = text(h);
    let mut ps = 0usize;
    loop {
        set_sel(h, ps as i32, ps as i32);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        if pf.numbering != 0 && pf.offset != BULLET_TEXT - BULLET_START {
            let mut n = ParaFormat2::new();
            n.mask = PFM_OFFSET | PFM_STARTINDENT | PFM_NUMBERINGTAB;
            list_indent(&mut n, true);
            send(h, EM_SETPARAFORMAT, 0, &n as *const _ as isize);
        }
        match t[ps..].iter().position(|&c| c == CR) {
            Some(i) if ps + i + 1 < t.len() => ps += i + 1,
            _ => break,
        }
    }
}

pub fn toggle_numbering(h: HWND, kind: u16) {
    let cur = numbering(h);
    let same = cur == kind || (is_bullet(cur) && is_bullet(kind));
    set_numbering(h, if same { 0 } else { kind });
}

/// Bullets by nesting level, as in Notes: • then ◦ then ▪, and round again.
const BULLETS: [u16; 3] = [PFN_BULLET, 0x25E6, 0x25AA];

/// Whether `numbering` is a bullet (of any level) rather than a number.
pub fn is_bullet(numbering: u16) -> bool {
    BULLETS.contains(&numbering)
}

/// The bullet for nesting level `level`.
pub fn bullet_for(level: i32) -> u16 {
    BULLETS[level.max(0) as usize % BULLETS.len()]
}

/// Give every bulleted paragraph the bullet for its level. RichEdit draws any character
/// as a bullet, but its RTF keeps only the plain one: this puts the others back once a
/// note is loaded.
pub fn style_bullets(h: HWND) {
    let t = text(h);
    let (a, b) = sel(h);
    let mut ps = 0usize;
    loop {
        set_sel(h, ps as i32, ps as i32);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        if is_bullet(pf.numbering) {
            let want = bullet_for(level_of(pf.start_indent, BULLET_START));
            if pf.numbering != want {
                let mut n = ParaFormat2::new();
                n.mask = PFM_NUMBERING;
                n.numbering = want;
                send(h, EM_SETPARAFORMAT, 0, &n as *const _ as isize);
            }
        }
        match t[ps..].iter().position(|&c| c == CR) {
            Some(i) if ps + i + 1 < t.len() => ps += i + 1,
            _ => break,
        }
    }
    set_sel(h, a, b);
}

/// First-line indent of a list paragraph at nesting level 0: bullets/numbers or checklists.
pub(super) fn list_base(checklist: bool) -> i32 {
    if checklist { (LIST_TEXT - mark_w()).max(0) } else { BULLET_START }
}

pub(super) fn level_of(start_indent: i32, base: i32) -> i32 {
    ((start_indent - base) as f32 / LEVEL as f32).round().clamp(0.0, MAX_LEVEL as f32) as i32
}

/// Nesting level of the list paragraph holding `cp`, or None if it isn't a list item.
pub(super) fn para_level(h: HWND, t: &[u16], cp: i32) -> Option<(i32, bool)> {
    let ps = para_start(t, cp);
    set_sel(h, ps, ps);
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    let checklist = t.get(ps as usize).copied().is_some_and(is_task);
    if !checklist && pf.numbering == 0 {
        return None;
    }
    Some((level_of(pf.start_indent, list_base(checklist)), checklist))
}

/// Numbered items at `level` above paragraph `ps` in the same list, stopping at the
/// list's start or at a shallower (parent) item; and the number the first of them has
/// (a list can start at 5), so the item at `ps` is that plus how many.
pub(super) fn siblings_before(h: HWND, t: &[u16], ps: i32, level: i32) -> (usize, u16) {
    let mut n = 0;
    let mut first = 1;
    let mut p = ps;
    while p > 0 {
        p = para_start(t, p - 1);
        // A quote under an item belongs to it: the count goes on past it.
        if t.get(p as usize) == Some(&ROW_END)
            && let Some((Block::Quote, rows)) = block_at(h, t, p)
            && quote_level(h, rows[0]) > 0
        {
            p = rows[0].start as i32;
            continue;
        }
        set_sel(h, p, p);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        if !matches!(pf.numbering, PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN) {
            break;
        }
        let l = level_of(pf.start_indent, BULLET_START);
        if l < level {
            break;
        }
        if l == level {
            n += 1;
            // The topmost one, in the end: the list's first item, which has its start.
            first = pf.numbering_start;
        }
    }
    (n, first)
}

/// Nesting level of the caret's paragraph, if it is a list item.
pub fn list_level(h: HWND) -> Option<i32> {
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let r = para_level(h, &text(h), a).map(|(l, _)| l);
    set_sel(h, a, b);
    send(h, EM_HIDESELECTION, 0, 0);
    r
}

/// Indent (`delta` = 1) or outdent (-1) every list paragraph in the selection, like Tab
/// and Shift+Tab in Notes. Returns false when the caret isn't in a list (Tab is a tab).
pub fn nest(h: HWND, delta: i32) -> bool {
    let (a, b) = sel(h);
    let t = text(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let mut starts = vec![para_start(&t, a)];
    for (i, &c) in t.iter().enumerate().take(b.max(a) as usize).skip(a as usize) {
        if c == CR && i + 1 < t.len() {
            starts.push(i as i32 + 1);
        }
    }
    let mut any = false;
    for ps in starts {
        let Some((level, checklist)) = para_level(h, &t, ps) else { continue };
        any = true;
        let next = (level + delta).clamp(0, MAX_LEVEL);
        let mut pf = ParaFormat2::new();
        pf.mask = PFM_STARTINDENT;
        pf.start_indent = list_base(checklist) + next * LEVEL;
        // Numbered lists count each level on its own, styled like Notes: 1. a. i.
        let mut cur = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut cur as *mut _ as isize);
        // Bullets change with the level too: • ◦ ▪.
        if is_bullet(cur.numbering) {
            pf.mask |= PFM_NUMBERING;
            pf.numbering = bullet_for(next);
        }
        if matches!(cur.numbering, PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN) {
            pf.mask |= PFM_NUMBERING | PFM_NUMBERINGSTYLE | PFM_NUMBERINGSTART;
            pf.numbering = [PFN_ARABIC, PFN_LCLETTER, PFN_LCROMAN][next as usize % 3];
            pf.numbering_style = PFNS_PERIOD;
            // Continue the count of this level across the nested items in between
            // (RichEdit would restart it after a change of style).
            let (before, first) = siblings_before(h, &t, ps, next);
            pf.numbering_start = if before > 0 { first + before as u16 } else { 1 };
            set_sel(h, ps, ps); // the count walked the selection up the list
        }
        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    }
    set_sel(h, a, b);
    send(h, EM_HIDESELECTION, 0, 0);
    any
}

/// The caret's paragraph lost its checklist marker but kept the hanging indent: reset.
pub fn fix_caret_indent(h: HWND) {
    fix_caret_indent_in(h, &text(h));
}

/// `fix_caret_indent`, with the editor's text (`t`) in hand.
pub(super) fn fix_caret_indent_in(h: HWND, t: &[u16]) {
    let (a, _) = sel(h);
    let ps = para_start(t, a) as usize;
    if t.get(ps).copied().is_some_and(is_task) {
        return;
    }
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    let base = list_base(true);
    if pf.numbering == 0 && pf.offset == mark_w() && pf.start_indent >= base && (pf.start_indent - base) % LEVEL == 0 {
        // Follows from the box having gone: Ctrl+Z brings the box back, not this first.
        unrecorded(h, || {
            pf = ParaFormat2::new();
            pf.mask = PFM_STARTINDENT | PFM_OFFSET | PFM_LINESPACING | PFM_SPACEAFTER;
            pf.line_spacing_rule = 5;
            pf.line_spacing = LINE_MULT;
            pf.space_after = PARA_AFTER;
            send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
        });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, WS_POPUP};

    fn editor() -> HWND {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        h
    }

    /// The bullet of each paragraph after the title, 0 for none.
    fn bullets(h: HWND) -> Vec<u16> {
        let t = text(h);
        let mut out = Vec::new();
        for (i, &c) in t.iter().enumerate() {
            if c == CR && i + 1 < t.len() {
                set_sel(h, i as i32 + 1, i as i32 + 1);
                out.push(numbering(h));
            }
        }
        out
    }

    #[test]
    fn bullets_by_level() {
        let h = editor();
        let md = "T\n- one\n  - two\n    - three\n      - four\n- five";
        load_markdown(h, md);
        assert_eq!(bullets(h), vec![PFN_BULLET, 0x25E6, 0x25AA, PFN_BULLET, PFN_BULLET]);
        assert_eq!(to_markdown(h, true), format!("# {md}"));
        // Tab and Shift+Tab change the mark with the level.
        let t = String::from_utf16_lossy(&text(h));
        let five = t.find("five").unwrap() as i32;
        set_sel(h, five, five);
        nest(h, 1);
        assert_eq!(bullets(h)[4], 0x25E6);
        nest(h, -1);
        assert_eq!(bullets(h)[4], PFN_BULLET);
        // A bulleted list is one kind, whatever its level: the toolbar takes it off.
        set_sel(h, five, five);
        nest(h, 1);
        toggle_numbering(h, PFN_BULLET);
        assert_eq!(bullets(h)[4], 0);
        // RTF keeps only the plain bullet; loading puts the others back.
        load_markdown(h, md);
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        style_bullets(h2);
        assert_eq!(bullets(h2), vec![PFN_BULLET, 0x25E6, 0x25AA, PFN_BULLET, PFN_BULLET]);
    }
}
