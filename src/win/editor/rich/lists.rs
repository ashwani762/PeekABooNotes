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
    pf.start_indent = list_base(true) + level_indent(level);
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

/// The nesting level of a list paragraph that starts `start_indent` in, where one at level
/// 0 starts at `base`.
pub(super) fn level_of(start_indent: i32, base: i32) -> i32 {
    level_at(start_indent - base)
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
        // A quote or code block under an item belongs to it: the count goes on past it.
        if let Some(row) = block_under_item_ending(h, t, p) {
            p = row.start as i32;
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

/// The first row of the quote or code block under a list item whose closing paragraph (its
/// U+FFFB) starts at `p`, if one does.
fn block_under_item_ending(h: HWND, t: &[u16], p: i32) -> Option<Row> {
    if t.get(p.max(0) as usize) != Some(&ROW_END) {
        return None;
    }
    match block_at(h, t, p) {
        Some((Block::Quote | Block::Code, rows)) if quote_level(h, rows[0]) > 0 => Some(rows[0]),
        _ => None,
    }
}

/// The numbered item at `cp`, right after a quote or code block under the item before it,
/// counts on past the block, as its Markdown does ("1. one", code under it, "2. two").
/// RichEdit starts its count again after a block (a table row), at the item's own start,
/// and goes on from an item only to the next one of the same start: so the item, and the
/// items after it that RichEdit counted on from it, start at the number it has. (A note
/// opened from Markdown has that number at every item already.)
pub fn count_on(h: HWND, cp: i32) {
    let t = text(h);
    let ps = para_start(&t, cp);
    if ps < 1 || block_under_item_ending(h, &t, para_start(&t, ps - 1)).is_none() {
        return;
    }
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let numbered = |p: usize| {
        set_sel(h, p as i32, p as i32);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        matches!(pf.numbering, PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN).then(|| (level_of(pf.start_indent, BULLET_START), pf.numbering_start))
    };
    if let Some((level, old)) = numbered(ps as usize) {
        let (before, first) = siblings_before(h, &t, ps, level);
        let want = if before > 0 { first.saturating_add(before as u16) } else { old };
        let mut p = ps as usize;
        while want != old && numbered(p) == Some((level, old)) {
            let mut pf = ParaFormat2::new();
            pf.mask = PFM_NUMBERINGSTART;
            pf.numbering_start = want;
            send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
            match t[p..].iter().position(|&c| c == CR) {
                Some(i) if p + i + 1 < t.len() => p += i + 1,
                _ => break,
            }
        }
    }
    set_sel(h, a, b);
    show_selection(h);
}

/// Nesting level of the caret's paragraph, if it is a list item.
pub fn list_level(h: HWND) -> Option<i32> {
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let r = para_level(h, &text(h), a).map(|(l, _)| l);
    set_sel(h, a, b);
    show_selection(h);
    r
}

/// Whether the paragraph before the one at `cp` is a list item (`t`: the editor's text).
pub fn item_before(h: HWND, t: &[u16], cp: i32) -> bool {
    if cp <= 0 {
        return false;
    }
    let (a, b) = sel(h);
    send(h, EM_HIDESELECTION, 1, 0);
    let r = para_level(h, t, para_start(t, cp - 1)).is_some();
    set_sel(h, a, b);
    show_selection(h);
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
        pf.start_indent = list_base(checklist) + level_indent(next);
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
    show_selection(h);
    any
}

/// Backspace at the very start of a nested list item's text (just after a checklist item's
/// box): the item steps out a level, as Shift+Tab does, rather than losing its bullet.
/// False at the top level, where Backspace takes the bullet off, and anywhere else.
pub fn backspace_outdents(h: HWND) -> bool {
    let (a, b) = sel(h);
    if a != b || a <= 0 {
        return false;
    }
    // The caret's paragraph tells whether it's at an item's start; only then the rest.
    let s = paras_around(h, a as usize, a as usize);
    let ps = s.from + para_start(&s.t, a - s.from as i32) as usize;
    let gap = match s.get(ps) {
        Some(c) if is_task(c) => 1 + (s.get(ps + 1) == Some(b' ' as u16)) as usize,
        _ => 0,
    };
    a as usize == ps + gap && list_level(h).is_some_and(|l| l > 0) && nest(h, -1)
}

/// Shift+Tab on lines that aren't a list, a quote, code or a table: a tab at the start of
/// each line in the selection goes, and nothing else happens (RichEdit typed a tab). True
/// when one went.
pub fn untab_lines(h: HWND) -> bool {
    let (a, b) = sel(h);
    let t = text(h);
    let mut starts = vec![para_start(&t, a)];
    for (i, &c) in t.iter().enumerate().take(b.max(a) as usize).skip(a as usize) {
        if c == CR && i + 1 < t.len() {
            starts.push(i as i32 + 1);
        }
    }
    starts.retain(|&ps| t.get(ps as usize) == Some(&(b'\t' as u16)));
    if starts.is_empty() {
        return false;
    }
    still(h, || {
        let (mut na, mut nb) = (a, b);
        // Last first, so the lines before keep their places.
        for &ps in starts.iter().rev() {
            set_sel(h, ps, ps + 1);
            replace_sel(h, "");
            let moved = |p: i32| if p > ps { p - 1 } else { p };
            na = moved(na);
            nb = moved(nb);
        }
        set_sel(h, na, nb);
    });
    true
}

/// The caret's paragraph lost its checklist marker but kept the hanging indent: reset.
/// Or it's an item that lost its indent: what followed it deleted up to the note's end
/// (Shift+Ctrl+End, Del) leaves it the paragraph mark that ended the note, and that one's
/// format: the item's look back. Ctrl+Z of that selects what it put back, which ends in
/// the paragraph the item's look went to: that one is put right too. `t`: the editor's text.
pub(super) fn fix_caret_indent(h: HWND, t: &[u16]) {
    let (a, b) = sel(h);
    let back = a != b && sel_backwards(h);
    let (pa, pb) = (para_start(t, a), para_start(t, b));
    fix_indent(h, t, pa, a == b);
    if pb != pa {
        fix_indent(h, t, pb, false);
    }
    match (a != b, back) {
        (true, true) => set_sel(h, b, a),
        (true, false) => set_sel(h, a, b),
        _ => {}
    }
}

/// `fix_caret_indent` for the paragraph starting at `ps`, which the caret is in (`here`)
/// or is put in to read it.
fn fix_indent(h: HWND, t: &[u16], ps: i32, here: bool) {
    if !here {
        set_sel(h, ps, ps);
    }
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    if t.get(ps as usize).copied().is_some_and(is_task) {
        if ps > 0 && pf.numbering == 0 && pf.offset != mark_w() {
            style_boxes_in(h, ps, ps);
        }
        return;
    }
    let base = list_base(true);
    let dx = pf.start_indent - base;
    if pf.numbering == 0 && pf.offset == mark_w() && dx >= 0 && level_indent(level_at(dx)) == dx {
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

/// Delete (`copy`: cut) the selection when it runs from inside a checklist item's text to
/// the note's end (Shift+Ctrl+End, Del): the item keeps its own look. Left to RichEdit, the
/// item takes the paragraph mark that ended the note, and its format, which a numbered or
/// bulleted line after it made a "1. ○" item; `fix_caret_indent` can't put that right
/// unseen, as Ctrl+Z would bring the line back with the item's format. Done here, as one
/// step, Ctrl+Z puts back both. False, with nothing done, for any other selection.
pub fn delete_keeping_item(h: HWND, copy: bool) -> bool {
    let (a, b) = sel(h);
    let (a, b) = (a.min(b).max(0), a.max(b));
    let t = text(h);
    let ps = para_start(&t, a);
    if a == b || (b as usize) < t.len() || a <= ps || !t.get(ps as usize).copied().is_some_and(is_task) {
        return false;
    }
    // Only lines: a block's marks are its own to take apart.
    if !t[a as usize..].contains(&CR) || t[ps as usize..].iter().any(|&c| c == CELL || c == ROW_START || c == ROW_END) {
        return false;
    }
    if copy {
        send(h, windows_sys::Win32::UI::WindowsAndMessaging::WM_COPY, 0, 0);
    }
    still(h, || {
        set_sel(h, ps, ps);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        pf.mask = PFM_NUMBERING | PFM_NUMBERINGSTYLE | PFM_NUMBERINGSTART | PFM_NUMBERINGTAB | PFM_STARTINDENT | PFM_OFFSET | PFM_ALIGNMENT | PFM_LINESPACING | PFM_SPACEBEFORE | PFM_SPACEAFTER;
        set_sel(h, a, b);
        replace_sel(h, "");
        // Recorded: Ctrl+Z gives the line it went to its own format back with the text.
        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    });
    true
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

    /// Tab takes a bullet, a number and a checklist item down to the deepest level, each
    /// level further in than the one before, and no further; Shift+Tab brings it back.
    #[test]
    fn tab_to_the_deepest_level_and_back() {
        for (md, mark) in [("T\n- item", "- "), ("T\n1. item", "1. "), ("T\n- [ ] item", "- [ ] "), ("T\n- [x] item", "- [x] ")] {
            let h = editor();
            load_markdown(h, md);
            let at = String::from_utf16_lossy(&text(h)).find("item").unwrap() as i32;
            let indent = || {
                set_sel(h, at, at);
                let mut pf = ParaFormat2::new();
                send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                pf.start_indent
            };
            let at_level = |level: i32| {
                assert_eq!(list_level(h), Some(level), "{md}");
                assert_eq!(to_markdown(h, true), format!("# T\n{}{mark}item", "  ".repeat(level as usize)), "{md}");
                // Stored and opened again, at the same level.
                let h2 = editor();
                set_rtf(h2, &get_rtf(h));
                assert_eq!(to_markdown(h2, true), to_markdown(h, true), "stored: {md}");
            };
            let mut last = indent();
            for tab in 1..=MAX_LEVEL + 2 {
                set_sel(h, at, at);
                assert!(nest(h, 1));
                let now = indent();
                if tab <= MAX_LEVEL {
                    assert!(now > last, "{md}: level {tab} at {now}, the one before at {last}");
                } else {
                    assert_eq!(now, last, "{md}: no deeper than the deepest");
                }
                last = now;
                at_level(tab.min(MAX_LEVEL));
            }
            for level in (0..MAX_LEVEL).rev() {
                set_sel(h, at, at);
                assert!(nest(h, -1));
                at_level(level);
            }
            assert_eq!(to_markdown(h, true), format!("# {md}"));
        }
        // The deepest item's text still leaves a new note (320 DIP wide, a text column of
        // about 4,000 twips) over 100 DIP for its words.
        assert!(BULLET_TEXT + level_indent(MAX_LEVEL) + 1500 < 4000);
        // Each level is told from the ones beside it, and the first five are where they
        // always were.
        for l in 0..=MAX_LEVEL {
            assert_eq!(level_at(level_indent(l)), l);
            assert_eq!(level_at(level_indent(l) + DEEP_LEVEL / 2 - 1), l);
        }
        assert_eq!((0..=FULL_LEVELS).map(level_indent).collect::<Vec<_>>(), vec![0, 320, 640, 960, 1280, 1600]);
    }

    /// Everything after a checklist's last item deleted (Shift+Ctrl+End, Del): the item keeps
    /// its place, though the paragraph mark left to it is the plain line's that ended the
    /// note. Ctrl+Z puts that line back plain.
    #[test]
    fn an_item_keeps_its_indent_when_the_rest_is_deleted() {
        let h = editor();
        load_markdown(h, "T\n- [ ] one\n- [ ] two\nplain");
        style_boxes(h);
        send(h, EM_EMPTYUNDOBUFFER, 0, 0);
        let md = to_markdown(h, true);
        let para = |at: i32| {
            set_sel(h, at, at);
            let mut pf = ParaFormat2::new();
            send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
            (pf.start_indent, pf.offset, pf.numbering)
        };
        let find = |s: &str| {
            let w: Vec<u16> = s.encode_utf16().collect();
            text(h).windows(w.len()).position(|v| v == &w[..]).unwrap() as i32
        };
        let (one, two) = (find("one"), find("two") + 3);
        let plain = para(find("plain"));
        let item = para(one);
        assert_ne!(plain, item);
        assert_eq!(para(two), item);
        set_sel(h, two, text(h).len() as i32);
        replace_sel(h, "");
        restyle(h);
        assert_eq!(sel(h), (two, two));
        assert_eq!(to_markdown(h, true), "# T\n- [ ] one\n- [ ] two");
        assert_eq!(para(two), item, "the last item where the others are");
        const EM_UNDO: u32 = 0x00C7;
        send(h, EM_UNDO, 0, 0);
        restyle(h);
        assert_eq!(to_markdown(h, true), md);
        assert_eq!(para(find("plain")), plain, "the plain line plain again");
        assert_eq!(para(two), item);
        // A selection made backwards over lines stays so.
        set_sel(h, find("plain") + 2, one);
        assert!(sel_backwards(h));
        restyle(h);
        assert_eq!(sel(h), (one, find("plain") + 2));
        assert!(sel_backwards(h));
    }

    /// The same with a numbered or bulleted line after the checklist: the last item stays
    /// an item, not numbered or bulleted, at its own indent; Ctrl+Z puts the line back
    /// numbered or bulleted, and Ctrl+Y takes it away again.
    #[test]
    fn an_item_stays_an_item_when_a_list_after_it_is_deleted() {
        for after in ["1. three", "- three", "  - [ ] nested\n1. three"] {
            let h = editor();
            load_markdown(h, &format!("T\n- [ ] one\n  - [ ] two\n{after}"));
            style_boxes(h);
            send(h, EM_EMPTYUNDOBUFFER, 0, 0);
            let md = to_markdown(h, true);
            let para = |at: i32| {
                set_sel(h, at, at);
                let mut pf = ParaFormat2::new();
                send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                (pf.start_indent, pf.offset, pf.numbering)
            };
            let find = |s: &str| {
                let w: Vec<u16> = s.encode_utf16().collect();
                text(h).windows(w.len()).position(|v| v == &w[..]).unwrap() as i32
            };
            let two = find("two") + 3;
            let item = para(two);
            let three = para(find("three"));
            assert_ne!(three.2, 0, "{after}");
            set_sel(h, two, text(h).len() as i32);
            assert!(delete_keeping_item(h, false), "{after}");
            restyle(h);
            assert_eq!(sel(h), (two, two));
            assert_eq!(to_markdown(h, true), "# T\n- [ ] one\n  - [ ] two", "{after}");
            assert_eq!(para(two), item, "{after}: the last item as it was");
            assert!(undo_group(h));
            restyle(h);
            assert_eq!(to_markdown(h, true), md, "{after}");
            assert_eq!(para(find("three")), three, "{after}: the list line as it was");
            assert_eq!(para(two), item);
            assert!(redo_group(h));
            restyle(h);
            assert_eq!(to_markdown(h, true), "# T\n- [ ] one\n  - [ ] two", "{after}");
            assert_eq!(para(two), item);
        }
        // Not from the box, nor short of the end, nor at a plain line: left to RichEdit.
        let h = editor();
        load_markdown(h, "T\n- [ ] one\n1. two\nplain");
        style_boxes(h);
        let t = text(h);
        let one = t.iter().position(|&c| is_task(c)).unwrap() as i32;
        for (a, b) in [(one, t.len() as i32), (one + 3, t.len() as i32 - 2), (0, t.len() as i32), (one + 3, one + 3)] {
            set_sel(h, a, b);
            assert!(!delete_keeping_item(h, false), "{a}..{b}");
            assert_eq!(text(h), t);
        }
    }
}
