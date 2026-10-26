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
        rich::style_boxes_in(e, cp, cp); // completed items turn secondary; reopened ones regain full ink
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
            // A checklist line is never also numbered; a nested one stays as deep.
            rich::numbering_to_task(e);
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
    rich::style_boxes_in(e, na, nb);
}

/// A pasted line's own list or checklist marker taken off: whether it was ticked, and
/// its text.
fn item_text(l: &str) -> (bool, &str) {
    let l = l.trim();
    for (m, done) in [("- [ ] ", false), ("- [x] ", true), ("- [X] ", true), ("[ ] ", false), ("[x] ", true), ("[X] ", true), ("\u{2610} ", false), ("\u{2611} ", true)] {
        if let Some(r) = l.strip_prefix(m) {
            return (done, r.trim_start());
        }
    }
    for m in ["- ", "* ", "\u{2022} "] {
        if let Some(r) = l.strip_prefix(m) {
            return (false, r.trim_start());
        }
    }
    (false, l)
}

/// Several lines pasted into a checklist item: each line an item of its own, as in Notes,
/// the first going on where the caret is (lines without boxes would keep the item's
/// indent and look like broken items). Markers on the lines (`- `, `[ ] `, `[x] `)
/// become the items' own, and blank lines are left out. False, with nothing done, when
/// the caret isn't in an item or the text is one line.
pub(super) fn paste_into_checklist(e: HWND, clip: &str) -> bool {
    let norm = clip.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = norm.split('\n').filter(|l| !l.trim().is_empty()).collect();
    if lines.len() < 2 {
        return false;
    }
    let (a, _) = rich::sel(e);
    let t = rich::text(e);
    let ps = rich::para_start(&t, a);
    if !t.get(ps as usize).copied().is_some_and(is_task) || (ps > 0 && t[ps as usize - 1] != rich::CR) {
        return false;
    }
    let gap = if t.get(ps as usize + 1) == Some(&(b' ' as u16)) { 2 } else { 1 };
    // At the start of the item's text, the first line's marker goes too.
    let first = if a <= ps + gap { item_text(lines[0]).1 } else { lines[0].trim_end() };
    let mut s = first.to_string();
    for l in &lines[1..] {
        let (done, body) = item_text(l);
        s.push('\r');
        s.push(if done { '\u{2611}' } else { '\u{2610}' });
        s.push(' ');
        s.push_str(body);
    }
    let first_len = first.encode_utf16().count() as i32;
    rich::still(e, || {
        rich::replace_sel(e, &s);
        let (end, _) = rich::sel(e);
        // The new items start open, whatever the one pasted into (`style_boxes` strikes
        // the ticked ones).
        rich::set_sel(e, a + first_len, end);
        rich::set_effect(e, CFM_STRIKEOUT, false);
        rich::set_sel(e, end, end);
        rich::style_boxes_in(e, a, end);
    });
    true
}

