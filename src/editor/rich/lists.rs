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
    set_numbering(h, if cur == kind { 0 } else { kind });
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
/// list's start or at a shallower (parent) item.
pub(super) fn siblings_before(h: HWND, t: &[u16], ps: i32, level: i32) -> usize {
    let mut n = 0;
    let mut p = ps;
    while p > 0 {
        p = para_start(t, p - 1);
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
        }
    }
    n
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
        if matches!(cur.numbering, PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN) {
            pf.mask |= PFM_NUMBERING | PFM_NUMBERINGSTYLE | PFM_NUMBERINGSTART;
            pf.numbering = [PFN_ARABIC, PFN_LCLETTER, PFN_LCROMAN][next as usize % 3];
            pf.numbering_style = PFNS_PERIOD;
            // Continue the count of this level across the nested items in between
            // (RichEdit would restart it after a change of style).
            pf.numbering_start = 1 + siblings_before(h, &t, ps, next) as u16;
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
    let (a, _) = sel(h);
    let t = text(h);
    let ps = para_start(&t, a) as usize;
    if t.get(ps).copied().is_some_and(is_task) {
        return;
    }
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    let base = list_base(true);
    if pf.numbering == 0 && pf.offset == mark_w() && pf.start_indent >= base && (pf.start_indent - base) % LEVEL == 0 {
        pf = ParaFormat2::new();
        pf.mask = PFM_STARTINDENT | PFM_OFFSET;
        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    }
}
