// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Note commands (menus, toolbar, shortcuts) and the keyboard map.

use super::*;

/// Run a note command that needs the editor (formatting, clipboard) or forward to the app.
pub(super) fn run_command(h: HWND, cmd: u32) {
    let Some(st) = state(h) else { return };
    let (id, e) = (st.id, st.edit);
    // Shown in one frame when it's done (`rich::quiet`), however many steps it takes.
    let fmt = |f: &dyn Fn()| {
        rich::quiet(e, f);
        mark_dirty(h);
    };
    match cmd {
        NC_BOLD => fmt(&|| rich::toggle_effect(e, CFM_BOLD)),
        NC_ITALIC => fmt(&|| rich::toggle_effect(e, CFM_ITALIC)),
        NC_UNDERLINE => fmt(&|| rich::toggle_effect(e, CFM_UNDERLINE)),
        NC_STRIKE => fmt(&|| rich::toggle_effect(e, CFM_STRIKEOUT)),
        // Not in code or a table (see `lists_refused`).
        NC_BULLETS | NC_NUMBERS | NC_TASK if lists_refused(e, &rich::text(e), rich::sel(e).0) => {}
        NC_BULLETS => fmt(&|| rich::toggle_numbering(e, PFN_BULLET)),
        NC_NUMBERS => fmt(&|| rich::toggle_numbering(e, PFN_ARABIC)),
        NC_TASK => fmt(&|| rich::still(e, || toggle_task_lines(e))),
        NC_TOGGLE_CHECK => fmt(&|| toggle_check_at_caret(e)),
        NC_CUT => fmt(&|| {
            if !rich::delete_keeping_item(e, true) {
                rich::send(e, WM_CUT, 0, 0);
            }
        }),
        NC_COPY => {
            rich::send(e, WM_COPY, 0, 0);
        }
        NC_PASTE => rich::quiet(e, || paste(h)),
        NC_SELECT_ALL => rich::set_sel(e, 0, -1),
        NC_ALIGN_LEFT => fmt(&|| rich::set_alignment(e, rich::PFA_LEFT)),
        NC_ALIGN_CENTER => fmt(&|| rich::set_alignment(e, rich::PFA_CENTER)),
        NC_ALIGN_RIGHT => fmt(&|| rich::set_alignment(e, rich::PFA_RIGHT)),
        NC_QUOTE => fmt(&|| rich::still(e, || block_command(e, rich::Block::Quote))),
        NC_CODE => fmt(&|| rich::still(e, || block_command(e, rich::Block::Code))),
        NC_TABLE => fmt(&|| rich::still(e, || block_command(e, rich::Block::Table))),
        NC_ROW_ABOVE | NC_ROW_BELOW | NC_COL_LEFT | NC_COL_RIGHT | NC_ROW_DELETE | NC_COL_DELETE | NC_TABLE_DELETE => fmt(&|| {
            let (a, _) = rich::sel(e);
            rich::still(e, || match cmd {
                NC_ROW_ABOVE => rich::insert_row(e, a, false),
                NC_ROW_BELOW => rich::insert_row(e, a, true),
                NC_COL_LEFT => rich::insert_column(e, a, false),
                NC_COL_RIGHT => rich::insert_column(e, a, true),
                NC_ROW_DELETE => rich::delete_row(e, a),
                NC_COL_DELETE => rich::delete_column(e, a),
                _ => rich::delete_block(e, a),
            });
        }),
        NC_LINK => link_command(h),
        NC_CLOSE | NC_NEXT | NC_PREV if st.embedded => unsafe {
            let parent = GetParent(h);
            SetFocus(parent);
            if cmd != NC_CLOSE {
                PostMessageW(parent, WM_KEYDOWN, (if cmd == NC_NEXT { VK_DOWN } else { VK_UP }) as usize, 0);
            }
        },
        NC_PIN if st.embedded => {}
        NC_CLOSE => unsafe {
            PostMessageW(h, WM_CLOSE, 0, 0);
        },
        NC_NEXT | NC_PREV => {
            flush(h);
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(h, &mut r) };
            if app::step_note(id, if cmd == NC_NEXT { 1 } else { -1 }, r) {
                unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
            }
        }
        _ => {
            flush(h);
            app::note_command(id, cmd);
        }
    }
}

