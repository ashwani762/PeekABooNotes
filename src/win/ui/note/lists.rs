// Copyright (C) 2026 Ashwani Kumar
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
    // Held still: putting the caret back would otherwise scroll to it, away from the box
    // that was just ticked.
    rich::still(e, || {
        let (a, b) = rich::sel(e);
        let end = rich::para_end(&t, cp);
        rich::set_sel(e, cp, cp + 1);
        rich::replace_sel(e, if checked { "\u{2610}" } else { "\u{2611}" });
        rich::set_sel(e, cp + 1, end);
        rich::set_effect(e, CFM_STRIKEOUT, !checked);
        // A caret before the box would draw inside the circle: keep it after it.
        let (a, b) = if a == b && a == cp { (cp + 1, cp + 1) } else { (a, b) };
        rich::set_sel(e, a, b);
        rich::style_boxes(e); // completed items turn secondary; reopened ones regain full ink
    });
    crate::win::editor::checks::animate(e, cp, !checked);
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

/// "- ", "* ", "1. ", "[] " / "[ ] " at the start of a line become real lists, and "> "
/// a quote.
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
    if prefix == ">" && ps > 0 && rich::cell_at(e, &t, a).is_none() {
        // On a list item the quote takes the item's place, under the item above it, as
        // deep as the item was.
        let item = rich::list_level(e);
        rich::still(e, || {
            rich::set_sel(e, ps, a);
            rich::replace_sel(e, "");
            if item.is_some() {
                rich::set_numbering(e, 0);
            }
            let t = rich::text(e);
            rich::wrap(e, ps, rich::para_end(&t, ps), rich::Block::Quote);
            if let Some(level) = item
                && let Some((_, rows)) = rich::block_at(e, &rich::text(e), ps)
            {
                rich::set_quote_level(e, rows[0], level as usize + 1);
            }
        });
        mark_dirty(h);
        return true;
    }
    let kind = match prefix.as_str() {
        "-" | "*" => (AutoKind::List, PFN_BULLET),
        "1." => (AutoKind::List, PFN_ARABIC),
        "[]" | "[ ]" => (AutoKind::Task, 0),
        _ => return false,
    };
    if kind.0 == AutoKind::List && rich::numbering(e) != 0 {
        return false;
    }
    let pos = rich::still(e, || {
        rich::set_sel(e, ps, a);
        rich::replace_sel(e, "");
        match kind {
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
        }
    });
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

/// Enter in a quote or code block makes a new line in it; on an empty last line it leaves
/// the block instead (an empty block goes altogether), as an empty list item ends a
/// list. A code line starts indented like the one before. "```" alone on a line opens a
/// code block. None when Enter is RichEdit's to handle.
pub(super) fn block_enter(e: HWND) -> Option<bool> {
    use rich::Block;
    let (a, b) = rich::sel(e);
    if a != b {
        return None;
    }
    let t = rich::text(e);
    let (ps, pe) = (rich::para_start(&t, a), rich::para_end(&t, a));
    if let Some((kind, rows, ca, cb)) = rich::cell_at(e, &t, a) {
        if kind == Block::Table {
            return None;
        }
        // Empty, or holding just the indent the last Enter gave it.
        let blank = t[ps as usize..pe as usize].iter().all(|&c| c == b' ' as u16);
        if blank && pe as usize == cb {
            rich::still(e, || {
                if ps as usize == ca {
                    rich::delete_block(e, a);
                } else {
                    // The empty line goes, and the caret moves on below the block.
                    rich::set_sel(e, ps - 1, pe);
                    rich::replace_sel(e, "");
                    let end = rows.last().map_or(0, |r| r.end) as i32 - (pe - ps + 1);
                    rich::set_sel(e, end, end);
                }
            });
            return Some(true);
        }
        if kind == Block::Code {
            let indent: String = t[ps as usize..pe as usize].iter().take_while(|&&c| c == b' ' as u16).map(|_| ' ').collect();
            rich::still(e, || {
                rich::replace_sel(e, &format!("\r{indent}"));
                // Only the last line keeps the air below it.
                rich::style_blocks(e);
            });
            return Some(true);
        }
        return None;
    }
    if ps > 0 && pe == a && String::from_utf16_lossy(&t[ps as usize..pe as usize]).trim() == "```" {
        rich::still(e, || {
            rich::set_sel(e, ps, pe);
            rich::replace_sel(e, "");
            let over = if rich::text(e).get(ps as usize) == Some(&rich::CR) { 1 } else { 0 };
            let cell = rich::insert_block_over(e, ps, over, Block::Code, 1, 1);
            rich::set_sel(e, cell, cell);
        });
        return Some(true);
    }
    None
}

