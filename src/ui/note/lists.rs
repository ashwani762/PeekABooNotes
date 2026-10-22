// Copyright (C) 2026 Ashwani Kumar Bachchan
// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists and checklists while typing: auto-lists, Enter, ticking, nesting.

use super::*;

pub(super) fn is_task(c: u16) -> bool {
    c == rich::BOX || c == rich::CHECKED
}

/// Flip ☐ <-> ☑ at `cp` and strike / unstrike the rest of the line.
pub(super) fn toggle_check(e: HWND, cp: i32) {
    let t = rich::text(e);
    let Some(&c) = t.get(cp as usize) else { return };
    if !is_task(c) {
        return;
    }
    let checked = c == rich::CHECKED;
    let (a, b) = rich::sel(e);
    let end = rich::para_end(&t, cp);
    rich::set_sel(e, cp, cp + 1);
    rich::replace_sel(e, if checked { "\u{2610}" } else { "\u{2611}" });
    rich::set_sel(e, cp + 1, end);
    rich::set_effect(e, CFM_STRIKEOUT, !checked);
    rich::set_sel(e, a, b);
    rich::style_boxes(e); // completed items turn secondary; reopened ones regain full ink
    crate::editor::checks::animate(e, cp, !checked);
    LAST_TOGGLE.with(|l| l.set(Some((e as isize, cp, rich::text(e).len()))));
}

thread_local! {
    /// The last tick, so an immediate Ctrl+Z reverses it whole (a tick is several
    /// RichEdit edits: glyph, strikeout and colours).
    pub(super) static LAST_TOGGLE: std::cell::Cell<Option<(isize, i32, usize)>> = const { std::cell::Cell::new(None) };
}

/// Ctrl+Z straight after a tick: untick (or re-tick) instead of a partial RichEdit undo.
pub(super) fn undo_toggle(e: HWND) -> bool {
    let Some((le, cp, len)) = LAST_TOGGLE.with(|l| l.take()) else { return false };
    if le != e as isize || rich::text(e).len() != len {
        return false;
    }
    toggle_check(e, cp);
    LAST_TOGGLE.with(|l| l.set(None));
    true
}

pub(super) fn toggle_check_at_caret(e: HWND) {
    let t = rich::text(e);
    let (a, _) = rich::sel(e);
    let ps = rich::para_start(&t, a);
    if t.get(ps as usize).copied().is_some_and(is_task) {
        toggle_check(e, ps);
    } else {
        toggle_task_lines(e);
    }
}

/// Add or remove a checkbox at the start of every paragraph in the selection.
pub(super) fn toggle_task_lines(e: HWND) {
    let t = rich::text(e);
    let (a, b) = rich::sel(e);
    let mut starts = vec![rich::para_start(&t, a)];
    for (i, &c) in t.iter().enumerate().take(b.max(a) as usize).skip(a as usize) {
        if c == 13 {
            starts.push(i as i32 + 1);
        }
    }
    let add = !t.get(starts[0] as usize).copied().is_some_and(is_task);
    let (mut na, mut nb) = (a, b);
    for &ps in starts.iter().rev() {
        let has = t.get(ps as usize).copied().is_some_and(is_task);
        if add && !has {
            rich::set_sel(e, ps, ps);
            if rich::numbering(e) != 0 {
                rich::set_numbering(e, 0); // a checklist line is never also numbered
            }
            rich::replace_sel(e, "\u{2610} ");
            nb += 2;
            if ps <= a {
                na += 2;
            }
        } else if !add && has {
            let end = rich::para_end(&t, ps);
            rich::set_sel(e, ps, end);
            rich::set_effect(e, CFM_STRIKEOUT, false);
            let n = if t.get(ps as usize + 1) == Some(&(b' ' as u16)) { 2 } else { 1 };
            rich::set_sel(e, ps, ps + n);
            rich::replace_sel(e, "");
            nb -= n;
            if ps < a {
                na -= n;
            }
        }
    }
    rich::set_sel(e, na.max(0), nb.max(na.max(0)));
    rich::style_boxes(e);
}