/// Quote, code or table at the caret. A quote or code block there already is taken apart
/// (or turned into the other); otherwise the selected paragraphs go into a new one, or an
/// empty one opens below the title. A table always starts empty.
pub(super) fn block_command(e: HWND, kind: rich::Block) {
    use rich::Block;
    let (a, b) = rich::sel(e);
    let t = rich::text(e);
    if let Some((have, ..)) = rich::cell_at(e, &t, a) {
        if have == Block::Table || kind == Block::Table {
            return;
        }
        let Some((start, end)) = rich::unwrap(e, a) else { return };
        if have != kind {
            rich::wrap(e, start, end, kind);
        }
        return;
    }
    // A selection running on into a block stops before it: the lines above become the
    // new block, and the one below stays whole (its row marks never go into another).
    let b = match t[a as usize..(b.max(a) as usize).min(t.len())].iter().position(|&c| c == rich::ROW_START) {
        Some(i) => (a + i as i32 - 1).max(a),
        None => b,
    };
    let (ps, pe) = (rich::para_start(&t, a), rich::para_end(&t, b.max(a)));
    // The title stays the title: a block goes below it.
    if ps == 0 {
        if pe as usize >= t.len() {
            rich::set_sel(e, pe, pe);
            rich::insert_text(e, "\r");
        }
        let at = pe + 1;
        let cell = rich::insert_block(e, at, kind, if kind == Block::Table { 2 } else { 1 }, if kind == Block::Table { 3 } else { 1 });
        rich::set_sel(e, cell, cell);
        return;
    }
    if kind == Block::Table {
        // An empty line becomes the table; after anything else, it goes below.
        let at = if ps == pe { ps } else { pe + 1 };
        if at as usize > t.len() || (ps != pe && pe as usize >= t.len()) {
            rich::set_sel(e, pe, pe);
            rich::insert_text(e, "\r");
        }
        let over = if ps == pe && t.get(ps as usize) == Some(&rich::CR) { 1 } else { 0 };
        let cell = rich::insert_block_over(e, at, over, kind, 2, 3);
        // A new line made for it below a list item is plain text, not another item.
        let t = rich::text(e);
        if let Some((_, rows)) = rich::block_at(e, &t, cell)
            && let Some(end) = rows.last().map(|r| r.end as i32)
            && rich::para_start(&t, end) == rich::para_end(&t, end)
        {
            rich::set_sel(e, end, end);
            rich::set_numbering(e, 0);
        }
        rich::set_sel(e, cell, cell);
        return;
    }
    rich::wrap(e, ps, pe, kind);
}

/// Ctrl+K: link the selection (or edit the link at the caret). An address on the
/// clipboard is offered, ready for Enter.
fn link_command(h: HWND) {
    let Some(st) = state(h) else { return };
    let e = st.edit;
    let (mut a, mut b) = rich::sel(e);
    // A selection made with Ctrl+Shift+Right takes the space after the word: link the words.
    {
        let t = rich::text(e);
        let blank = |i: i32| t.get(i as usize).is_some_and(|&c| c == b' ' as u16 || c == b'\t' as u16);
        while b > a && blank(b - 1) {
            b -= 1;
        }
        while a < b && blank(a) {
            a += 1;
        }
    }
    let existing = rich::link_at(e, a);
    let clip = crate::win::util::clipboard_text().map(|s| s.trim().to_string()).filter(|s| rich::looks_like_url(s));
    // Under the start of what's being linked.
    let t = rich::text(e);
    let from = existing.as_ref().map_or(a, |l| rich::name_start(&t, l.0, l.1));
    let mut p = rich::pos_from_char(e, from);
    p.y += (rich::body_size() as f32 / 15.0 * 1.6) as i32 + (6.0 * st.s) as i32;
    unsafe { ClientToScreen(e, &mut p) };
    let current = existing.as_ref().map(|l| l.2.clone()).or(clip).unwrap_or_default();
    let answer = crate::win::ui::linkbox::ask(h, p, &current);
    unsafe { SetFocus(e) };
    let Some(input) = answer else { return };
    let input = input.trim();
    match (existing, input.is_empty()) {
        (Some((la, lb, _)), true) => {
            rich::remove_link(e, la, lb);
            mark_dirty(h);
        }
        (None, true) => {}
        (Some((la, lb, _)), false) => {
            // Same name, new address.
            let name = lb - rich::name_start(&t, la, lb);
            rich::remove_link(e, la, lb);
            rich::set_link(e, la, la + name, &rich::normalize_url(input));
            mark_dirty(h);
        }
        (None, false) => {
            let url = rich::normalize_url(input);
            let (a, b) = if a == b {
                // Nothing selected: the address goes in as its own name.
                rich::set_sel(e, a, a);
                rich::replace_sel(e, input);
                (a, a + input.encode_utf16().count() as i32)
            } else {
                (a, b)
            };
            rich::set_link(e, a, b, &url);
            let end = link_end(e, a, b);
            rich::set_sel(e, end, end);
            mark_dirty(h);
        }
    }
}