/// "- ", "* ", "1. ", "[] " / "[ ] " at the start of a line become real lists, and "> "
/// a quote.
pub(super) fn try_auto_list(h: HWND, e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    if a != b {
        return false;
    }
    // A space typed far into a line, as most are: its paragraph says so (each copied the
    // whole note).
    let p = rich::paras_around(e, a.max(0) as usize, a.max(0) as usize);
    if a - (p.from as i32 + rich::para_start(&p.t, a - p.from as i32)) > 3 {
        return false;
    }
    let t = rich::text(e);
    let ps = rich::para_start(&t, a);
    // In code a "- " is just code, and a table's cells hold plain lines (their lists
    // wouldn't survive export): only quotes take lists.
    if lists_refused(e, &t, a) {
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
        // Ticked.
        "[x]" | "[X]" => (AutoKind::Task, 1),
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
            (AutoKind::Task, ticked) => {
                rich::numbering_to_task(e);
                rich::replace_sel(e, if ticked == 1 { "\u{2611} " } else { "\u{2610} " });
                rich::style_boxes_in(e, ps, ps);
                if ticked == 1 {
                    // What's typed next is done text: struck and faded.
                    rich::set_done_format(e);
                }
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
/// list; so it does in a table's last cell. A code line starts indented like the one
/// before. "```" alone on a line opens a code block. None when Enter is RichEdit's to
/// handle.
pub(super) fn block_enter(e: HWND) -> Option<bool> {
    use rich::Block;
    let (a, b) = rich::sel(e);
    if a != b {
        return None;
    }
    let t = rich::text(e);
    let (ps, pe) = (rich::para_start(&t, a), rich::para_end(&t, a));
    if let Some((kind, rows, ca, cb)) = rich::cell_at(e, &t, a) {
        // A code block's first line starts past its language, kept hidden there.
        let text = if kind == Block::Code && ps as usize == ca { rich::past_tag(e, &t, ca, cb) } else { ps as usize };
        // Empty, or holding just the indent the last Enter gave it.
        let blank = t[text..pe as usize].iter().all(|&c| c == b' ' as u16);
        if kind == Block::Table {
            // Only the table's last cell is a way out (Enter elsewhere is a line in a cell):
            // empty, or on an empty line after its text, Enter goes on below the table.
            let last = rows.last().and_then(|r| rich::cells(&t, *r).last().copied());
            if !blank || pe as usize != cb || last != Some((ca, cb)) {
                return None;
            }
            rich::still(e, || {
                let mut end = rows.last().map_or(0, |r| r.end) as i32;
                if ps as usize > ca {
                    rich::set_sel(e, ps - 1, pe);
                    rich::replace_sel(e, "");
                    end -= pe - ps + 1;
                }
                rich::set_sel(e, end, end);
            });
            return Some(true);
        }
        // Code keeps empty lines: only an empty last line after another empty one leaves
        // the block, and both go (one Enter for a gap in the code, two to be done).
        let from = if kind == Block::Code && ps as usize > ca {
            let pp = rich::para_start(&t, ps - 1);
            let line = if pp as usize == ca { rich::past_tag(e, &t, ca, cb) } else { pp as usize };
            t[line..(ps - 1) as usize].iter().all(|&c| c == b' ' as u16).then_some(pp)
        } else {
            Some(ps)
        };
        if let Some(from) = from.filter(|_| blank && pe as usize == cb) {
            rich::still(e, || {
                if from as usize == ca {
                    rich::delete_block(e, a);
                } else {
                    // The empty lines go, and the caret moves on below the block.
                    rich::set_sel(e, from - 1, pe);
                    rich::replace_sel(e, "");
                    let end = rows.last().map_or(0, |r| r.end) as i32 - (pe - from + 1);
                    rich::set_sel(e, end, end);
                }
            });
            return Some(true);
        }
        if kind == Block::Code {
            let indent: String = t[text..pe as usize].iter().take_while(|&&c| c == b' ' as u16).map(|_| ' ').collect();
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
        Some((rich::Block::Code, _, ca, cb)) => {
            if a == b && !shift {
                rich::replace_sel(e, "    ");
                return true;
            }
            // Each line the selection touches (within the block), last first so the
            // positions of those before don't move.
            let mut starts = vec![rich::para_start(&t, a)];
            for i in a.max(ca as i32)..b.min(cb as i32) {
                if t[i as usize] == rich::CR {
                    starts.push(i + 1);
                }
            }
            let (mut na, mut nb) = (a, b);
            rich::still(e, || {
                for &ps in starts.iter().rev() {
                    // The first line's text starts past the block's hidden language.
                    let ps = if ps as usize == ca { rich::past_tag(e, &t, ca, cb) as i32 } else { ps };
                    let n = if shift { t[ps as usize..].iter().take(4).take_while(|&&c| c == b' ' as u16).count() as i32 } else { -4 };
                    if n == 0 {
                        continue;
                    }
                    if n > 0 {
                        rich::set_sel(e, ps, ps + n);
                        rich::replace_sel(e, "");
                    } else {
                        rich::set_sel(e, ps, ps);
                        rich::replace_sel(e, "    ");
                    }
                    // What was after this line's start moves with it.
                    let moved = |p: i32| if p > ps { (p - n).max(ps) } else { p };
                    na = moved(na);
                    nb = moved(nb);
                }
                rich::set_sel(e, na, nb);
            });
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
/// its text as ordinary paragraphs. (A code block's text starts past its hidden language,
/// where the caret is put.)
pub(super) fn block_backspace(e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    // Only in a cell's first paragraph, which follows a row's start or another cell: said
    // by the marks before the caret's paragraph, without the whole text (each Backspace
    // copied it).
    let p = rich::paras_around(e, a.max(0) as usize, a.max(0) as usize);
    let ps = p.from + rich::para_start(&p.t, a - p.from as i32) as usize;
    let before = rich::raw_text(e, ps.saturating_sub(2), ps);
    if !(before.last() == Some(&rich::CELL) || before == [rich::ROW_START, rich::CR]) {
        return false;
    }
    let t = rich::text(e);
    match rich::cell_at(e, &t, a) {
        Some((rich::Block::Quote, _, ca, _)) if a == b && a as usize == ca => {
            rich::still(e, || rich::unwrap(e, a));
            true
        }
        Some((rich::Block::Code, _, ca, cb)) if a == b && (a as usize == ca || a as usize == rich::past_tag(e, &t, ca, cb)) => {
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
                rich::style_boxes_in(e, c - 2, c - 2);
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

/// Whether position `cp` is in a code block or a table, where lists aren't made.
pub(super) fn lists_refused(e: HWND, t: &[u16], cp: i32) -> bool {
    matches!(rich::cell_at(e, t, cp), Some((rich::Block::Code | rich::Block::Table, ..)))
}

/// The caret never rests in front of a checkbox or between it and its text (typed there,
/// a letter would bury the box in the line): it goes to the start of the item's text, or,
/// moving left (`left`), on to the end of the line before.
pub(super) fn caret_off_box(e: HWND, left: bool) {
    let (a, b) = rich::sel(e);
    if a != b {
        return;
    }
    // Its paragraph is all it takes (a click or an arrow key copied the whole note).
    let t = rich::paras_around(e, a.max(0) as usize, a.max(0) as usize);
    let ps = (t.from + rich::para_start(&t.t, a - t.from as i32) as usize) as i32;
    if !t.get(ps as usize).is_some_and(is_task) || (ps > 0 && t.get(ps as usize - 1) != Some(rich::CR)) {
        return;
    }
    let gap = if t.get(ps as usize + 1) == Some(b' ' as u16) { 2 } else { 1 };
    if a >= ps + gap {
        return;
    }
    let to = if left && ps > 0 { ps - 1 } else { ps + gap };
    rich::set_sel(e, to, to);
}

/// After RichEdit's own undo or redo, which selects what it put back: a selection holding a
/// checklist box (a tick undone or redone a step at a time) is let go, the caret after it.
/// Selected, the box's glyph showed beside its circle as a block of the selection's colour,
/// and the circle sat in a halo of it.
pub(super) fn selection_off_boxes(e: HWND) {
    let (a, b) = rich::sel(e);
    if a == b {
        return;
    }
    let (a, b) = (a.min(b).max(0) as usize, a.max(b).max(0) as usize);
    let from = a.saturating_sub(1);
    let t = rich::raw_text(e, from, b);
    let boxed = (a..b).any(|i| t.get(i - from).copied().is_some_and(is_task) && (i == 0 || t.get(i - 1 - from) == Some(&rich::CR)));
    if boxed {
        rich::set_sel(e, b as i32, b as i32);
        caret_off_box(e, false);
    }
}

/// Delete at the end of a line whose next line is a checklist item: the item's text joins
/// this line, without its box (which would sit in the middle of the line, invisible).
pub(super) fn delete_into_item(e: HWND) -> bool {
    let (a, b) = rich::sel(e);
    if a != b {
        return false;
    }
    // The paragraph mark at the caret and the box after it, not the whole text.
    let t = rich::raw_text(e, a.max(0) as usize, a.max(0) as usize + 3);
    if t.first() != Some(&rich::CR) || !t.get(1).copied().is_some_and(is_task) {
        return false;
    }
    let n = a as usize + 1;
    let gap = if t.get(2) == Some(&(b' ' as u16)) { 2 } else { 1 };
    rich::still(e, || {
        rich::set_sel(e, a, (n + gap) as i32);
        rich::replace_sel(e, "");
    });
    true
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

    /// Shift+Tab on a plain line takes a tab off its start and never types one; Backspace
    /// at a nested item's start steps it out a level, and only at the top level is it
    /// left to take the bullet.
    #[test]
    fn shift_tab_and_backspace_step_out() {
        let h = editor("T\r\tone\r\t\ttwo\rthree");
        caret(h, "one");
        assert!(rich::untab_lines(h));
        assert_eq!(plain(h), "T/^one/\t\ttwo/three");
        // Every line of a selection, one tab each.
        let (a, _) = rich::sel(h);
        rich::set_sel(h, a, rich::text(h).len() as i32);
        assert!(rich::untab_lines(h));
        assert_eq!(plain(h).replace('^', ""), "T/one/\ttwo/three");
        caret(h, "three");
        assert!(!rich::untab_lines(h), "no tab: nothing");
        assert_eq!(plain(h).replace('^', ""), "T/one/\ttwo/three");
        for md in ["T\n- a\n  - b\n    - c", "T\n1. a\n  1. b\n    1. c", "T\n- [ ] a\n  - [ ] b\n    - [ ] c"] {
            let h = editor("");
            rich::load_markdown(h, md);
            rich::style_boxes(h);
            let t = rich::text(h);
            let c = t.iter().rposition(|&ch| ch == b'c' as u16).unwrap() as i32;
            rich::set_sel(h, c, c);
            assert_eq!(rich::list_level(h), Some(2), "{md}");
            assert!(rich::backspace_outdents(h), "{md}");
            assert_eq!(rich::list_level(h), Some(1), "{md}");
            assert!(rich::backspace_outdents(h), "{md}");
            assert_eq!(rich::list_level(h), Some(0), "{md}");
            assert!(!rich::backspace_outdents(h), "{md}: at the top, the bullet goes");
            // Not at the item's start: a letter goes, as ever.
            rich::set_sel(h, c + 1, c + 1);
            assert!(!rich::backspace_outdents(h), "{md}");
            assert_eq!(rich::text(h), t, "{md}: no text changed");
        }
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
        // A last line with nothing but its indent: Enter makes a gap in the code; a second
        // empty line at the end leaves the block, and both go.
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/    a/    /    ^|]/end");
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
        // Empty lines in the middle of code stay, however many.
        let h = editor("Title\ra\rb\rend");
        caret(h, "a");
        let (s, _) = rich::sel(h);
        rich::set_sel(h, s, s + 3);
        block_command(h, Block::Code);
        caret(h, "a/b");
        let at = rich::sel(h).0 + 1;
        rich::set_sel(h, at, at);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/a//^/b|]/end");
        // Two empty lines that end the block, the caret on the second: out, and both go.
        let h = editor("Title\ra\rend");
        caret(h, "a");
        block_command(h, Block::Code);
        caret(h, "a|");
        let at = rich::sel(h).0 + 1;
        rich::set_sel(h, at, at);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/a//^|]/end");
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "Title/[/a|]/^end");
    }

    /// Backspace takes a block apart wherever it did when it looked through the whole
    /// text: at the start of a quote's or a code block's text (past its language), in a
    /// note, in a list item and nested, and nowhere else.
    #[test]
    fn backspace_finds_block_starts_as_before() {
        crate::common::i18n::init("en");
        let h = editor("");
        let md = "T\n> quoted\n> more\n\n```rust\nlet x = 1;\n```\n| a | b |\n| --- | --- |\n| 1 | 2 |\n- item\n    > under it\n> > nested\n\nend";
        rich::load_markdown(h, md);
        let t = rich::text(h);
        let was = |a: i32| match rich::cell_at(h, &t, a) {
            Some((rich::Block::Quote, _, ca, _)) => a as usize == ca,
            Some((rich::Block::Code, _, ca, cb)) => a as usize == ca || a as usize == rich::past_tag(h, &t, ca, cb),
            _ => false,
        };
        let mut starts = 0;
        for a in 0..=t.len() as i32 {
            // Where RichEdit puts the caret (never on a row's own marks).
            rich::set_sel(h, a, a);
            let a = rich::sel(h).0;
            if was(a) {
                starts += 1;
                // Taken apart, then the note as it was again.
                assert!(block_backspace(h), "at {a}");
                rich::load_markdown(h, md);
                assert_eq!(rich::text(h), t);
            } else {
                assert!(!block_backspace(h), "at {a}");
            }
        }
        assert!(starts >= 4, "{starts}");
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

    /// Lines pasted into a checklist item become items, each with its box and look; their
    /// own markers become the items' (ticked or not), and one Ctrl+Z takes it all back.
    #[test]
    fn lines_pasted_into_an_item_are_items() {
        let h = editor("T\r\u{2611} one");
        rich::style_boxes(h);
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        assert!(!paste_into_checklist(h, "just one line\n"), "one line is just text");
        assert!(paste_into_checklist(h, " more\r\ntwo\n\n- [x] three\n- four"));
        assert_eq!(plain(h), "T/\u{2611} one more/\u{2610} two/\u{2611} three/\u{2610} four^");
        assert_eq!(rich::to_markdown(h, true), "# T\n- [x] one more\n- [ ] two\n- [x] three\n- [ ] four");
        // Each new line has the item's look: the hanging indent of the one pasted into.
        let indent = |h: HWND, cp: i32| {
            rich::set_sel(h, cp, cp);
            let mut pf = rich::ParaFormat2::new();
            rich::send(h, rich::EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
            (pf.start_indent, pf.offset)
        };
        let t = rich::text(h);
        let boxes: Vec<i32> = t.iter().enumerate().filter(|(_, c)| is_task(**c)).map(|(i, _)| i as i32).collect();
        let first = indent(h, boxes[0]);
        assert_ne!(first.1, 0);
        for &b in &boxes[1..] {
            assert_eq!(indent(h, b), first);
        }
        assert!(rich::undo_group(h));
        assert_eq!(plain(h).replace('^', ""), "T/\u{2611} one");
        // Into an empty item: its first line's marker goes too.
        let h = editor("T\r\u{2610} ");
        rich::style_boxes(h);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        assert!(paste_into_checklist(h, "- milk\n- eggs"));
        assert_eq!(plain(h), "T/\u{2610} milk/\u{2610} eggs^");
        // Not in an item: not ours.
        let h = editor("T\rplain");
        rich::set_sel(h, 4, 4);
        assert!(!paste_into_checklist(h, "a\nb"));
        // A box pasted at a line's start (as text) is styled as an item once boxes are.
        let h = editor("T\r");
        rich::set_sel(h, 2, 2);
        rich::replace_sel(h, "\u{2610} pasted");
        rich::style_boxes(h);
        assert_ne!(indent(h, 2).1, 0);
    }

    /// A nested bullet made a checklist item ("[] " typed on it, or Ctrl+Shift+L) stays
    /// as deep as it was.
    #[test]
    fn nested_bullets_become_nested_items() {
        let h = editor("");
        rich::load_markdown(h, "T\n- a\n  - b\n    - c");
        let at = |h: HWND, s: &str| {
            let w: Vec<u16> = s.encode_utf16().collect();
            rich::text(h).windows(w.len()).position(|v| v == &w[..]).unwrap() as i32
        };
        let b = at(h, "b");
        rich::set_sel(h, b, b);
        rich::replace_sel(h, "[]");
        assert!(try_auto_list(null_mut(), h));
        assert_eq!(rich::to_markdown(h, true), "# T\n- a\n  - [ ] b\n    - c");
        let c = at(h, "c");
        rich::set_sel(h, c, c);
        toggle_task_lines(h);
        assert_eq!(rich::to_markdown(h, true), "# T\n- a\n  - [ ] b\n    - [ ] c");
        // Stored and opened again: the same.
        let h2 = editor("");
        rich::set_rtf(h2, &rich::get_rtf(h));
        rich::style_boxes(h2);
        assert_eq!(rich::to_markdown(h2, true), "# T\n- a\n  - [ ] b\n    - [ ] c");
    }

    /// Enter in a table's last cell, empty or on an empty line after its text, goes on
    /// below the table (anywhere else it's a line in the cell); and "[x] " typed makes a
    /// ticked item, whose text is done text.
    #[test]
    fn leaving_a_table_and_ticked_items() {
        let h = editor("");
        rich::load_markdown(h, "T\n| a | b |\n| --- | --- |\n| 1 | |\nafter");
        // The empty last cell: out, to the line below.
        let t = rich::text(h);
        let rows = rich::rows(&t);
        let (_, last) = *rich::cells(&t, *rows.last().unwrap()).last().unwrap();
        rich::set_sel(h, last as i32, last as i32);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "T/[/a|b|]/[/1||]/^after");
        // Any other cell: a line in it.
        caret(h, "1|");
        assert_eq!(block_enter(h), None);
        // The last cell with text: Enter makes a line in it, Enter again on that empty
        // line goes out and takes the line away.
        rich::set_sel(h, last as i32, last as i32);
        rich::replace_sel(h, "2");
        assert_eq!(block_enter(h), None);
        rich::replace_sel(h, "\r");
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(plain(h), "T/[/a|b|]/[/1|2|]/^after");
        assert_eq!(rich::to_markdown(h, true), "# T\n| a | b |\n| --- | --- |\n| 1 | 2 |\nafter");
        // A table at the very end: out to the line after it.
        let h = editor("");
        rich::load_markdown(h, "T\n| a |\n| --- |\n| |");
        let t = rich::text(h);
        let (_, last) = *rich::cells(&t, *rich::rows(&t).last().unwrap()).last().unwrap();
        rich::set_sel(h, last as i32, last as i32);
        assert_eq!(block_enter(h), Some(true));
        assert!(plain(h).ends_with("]/^"), "{}", plain(h));
        // "[x] " typed.
        let h = editor("T\r[x]");
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        assert!(try_auto_list(null_mut(), h));
        rich::replace_sel(h, "done");
        assert_eq!(plain(h), "T/\u{2611} done^");
        assert_eq!(rich::to_markdown(h, true), "# T\n- [x] done");
        rich::set_sel(h, end + 1, end + 3);
        assert_ne!(rich::char_format(h, rich::SCF_SELECTION).effects & CFM_STRIKEOUT, 0, "done text is struck");
    }

    /// A tick restyles its own item (struck, faded) and an untick brings its ink back;
    /// a new item from Enter or "[] " has the item look, while the rest are left alone.
    #[test]
    fn ticks_and_new_items_restyle_their_own_line() {
        let h = editor("T\r\u{2610} one\r\u{2610} two");
        rich::set_box_colors(h, 0xFFFFFF, 0x888888, 0x1D1D1F);
        rich::style_boxes(h);
        // Where `w` is, in UTF-16 units (a box is three bytes as UTF-8).
        let pos = |h: HWND, w: &str| {
            let w: Vec<u16> = w.encode_utf16().collect();
            rich::text(h).windows(w.len()).position(|v| v == &w[..]).unwrap() as i32
        };
        let look = |h: HWND, word: &str| {
            let at = pos(h, word);
            rich::set_sel(h, at, at + word.len() as i32);
            let cf = rich::char_format(h, rich::SCF_SELECTION);
            let mut pf = rich::ParaFormat2::new();
            rich::send(h, rich::EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
            (cf.effects & CFM_STRIKEOUT != 0, cf.color, pf.offset)
        };
        let (_, ink, offset) = look(h, "two");
        let done = crate::win::util::cref(0x888888);
        let two = pos(h, "two") - 2;
        toggle_check(h, two);
        assert_eq!(look(h, "two"), (true, done, offset));
        assert_eq!(look(h, "one"), (false, ink, offset));
        toggle_check(h, two);
        assert_eq!(look(h, "two"), (false, ink, offset));
        // Enter after "one": the new item, typed into, looks like the others.
        let at = pos(h, "one") + 3;
        rich::set_sel(h, at, at);
        assert!(handle_enter(null_mut(), h));
        rich::replace_sel(h, "new");
        assert_eq!(look(h, "new"), (false, ink, offset));
        // "[] " on a line of its own.
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        rich::replace_sel(h, "\r[]");
        assert!(try_auto_list(null_mut(), h));
        rich::replace_sel(h, "typed");
        assert_eq!(look(h, "typed"), (false, ink, offset));
    }

    /// Timing (run with --ignored --nocapture): a tick, and Enter on an item, in a
    /// checklist of 2,000 items.
    #[test]
    #[ignore]
    fn probe_checklist_costs() {
        let mut md = String::from("T\n");
        for i in 0..2000 {
            md.push_str(&format!("- [ ] item {i}\n"));
        }
        let h = editor("");
        rich::load_markdown(h, md.trim_end());
        rich::style_boxes(h);
        let boxes: Vec<i32> = rich::text(h).iter().enumerate().filter(|(_, c)| is_task(**c)).map(|(i, _)| i as i32).collect();
        let n = 10;
        let t0 = std::time::Instant::now();
        for k in 0..n {
            toggle_check(h, boxes[1000 + k as usize]);
        }
        let tick = t0.elapsed() / n;
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        let t0 = std::time::Instant::now();
        for k in 0..n {
            rich::replace_sel(h, &format!("x{k}"));
            handle_enter(null_mut(), h);
        }
        let enter = t0.elapsed() / n;
        eprintln!("PROBE 2,000 items: tick {tick:?}, Enter {enter:?}");
    }

    /// Two ticks undone: the first by its group, the second by RichEdit's own undo, which
    /// selects what it put back; with a box in it, the selection goes and the caret is after it.
    #[test]
    fn undoing_ticks_leaves_no_box_selected() {
        let h = editor("T\r\u{2610} task");
        rich::style_boxes(h);
        rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
        let end = rich::text(h).len() as i32;
        rich::set_sel(h, end, end);
        toggle_check_at_caret(h);
        toggle_check_at_caret(h);
        assert!(rich::undo_group(h));
        assert!(!rich::undo_group(h), "one group a time");
        const EM_UNDO: u32 = 0x00C7;
        assert_eq!(rich::send(h, EM_UNDO, 0, 0), 1);
        assert_eq!(plain(h).replace('^', ""), "T/\u{2610} task");
        selection_off_boxes(h);
        let (a, b) = rich::sel(h);
        assert_eq!(a, b, "no selection");
        assert!(a >= 4, "after the box: {a}");
        // A selection of text alone stays.
        rich::set_sel(h, 4, 6);
        selection_off_boxes(h);
        assert_eq!(rich::sel(h), (4, 6));
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

    /// A word struck out in an open checklist item stays struck through restyling, and
    /// ticking and unticking the item.
    #[test]
    fn struck_words_in_open_items_stay() {
        let md = "T\n- [ ] keep ~~this~~ struck\n- [x] done item";
        let h = editor("");
        rich::load_markdown(h, md);
        rich::style_boxes(h);
        assert_eq!(rich::to_markdown(h, true), format!("# {md}"));
        // Another box ticked and unticked: the struck word is still struck.
        let t = rich::text(h);
        let done = t.iter().position(|&c| c == rich::CHECKED).unwrap() as i32;
        toggle_check(h, done);
        toggle_check(h, done);
        assert_eq!(rich::to_markdown(h, true), format!("# {md}"));
    }

    /// The caret is kept off the box, and Delete joins the next item's text, not its box.
    #[test]
    fn caret_stays_off_the_box() {
        let h = editor("T\r\u{2610} one\r\u{2610} two");
        rich::style_boxes(h);
        let t = rich::text(h);
        let first = t.iter().position(|&c| c == rich::BOX).unwrap() as i32;
        // Home on the item: after the box.
        rich::set_sel(h, first, first);
        caret_off_box(h, false);
        assert_eq!(rich::sel(h), (first + 2, first + 2));
        // Between the box and the space: the same.
        rich::set_sel(h, first + 1, first + 1);
        caret_off_box(h, false);
        assert_eq!(rich::sel(h), (first + 2, first + 2));
        // Left from the text's start: on to the line before.
        rich::set_sel(h, first, first);
        caret_off_box(h, true);
        assert_eq!(rich::sel(h), (first - 1, first - 1));
        // Delete at the end of "one": "two" joins it, without its box.
        let end_one = first + 5;
        rich::set_sel(h, end_one, end_one);
        assert!(delete_into_item(h));
        assert_eq!(plain(h), "T/\u{2610} one^two");
    }

    /// In code: Tab and Shift+Tab move every selected line, a "- " stays code, and so
    /// does an address.
    #[test]
    fn code_keeps_code() {
        let h = editor("");
        rich::load_markdown(h, "T\n```\none\n  two\nthree\n```");
        let t = rich::text(h);
        let one = t.windows(3).position(|w| w == [b'o' as u16, b'n' as u16, b'e' as u16]).unwrap() as i32;
        let three = t.windows(5).position(|w| *w == "three".encode_utf16().collect::<Vec<_>>()[..]).unwrap() as i32;
        rich::set_sel(h, one, three + 2);
        assert!(block_tab(h, false));
        assert_eq!(rich::to_markdown(h, false), "T\n```\n    one\n      two\n    three\n```");
        // (Reading the note out moves the selection: select the lines again.)
        rich::set_sel(h, one, three + 10);
        assert!(block_tab(h, true));
        assert!(block_tab(h, true));
        assert_eq!(rich::to_markdown(h, false), "T\n```\none\ntwo\nthree\n```");
        // "- " at a line's start in code stays as typed.
        rich::set_sel(h, one, one);
        rich::replace_sel(h, "-");
        assert!(!try_auto_list(null_mut(), h));
    }

    /// A code block's language is hidden at its start: Enter, Tab and Backspace there work
    /// as they do without one.
    #[test]
    fn code_with_a_language_edits_as_code() {
        let start = |h: HWND| {
            let t = rich::text(h);
            let (_, _, ca, cb) = rich::cell_at(h, &t, 4).unwrap();
            rich::past_tag(h, &t, ca, cb) as i32
        };
        // Tab and Shift+Tab move the first line too, after the language.
        let h = editor("");
        rich::load_markdown(h, "T\n```rust\none\ntwo\n```");
        let a = start(h);
        rich::set_sel(h, a, a + 6);
        assert!(block_tab(h, false));
        assert_eq!(rich::to_markdown(h, true), "# T\n```rust\n    one\n    two\n```");
        let a = start(h);
        rich::set_sel(h, a, a + 10);
        assert!(block_tab(h, true));
        assert_eq!(rich::to_markdown(h, true), "# T\n```rust\none\ntwo\n```");
        // Enter on an indented first line keeps its indent.
        let h = editor("");
        rich::load_markdown(h, "T\n```py\n  x\n```");
        let a = start(h);
        rich::set_sel(h, a + 3, a + 3);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(rich::to_markdown(h, true), "# T\n```py\n  x\n  \n```");
        // Enter in an empty block takes it away, language and all.
        let h = editor("");
        rich::load_markdown(h, "T\n```js\n```\nend");
        let a = start(h);
        rich::set_sel(h, a, a);
        assert_eq!(block_enter(h), Some(true));
        assert_eq!(rich::to_markdown(h, true), "# T\nend");
        // Backspace at the start of its text takes the block apart.
        let h = editor("");
        rich::load_markdown(h, "T\n```js\nlet a\n```");
        let a = start(h);
        rich::set_sel(h, a, a);
        assert!(block_backspace(h));
        assert_eq!(rich::to_markdown(h, true), "# T\nlet a");
    }

    /// Enter on a code block's first line: the new line doesn't take the first line's air
    /// above it, so the lines stay evenly spaced.
    #[test]
    fn enter_on_a_code_blocks_first_line_keeps_it_even() {
        // Where each line of the code block holding `cp` starts, and its paragraph spacing.
        let lines = |h: HWND| {
            let t = rich::text(h);
            let (_, _, ca, cb) = rich::cell_at(h, &t, 4).unwrap();
            let starts: Vec<usize> = std::iter::once(ca).chain((ca..cb).filter(|&i| t[i] == rich::CR).map(|i| i + 1)).filter(|&i| i < cb).collect();
            starts
                .into_iter()
                .map(|i| {
                    rich::set_sel(h, i as i32, i as i32);
                    let mut pf = rich::ParaFormat2::new();
                    rich::send(h, rich::EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                    (rich::baseline_at(h, i as i32).unwrap_or(-1), pf.space_before, pf.space_after)
                })
                .collect::<Vec<_>>()
        };
        let h = editor("");
        rich::load_markdown(h, "T\n```\none\ntwo\nthree\n```");
        rich::style_blocks(h);
        let t = rich::text(h);
        let (_, _, ca, cb) = rich::cell_at(h, &t, 4).unwrap();
        let a = rich::past_tag(h, &t, ca, cb) as i32;
        rich::set_sel(h, a + 3, a + 3);
        assert_eq!(block_enter(h), Some(true));
        let l = lines(h);
        assert_eq!(l.len(), 4);
        let gaps: Vec<i32> = l.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert!(gaps.iter().all(|&g| g == gaps[0]), "line gaps {gaps:?}, lines (baseline, before, after) {l:?}");
    }

    /// A selection from a line into the quote below it: the line becomes a code block,
    /// the quote stays a quote, and no row marks end up as text.
    #[test]
    fn block_command_stops_at_a_block() {
        let h = editor("");
        rich::load_markdown(h, "T\nline one\nline two\n> quoted");
        let t = rich::text(h);
        let one = t.iter().position(|&c| c == b'l' as u16).unwrap() as i32;
        let q = t.iter().position(|&c| c == b'q' as u16).unwrap() as i32;
        rich::set_sel(h, one, q + 3);
        block_command(h, Block::Code);
        assert_eq!(rich::to_markdown(h, true), "# T\n```\nline one\nline two\n```\n> quoted");
    }

    /// A Shift+Enter line break goes out as a line (in a table cell, as <br>), never as
    /// a control character.
    #[test]
    fn soft_breaks_go_out_as_lines() {
        let h = editor("T\rfirst\u{B}second");
        assert_eq!(rich::to_markdown(h, true), "# T\nfirst\nsecond");
        assert_eq!(rich::to_markdown(h, false), "T\nfirst\nsecond");
        let h = editor("");
        rich::load_markdown(h, "T\n| a |\n| --- |\n| x |");
        let t = rich::text(h);
        let x = t.iter().position(|&c| c == b'x' as u16).unwrap() as i32;
        rich::set_sel(h, x + 1, x + 1);
        rich::replace_sel(h, "\u{B}y");
        assert_eq!(rich::to_markdown(h, true), "# T\n| a |\n| --- |\n| x<br>y |");
    }

    /// One Ctrl+Z takes back a change over many lines whole (a checklist toggled over 60,
    /// Tab over 60 lines of code), and one Ctrl+Y brings it again.
    #[test]
    fn many_lines_undo_in_one() {
        let lines: Vec<String> = (0..60).map(|i| format!("line {i}")).collect();
        for code in [false, true] {
            let h = editor(&format!("Title\r{}", lines.join("\r")));
            rich::send(h, rich::EM_SETUNDOLIMIT, 110, 0);
            if code {
                rich::wrap(h, 6, rich::text(h).len() as i32, Block::Code);
            }
            rich::send(h, rich::EM_EMPTYUNDOBUFFER, 0, 0);
            let before = plain(h).replace('^', "");
            let t = rich::text(h);
            let end = t.iter().rposition(|&c| c == b'9' as u16).unwrap() as i32 + 1;
            let start = if code { rich::cell_at(h, &t, end).unwrap().2 as i32 } else { 6 };
            rich::set_sel(h, start, end);
            if code {
                assert!(block_tab(h, false));
            } else {
                rich::still(h, || toggle_task_lines(h));
            }
            let after = plain(h).replace('^', "");
            assert_ne!(after, before);
            assert!(rich::undo_group(h));
            assert_eq!(plain(h).replace('^', ""), before, "code: {code}");
            assert!(rich::redo_group(h));
            assert_eq!(plain(h).replace('^', ""), after, "code: {code}");
            // Restyling the blocks after typing is no step of its own.
            const EM_UNDO: u32 = 0x00C7;
            let n = rich::text(h).len() as i32;
            rich::set_sel(h, n, n);
            rich::replace_sel(h, " typed");
            rich::style_blocks(h);
            rich::set_block_colors(h, 0x202020, 0xEEEEEE, true);
            rich::style_blocks(h);
            assert_eq!(rich::send(h, EM_UNDO, 0, 0), 1);
            assert_eq!(plain(h).replace('^', ""), after, "code: {code}");
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
            rich::restyle(h);
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

    /// A note with checklist items (one nested, one ticked), links in sentences, a file
    /// and a table, long enough to scroll.
    fn pointer_note() -> HWND {
        let h = editor("");
        let mut md = String::from("Pointer\n");
        for i in 0..40 {
            md.push_str(&match i % 5 {
                0 => format!("- [ ] task {i} with [a link](https://example.com/{i}) in it\n"),
                1 => format!("  - [x] done {i}\n"),
                2 => format!("see https://example.org/{i} and [another](https://x.org/{i}) here\n"),
                3 => format!("[plan {i}.pdf](attachment:plan%20{i}.pdf)\n"),
                _ => format!("| a {i} | b |\n| --- | --- |\n| [c](https://c.example) | d |\nplain {i}\n"),
            });
        }
        rich::load_markdown(h, &md);
        rich::link_addresses(h, 0, rich::text(h).len());
        rich::style_boxes(h);
        h
    }

    /// The checkbox and link under a point, worked out from the paragraphs around it and
    /// kept while the pointer stays on one character, are what reading the whole text
    /// for every move found.
    #[test]
    fn pointer_finds_what_the_whole_text_did() {
        crate::common::i18n::init("en");
        let e = pointer_note();
        let lh = (rich::body_size() as f32 / 15.0 * 1.45) as i32;
        // As every pointer move worked it out before.
        let box_was = |x: i32, y: i32| {
            let cp = rich::char_from_pos(e, x, y);
            let t = rich::text(e);
            let ps = rich::para_start(&t, cp.max(0));
            let after = if t.get(cp.max(0) as usize) == Some(&rich::CR) { cp + 1 } else { ps };
            [ps, cp, cp - 1, after].into_iter().find(|&c| {
                c >= 0 && t.get(c as usize).copied().is_some_and(is_task) && (c == 0 || t[c as usize - 1] == rich::CR) && {
                    let (cx, cy, r) = crate::win::editor::checks::circle(e, c, t.get(c as usize + 1).copied());
                    (x as f32 - cx).abs() <= r && (y as f32 - cy).abs() <= r
                }
            })
        };
        let link_was = |x: i32, y: i32| {
            let t = rich::text(e);
            let cp = rich::char_from_pos(e, x, y);
            let over = |c: i32| {
                let (p, q) = (rich::pos_from_char(e, c), rich::pos_from_char(e, c + 1));
                let right = if q.y == p.y && q.x > p.x { q.x } else { p.x };
                let next_line = t.get(c.max(0) as usize..).and_then(|r| r.iter().position(|&ch| ch == 13)).map(|i| c + i as i32 + 1);
                let bottom = next_line.filter(|&n| (n as usize) < t.len()).map_or(p.y + lh, |n| rich::pos_from_char(e, n).y);
                c >= 0 && x >= p.x && x < right && y >= p.y && y < bottom.max(p.y + lh)
            };
            let hit = [cp, cp - 1].into_iter().find(|&c| over(c))?;
            if let Some((a, b, url)) = rich::item_link(e, &t, hit as usize) {
                return Some((a, b, url, hit));
            }
            rich::link_at(e, hit).filter(|l| hit >= l.0 && hit < l.1).map(|(a, b, url)| (a, b, url, hit))
        };
        let (mut boxes, mut links) = (0, 0);
        for scroll in [0, 250] {
            rich::set_scroll_y(e, scroll);
            hover_forget();
            for y in (0..300).step_by(3) {
                for x in (0..400).step_by(4) {
                    let b = checkbox_at(e, x, y);
                    assert_eq!(b, box_was(x, y), "box at {x},{y}");
                    let l = link_under(e, x, y);
                    assert_eq!(l, link_was(x, y), "link at {x},{y}");
                    boxes += b.is_some() as usize;
                    links += l.is_some() as usize;
                }
            }
        }
        assert!(boxes > 20 && links > 100, "{boxes} box and {links} link points");
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(e) };
    }

    /// A pointer that jumps onto a circle (from the text, the margin, another line) finds
    /// the box there, as one that moved onto it a pixel at a time does; and every point
    /// well inside a drawn circle, the last item's too, is on its box.
    #[test]
    fn pointer_jumping_onto_a_circle_finds_its_box() {
        crate::common::i18n::init("en");
        let e = editor("");
        rich::load_markdown(e, "Title\nsome text here\n- [ ] alpha task\n- [ ] beta task\n- [ ] gamma task");
        rich::style_boxes(e);
        crate::win::editor::checks::set_style(e, 0, 0xFFFFFF, 1.0);
        let t = rich::text(e);
        let all: Vec<i32> = (0..t.len() as i32).filter(|&c| is_task(t[c as usize]) && (c == 0 || t[c as usize - 1] == rich::CR)).collect();
        assert_eq!(all.len(), 3);
        let mut inside = 0;
        for &c in &all {
            let (cx, cy, r) = crate::win::editor::checks::circle(e, c, t.get(c as usize + 1).copied());
            for y in (cy - r + 2.0) as i32..=(cy + r - 2.0) as i32 {
                for x in (cx - r + 2.0).max(0.0) as i32..=(cx + r - 2.0) as i32 {
                    for (qx, qy) in [(x + 60, y), (x + 120, y), (x, y + 40), (x, y - 30), (300, 290), (2, y)] {
                        hover_forget();
                        let _ = checkbox_at(e, qx, qy);
                        let _ = link_under(e, qx, qy);
                        assert_eq!(checkbox_at(e, x, y), Some(c), "onto {x},{y} from {qx},{qy}");
                        assert_eq!(checkbox_now(e, x, y), Some(c), "pressed at {x},{y}");
                    }
                    inside += 1;
                }
            }
        }
        assert!(inside > 300, "{inside} points");
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(e) };
    }

    /// Timing (run with --ignored --nocapture): what a pointer move over a long note costs
    /// (the checkbox under it, as WM_MOUSEMOVE and WM_SETCURSOR each ask, and the link
    /// under it, for its tip), onto another character and within the same one.
    #[test]
    #[ignore]
    fn probe_pointer_moves() {
        crate::common::i18n::init("en");
        let e = pointer_note();
        let md = rich::to_markdown(e, true).repeat(40);
        rich::load_markdown(e, &md);
        rich::style_boxes(e);
        rich::set_scroll_y(e, 20_000);
        eprintln!("PROBE --- {} characters", rich::text_len(e));
        let moves: Vec<(i32, i32)> = (0..400).map(|k| (20 + (k * 7) % 360, 10 + (k * 13) % 280)).collect();
        let (mut new_char, mut same_char) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        let runs = 5;
        for _ in 0..runs {
            for &(x, y) in &moves {
                hover_forget();
                // Onto another character, then a pixel on (mostly the same character).
                for (x, total) in [(x, &mut new_char), (x + 1, &mut same_char)] {
                    let t0 = std::time::Instant::now();
                    std::hint::black_box(checkbox_at(e, x, y));
                    std::hint::black_box(checkbox_at(e, x, y));
                    std::hint::black_box(link_under(e, x, y));
                    *total += t0.elapsed();
                }
            }
        }
        let n = runs * moves.len() as u32;
        let (new_char, same_char) = (new_char / n, same_char / n);
        eprintln!("PROBE pointer move: onto another character {new_char:?}, within one {same_char:?}");
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(e) };
    }
}