/// "- ", "* ", "1. ", "[] " / "[ ] " at the start of a line become real lists.
pub(super) fn try_auto_list(h: HWND, e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    if a != b {
        return false;
    }
    let t = rich::text(e);
    let ps = rich::para_start(&t, a);
    if a - ps > 3 {
        return false;
    }
    let prefix = String::from_utf16_lossy(&t[ps as usize..a as usize]);
    let kind = match prefix.as_str() {
        "-" | "*" => (AutoKind::List, PFN_BULLET),
        "1." => (AutoKind::List, PFN_ARABIC),
        "[]" | "[ ]" => (AutoKind::Task, 0),
        _ => return false,
    };
    if kind.0 == AutoKind::List && rich::numbering(e) != 0 {
        return false;
    }
    rich::set_sel(e, ps, a);
    rich::replace_sel(e, "");
    let pos = match kind {
        (AutoKind::List, n) => {
            rich::set_numbering(e, n);
            ps
        }
        (AutoKind::Task, _) => {
            if rich::numbering(e) != 0 {
                rich::set_numbering(e, 0);
            }
            rich::replace_sel(e, "\u{2610} ");
            rich::style_boxes(e);
            ps + 2
        }
    };
    if let Some(st) = state(h) {
        st.auto = Some(Auto { kind: kind.0, typed: format!("{prefix} "), pos });
    }
    mark_dirty(h);
    true
}

pub(super) fn revert_auto(h: HWND, e: HWND) -> bool {
    let Some(st) = state(h) else { return false };
    let Some(au) = st.auto.take() else { return false };
    let (a, b) = rich::sel(e);
    if a != b || a != au.pos {
        return false;
    }
    match au.kind {
        AutoKind::List => {
            rich::set_numbering(e, 0);
            rich::replace_sel(e, &au.typed);
        }
        AutoKind::Task => {
            rich::set_sel(e, a - 2, a);
            rich::replace_sel(e, &au.typed);
        }
    }
    mark_dirty(h);
    true
}

/// Enter continues checklists and ends lists on an empty item.
pub(super) fn handle_enter(h: HWND, e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    let t = rich::text(e);
    let ps = rich::para_start(&t, a);
    let pe = rich::para_end(&t, b);
    let para = &t[ps as usize..pe as usize];
    if para.first().copied().is_some_and(is_task) {
        let rest = String::from_utf16_lossy(&para[1..]);
        if rest.trim().is_empty() && rich::list_level(e).unwrap_or(0) > 0 {
            rich::nest(e, -1);
        } else if rest.trim().is_empty() {
            rich::set_sel(e, ps, pe);
            rich::set_effect(e, CFM_STRIKEOUT, false);
            rich::replace_sel(e, "");
        } else {
            rich::replace_sel(e, "\r");
            rich::replace_sel(e, "\u{2610} ");
            let (c, _) = rich::sel(e);
            rich::set_sel(e, c - 2, c);
            rich::set_effect(e, CFM_STRIKEOUT, false);
            rich::set_sel(e, c, c);
            rich::style_boxes(e);
        }
        mark_dirty(h);
        return true;
    }
    if ps == 0 && a == b && rich::numbering(e) == 0 {
        // Leaving the title line: the next line starts as regular body text.
        rich::replace_sel(e, "\r");
        let (c, _) = rich::sel(e);
        let pe2 = rich::para_end(&rich::text(e), c);
        rich::set_sel(e, c, pe2);
        rich::set_body_format(e);
        rich::set_sel(e, c, c);
        rich::set_body_format(e);
        mark_dirty(h);
        return true;
    }
    if para.iter().all(|&c| c == b' ' as u16) && rich::numbering(e) != 0 {
        if rich::list_level(e).unwrap_or(0) > 0 {
            rich::nest(e, -1);
        } else {
            rich::set_numbering(e, 0);
        }
        mark_dirty(h);
        return true;
    }
    false
}

pub(super) fn checkbox_at(e: HWND, x: i32, y: i32) -> Option<i32> {
    let cp = rich::char_from_pos(e, x, y);
    let t = rich::text(e);
    for c in [cp, cp - 1] {
        if c < 0 || !t.get(c as usize).copied().is_some_and(is_task) {
            continue;
        }
        let p0 = rich::pos_from_char(e, c);
        let p1 = rich::pos_from_char(e, c + 1);
        let w = if p1.y == p0.y && p1.x > p0.x { p1.x - p0.x } else { 16 };
        if x >= p0.x - 2 && x <= p0.x + w + 2 && y >= p0.y - 2 && y <= p0.y + w * 2 {
            return Some(c);
        }
    }
    None
}
