// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Note commands (menus, toolbar, shortcuts) and the keyboard map.

use super::*;

/// Run a note command that needs the editor (formatting, clipboard) or forward to the app.
pub(super) fn run_command(h: HWND, cmd: u32) {
    let Some(st) = state(h) else { return };
    let (id, e) = (st.id, st.edit);
    let fmt = |f: &dyn Fn()| {
        f();
        mark_dirty(h);
    };
    match cmd {
        NC_BOLD => fmt(&|| rich::toggle_effect(e, CFM_BOLD)),
        NC_ITALIC => fmt(&|| rich::toggle_effect(e, CFM_ITALIC)),
        NC_UNDERLINE => fmt(&|| rich::toggle_effect(e, CFM_UNDERLINE)),
        NC_STRIKE => fmt(&|| rich::toggle_effect(e, CFM_STRIKEOUT)),
        NC_BULLETS => fmt(&|| rich::toggle_numbering(e, PFN_BULLET)),
        NC_NUMBERS => fmt(&|| rich::toggle_numbering(e, PFN_ARABIC)),
        NC_TASK => fmt(&|| toggle_task_lines(e)),
        NC_TOGGLE_CHECK => fmt(&|| toggle_check_at_caret(e)),
        NC_CUT => fmt(&|| {
            rich::send(e, WM_CUT, 0, 0);
        }),
        NC_COPY => {
            rich::send(e, WM_COPY, 0, 0);
        }
        NC_PASTE => paste(h),
        NC_SELECT_ALL => rich::set_sel(e, 0, -1),
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

/// Ctrl+K: link the selection (or edit the link at the caret). An address on the
/// clipboard is offered, ready for Enter.
fn link_command(h: HWND) {
    let Some(st) = state(h) else { return };
    let e = st.edit;
    let (a, b) = rich::sel(e);
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
        return;
    }
    if a != b
        && let Some(url) = crate::win::util::clipboard_text().map(|s| s.trim().to_string()).filter(|s| rich::looks_like_url(s))
        && !rich::text(e)[a as usize..b as usize].contains(&13)
    {
        rich::set_link(e, a, b, &url);
        let end = link_end(e, a, b);
        rich::set_sel(e, end, end);
    } else {
        rich::paste_plain(e);
        let (end, _) = rich::sel(e);
        rich::link_addresses(e, a.max(0) as usize, end.max(0) as usize);
    }
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