/// Paste as plain text; an address pasted over selected text links it instead.
pub(super) fn paste(h: HWND) {
    let Some(st) = state(h) else { return };
    let e = st.edit;
    let (a, b) = rich::sel(e);
    // Files copied in Explorer come in as attachments.
    let files = super::attach::clipboard_files();
    if !files.is_empty() {
        rich::set_sel(e, a, b);
        if a != b {
            rich::replace_sel(e, "");
        }
        if super::attach::add(e, a, &files) > 0 {
            mark_dirty(h);
        }
        super::scrollbar::keep_caret_in_view(e);
        return;
    }
    let clip = crate::win::util::clipboard_text();
    if a != b
        && let Some(url) = clip.as_deref().map(str::trim).filter(|s| rich::looks_like_url(s))
        && !rich::text(e)[a as usize..b as usize].contains(&13)
    {
        rich::set_link(e, a, b, url);
        let end = link_end(e, a, b);
        rich::set_sel(e, end, end);
    } else if clip.as_deref().is_some_and(|c| paste_into_checklist(e, c) || rich::paste_markdown(e, c)) {
        let (end, _) = rich::sel(e);
        rich::link_addresses(e, a.max(0) as usize, end.max(0) as usize);
    } else {
        rich::paste_plain(e);
        let (end, _) = rich::sel(e);
        // Lines that came in starting with a box are checklist items: they get their look.
        if clip.as_deref().is_some_and(|c| c.contains(['\u{2610}', '\u{2611}'])) {
            rich::style_boxes_in(e, a, end);
        }
        rich::link_addresses(e, a.max(0) as usize, end.max(0) as usize);
    }
    // What came in ends where the caret is: in view, as typing keeps it. A paste is put in
    // with the view held still (`rich::still`), so lines pasted at the bottom of the note
    // (All Notes' card most of all, a few lines tall) went on out of sight below it.
    super::scrollbar::keep_caret_in_view(e);
    mark_dirty(h);
}

/// Where the text linked as `a..b` ends now that the link's hidden address sits before it.
fn link_end(e: HWND, a: i32, b: i32) -> i32 {
    let t = rich::text(e);
    rich::instructions(&t, a as usize, t.len()).first().filter(|l| l.0 == a as usize).map_or(b, |l| b + (l.1 - l.0) as i32)
}

pub(super) fn key_cmd(vk: u16, ctrl: bool, shift: bool) -> Option<u32> {
    let k = |c: char| c as u16;
    Some(match (ctrl, shift, vk) {
        (true, false, v) if v == k('B') => NC_BOLD,
        (true, false, v) if v == k('I') => NC_ITALIC,
        (true, false, v) if v == k('U') => NC_UNDERLINE,
        (true, true, v) if v == k('X') => NC_STRIKE,
        (true, true, v) if v == k('8') => NC_BULLETS,
        (true, true, v) if v == k('7') => NC_NUMBERS,
        (true, true, v) if v == k('L') => NC_TASK,
        (true, true, v) if v == k('A') => NC_ARCHIVE,
        (true, true, v) if v == k('P') => NC_PIN,
        (true, false, v) if v == k('W') => NC_CLOSE,
        (true, false, v) if v == k('K') => NC_LINK,
        (true, false, v) if v == k('L') => NC_ALIGN_LEFT,
        (true, false, v) if v == k('E') => NC_ALIGN_CENTER,
        (true, false, v) if v == k('R') => NC_ALIGN_RIGHT,
        (true, false, VK_OEM_7) => NC_QUOTE,
        (true, true, v) if v == k('M') => NC_CODE,
        (true, true, v) if v == k('T') => NC_TABLE,
        (true, false, v) if v == k('N') => NC_NEW,
        (true, false, v) if v == k('V') => NC_PASTE,
        (false, true, VK_INSERT) => NC_PASTE,
        (true, false, VK_RETURN) => NC_TOGGLE_CHECK,
        (true, false, VK_OEM_PERIOD) => NC_CYCLE_COLOR,
        (true, true, VK_BACK) | (true, true, VK_DELETE) => NC_DELETE,
        (true, false, VK_OEM_6) => NC_NEXT,
        (true, false, VK_OEM_4) => NC_PREV,
        (false, false, VK_ESCAPE) => NC_CLOSE,
        _ => return None,
    })
}