/// Tab in a quote or code block: code indents by four spaces (Shift+Tab takes up to four
/// back); a quote steps in under a list item (Shift+Tab back out), unless the caret is on
/// a list item inside it, which nests as anywhere. False when it's not ours: in a table
/// Tab moves between cells.
pub(super) fn block_tab(e: HWND, shift: bool) -> bool {
    let (a, b) = rich::sel(e);
    let t = rich::text(e);
    match rich::cell_at(e, &t, a) {
        Some((rich::Block::Code, ..)) => {
            let ps = rich::para_start(&t, a);
            if shift {
                let n = t[ps as usize..].iter().take(4).take_while(|&&c| c == b' ' as u16).count() as i32;
                if n > 0 {
                    rich::set_sel(e, ps, ps + n);
                    rich::replace_sel(e, "");
                    rich::set_sel(e, (a - n).max(ps), (b - n).max(ps));
                }
            } else {
                rich::replace_sel(e, "    ");
            }
            true
        }
        Some((rich::Block::Quote, rows, ..)) => {
            if rich::list_level(e).is_some() {
                return false;
            }
            let level = rich::quote_level(e, rows[0]);
            let next = if shift { level.saturating_sub(1) } else { level + 1 };
            if next != level {
                rich::still(e, || rich::set_quote_level(e, rows[0], next));
            }
            true
        }
        _ => false,
    }
}

/// Backspace at the very start of a quote or code block takes the block apart, leaving
/// its text as ordinary paragraphs.
pub(super) fn block_backspace(e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    let t = rich::text(e);
    match rich::cell_at(e, &t, a) {
        Some((rich::Block::Quote | rich::Block::Code, _, ca, _)) if a == b && a as usize == ca => {
            rich::still(e, || rich::unwrap(e, a));
            true
        }
        _ => false,
    }
}

/// Enter continues checklists and ends lists on an empty item.
pub(super) fn handle_enter(h: HWND, e: HWND) -> bool {
    if let Some(done) = block_enter(e) {
        mark_dirty(h);
        return done;
    }
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
            rich::still(e, || {
                rich::replace_sel(e, "\r");
                rich::replace_sel(e, "\u{2610} ");
                let (c, _) = rich::sel(e);
                rich::set_sel(e, c - 2, c);
                rich::set_effect(e, CFM_STRIKEOUT, false);
                rich::set_sel(e, c, c);
                rich::style_boxes(e);
            });
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

#[cfg(test)]
mod tests {
    use super::super::commands::block_command;
    use super::*;
    use rich::{Block, CELL, CR, ROW_END, ROW_START};
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, WS_POPUP};

    fn editor(s: &str) -> HWND {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(h);
        rich::insert_text(h, s);
        h
    }

    /// The text, with `/` for paragraph ends, `|` cell ends, `[` `]` rows, `^` the caret.
    fn plain(h: HWND) -> String {
        let (a, _) = rich::sel(h);
        let mut out = String::new();
        for (i, &c) in rich::text(h).iter().enumerate() {
            if i as i32 == a {
                out.push('^');
            }
            out.push(match c {
                CR => '/',
                CELL => '|',
                ROW_START => '[',
                ROW_END => ']',
                c => char::from_u32(c as u32).unwrap_or('?'),
            });
        }
        if a as usize >= rich::text(h).len() {
            out.push('^');
        }
        out
    }

    fn caret(h: HWND, s: &str) {
        let at = plain(h).replace('^', "").find(s).unwrap() as i32;
        rich::set_sel(h, at, at);
    }

    #[test]
    fn quote_and_code_commands() {
        let h = editor("Title\rone\rtwo\rend");
        caret(h, "one");
        block_command(h, Block::Quote);
        assert_eq!(plain(h).replace('^', ""), "Title/[/one|]/two/end");
        // Again: back to text.
        caret(h, "one");
        block_command(h, Block::Quote);
        assert_eq!(plain(h).replace('^', ""), "Title/one/two/end");
        // Two lines into code, then code into a quote.
        let a = plain(h).replace('^', "").find("one").unwrap() as i32;
        let b = plain(h).replace('^', "").find("two").unwrap() as i32 + 1;
        rich::set_sel(h, a, b);
        block_command(h, Block::Code);
        assert_eq!(plain(h).replace('^', ""), "Title/[/one/two|]/end");
        caret(h, "two");
        block_command(h, Block::Quote);
        assert_eq!(plain(h).replace('^', ""), "Title/[/one/two|]/end");
        let t = rich::text(h);
        assert_eq!(rich::block_at(h, &t, 8).map(|b| b.0), Some(Block::Quote));
        // On the title: an empty block below it, the caret in it.
        let h = editor("Title");
        block_command(h, Block::Code);
        assert_eq!(plain(h), "Title/[/^|]/");
        // A table on an empty line takes its place.
        let h = editor("Title\r\rend");
        rich::set_sel(h, 6, 6);
        block_command(h, Block::Table);
        assert_eq!(plain(h), "Title/[/^|||]/[/|||]/end");
    }

    #[test]
    fn one_undo_per_block_command() {
        let h = editor("Title\rone\rtwo\rend");
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        caret(h, "one");
        rich::still(h, || block_command(h, Block::Quote));
        assert_eq!(plain(h).replace('^', ""), "Title/[/one|]/two/end");
        assert!(rich::undo_group(h));
        assert_eq!(plain(h).replace('^', ""), "Title/one/two/end");
    }

    #[test]
    fn keys_in_blocks() {
        let h = editor("Title\r    a\rend");
        caret(h, "    a");
        block_command(h, Block::Code);
        // Enter keeps the indent.
        caret(h, "a|");
        let at = rich::sel(h).0 + 1;
        rich::set_sel(h, at, at);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/    a/    ^|]/end");
        // Tab and Shift+Tab.
        assert!(block_tab(h, false));
        assert_eq!(plain(h), "Title/[/    a/        ^|]/end");
        assert!(block_tab(h, true));
        assert_eq!(plain(h), "Title/[/    a/    ^|]/end");
        // A last line with nothing but its indent: Enter leaves the block.
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/    a|]/^end");
        // Backspace at the start of the block takes it apart.
        caret(h, "    a");
        assert!(block_backspace(h));
        assert_eq!(plain(h).replace('^', ""), "Title/    a/end");
        // ``` and Enter opens a code block.
        let h = editor("Title\r```");
        rich::set_sel(h, 9, 9);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/^|]/");
        // Enter in an empty block takes it away.
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h).replace('^', ""), "Title/");
    }

    #[test]
    fn quote_typed_after_a_quote() {
        // "> " typed on the line after a quote, then on the line after text below it.
        let h = editor("Title\rline one\r");
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        let t = rich::text(h);
        rich::wrap(h, rich::para_start(&t, end), end, Block::Quote);
        assert_eq!(plain(h), "Title/line one/[/^|]/");
        rich::replace_sel(h, "q1");
        // Enter twice leaves it.
        assert_eq!(block_enter(h), None);
        rich::replace_sel(h, "\r");
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/line one/[/q1|]/^");
        rich::replace_sel(h, "after\r");
        let (a, _) = rich::sel(h);
        let t = rich::text(h);
        rich::wrap(h, rich::para_start(&t, a), rich::para_end(&t, a), Block::Quote);
        // Right after the line above: no empty paragraph between.
        assert_eq!(plain(h), "Title/line one/[/q1|]/after/[/^|]/");
    }

    #[test]
    fn quote_typed_on_a_nested_item() {
        // "> " on a new item under "bravo": a quote under bravo, not a bullet in a quote.
        let h = editor("");
        rich::load_markdown(h, "T\n- alpha\n  - bravo\n  - x");
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end - 1, end);
        rich::replace_sel(h, ">");
        assert!(try_auto_list(null_mut(), h));
        rich::replace_sel(h, "q");
        assert_eq!(rich::to_markdown(h, true), "# T\n- alpha\n  - bravo\n    > q");
    }

    /// Enter on a task, or "[] " typed: one Ctrl+Z takes the new item back, not first the
    /// formatting of its box.
    #[test]
    fn one_undo_per_new_item() {
        let h = editor("T\r\u{2610} task");
        rich::style_boxes(h);
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        assert!(handle_enter(null_mut(), h));
        assert_eq!(plain(h), "T/☐ task/☐ ^");
        assert!(rich::undo_group(h));
        assert_eq!(plain(h), "T/☐ task^");
        rich::replace_sel(h, "\r[]");
        assert!(try_auto_list(null_mut(), h));
        assert_eq!(plain(h), "T/☐ task/☐ ^");
        assert!(rich::undo_group(h));
        assert_eq!(plain(h), "T/☐ task/[]^");
    }

    /// Ticking a box up the note while the caret is far below keeps the view where it is.
    #[test]
    fn ticking_keeps_the_view() {
        let mut body = String::from("Title\r\u{2610} up here\r");
        for i in 0..80 {
            body.push_str(&format!("line {i}\r"));
        }
        let h = editor(&body);
        rich::style_boxes(h);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        rich::set_scroll_y(h, 0);
        assert_eq!(rich::scroll_y(h), 0);
        let cp = rich::text(h).iter().position(|&c| c == rich::BOX).unwrap() as i32;
        toggle_check(h, cp);
        assert_eq!(rich::scroll_y(h), 0, "the view jumped to the caret");
        assert_eq!(rich::sel(h), (end, end), "the caret stays where it was");
    }

    /// A tick: Ctrl+Z unticks it whole (box and strike), Ctrl+Y ticks it again, and so on,
    /// without the undo itself becoming a new tick.
    #[test]
    fn tick_undo_and_redo() {
        let h = editor("T\r\u{2610} task\r\u{2610} other");
        rich::style_boxes(h);
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        let cp = rich::text(h).iter().position(|&c| c == rich::BOX).unwrap() as i32;
        let struck = |h: HWND| {
            rich::set_sel(h, cp + 2, cp + 6);
            let cf = rich::char_format(h, rich::SCF_SELECTION);
            rich::set_sel(h, end, end);
            cf.effects & CFM_STRIKEOUT != 0
        };
        toggle_check(h, cp);
        assert_eq!(rich::text(h)[cp as usize], rich::CHECKED);
        assert!(struck(h));
        for _ in 0..2 {
            assert!(rich::undo_group(h));
            assert_eq!(rich::text(h)[cp as usize], rich::BOX, "undo unticks");
            assert!(!struck(h), "and unstrikes");
            assert!(rich::redo_group(h));
            assert_eq!(rich::text(h)[cp as usize], rich::CHECKED, "redo ticks again");
            assert!(struck(h));
        }
    }

    /// An assistant's change (or a version put back) in an open note: one Ctrl+Z takes it
    /// back, Ctrl+Y brings it again, and the content is just as the note was saved.
    #[test]
    fn whole_content_replaced_as_one_edit() {
        let h = editor("");
        rich::load_markdown(h, "Mine\n- one\n- two");
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let mine = rich::text(h);
        let w = editor("");
        rich::load_markdown(w, "Theirs\n> quoted\n- [ ] task\n| a | b |\n| --- | --- |\n| 1 | 2 |");
        let theirs_rtf = rich::get_rtf(w);
        let theirs = rich::text(w);
        rich::replace_all_rtf(h, &theirs_rtf);
        assert_eq!(rich::text(h), theirs);
        assert!(rich::undo_group(h));
        assert_eq!(rich::text(h), mine);
        assert!(rich::redo_group(h));
        assert_eq!(rich::text(h), theirs);
    }

    /// Restyling inside a bulk change stays out of the undo history all the way through,
    /// even after a nested `unrecorded` ends.
    #[test]
    fn nested_unrecorded_keeps_bulk_unrecorded() {
        const EM_CANUNDO: u32 = 0x00C6;
        let h = editor("T\rbody");
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        rich::bulk(h, || {
            rich::unrecorded(h, || rich::style_title(h));
            rich::set_sel(h, 0, 1);
            rich::toggle_effect(h, CFM_BOLD);
        });
        assert_eq!(rich::send(h, EM_CANUNDO, 0, 0), 0);
    }

    /// Blocks of a kind side by side stay apart through export and import: a quote typed
    /// right after another, a quote under an item then one after the list, two tables.
    #[test]
    fn neighbouring_blocks_stay_apart() {
        let h = editor("Title\r");
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        let t = rich::text(h);
        rich::wrap(h, rich::para_start(&t, end), end, Block::Quote);
        rich::replace_sel(h, "first");
        assert_eq!(block_enter(h), None);
        rich::replace_sel(h, "\r");
        assert_eq!(block_enter(h), Some(true));
        rich::replace_sel(h, ">");
        assert!(try_auto_list(null_mut(), h));
        rich::replace_sel(h, "second");
        let md = rich::to_markdown(h, true);
        assert_eq!(md, "# Title\n> first\n\n> second");
        for md in ["T\n> first\n\n> second", "T\n- item\n  > under\n\n> top", "T\n| a |\n| --- |\n| 1 |\n\n| b |\n| --- |\n| 2 |"] {
            let h = editor("");
            rich::load_markdown(h, md);
            let t = rich::text(h);
            assert_eq!(rich::blocks(h, &t).len(), 2, "{md}");
            assert_eq!(rich::to_markdown(h, true), format!("# {md}"));
        }
    }

    #[test]
    fn undo_after_restyle_and_save() {
        const EM_UNDO: u32 = 0x00C7;
        let h = editor("Title\rbody");
        rich::send(h, rich::EM_SETUNDOLIMIT, 110, 0);
        rich::style_title(h);
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let worker = editor("");
        for k in 0..10 {
            let end = rich::text(h).len() as i32;
            rich::set_sel(h, end, end);
            rich::replace_sel(h, &format!(" w{k}"));
            // What the note does after a burst of typing, then when it saves.
            rich::style_title(h);
            rich::color_named_links(h);
            rich::fix_caret_indent(h);
            let rtf = rich::get_rtf(h);
            rich::set_rtf(worker, &rtf);
            let _ = rich::to_markdown(worker, false);
        }
        // Each Ctrl+Z takes back one edit, newest first, down to how it started.
        for k in (0..10).rev() {
            assert_eq!(rich::send(h, EM_UNDO, 0, 0), 1);
            let words: String = (0..k).map(|j| format!(" w{j}")).collect();
            assert_eq!(plain(h).replace('^', ""), format!("Title/body{words}"));
        }
        assert_eq!(rich::send(h, EM_UNDO, 0, 0), 0);
    }
}
