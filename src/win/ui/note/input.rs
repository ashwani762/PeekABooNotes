// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor control's subclass: keys, clicks and the checklist overlay hooks.

use super::*;

/// Over a link, a tooltip shows where it goes.
fn link_tip(e: HWND, x: i32, y: i32) {
    use crate::win::ui::tooltip;
    let lh = (rich::body_size() as f32 / 15.0 * 1.45) as i32;
    // EM_CHARFROMPOS gives the nearest caret position: the character under the pointer
    // is the one after it or the one before. None when off the end of a line.
    let cp = rich::char_from_pos(e, x, y);
    let over = |c: i32| {
        let (p, q) = (rich::pos_from_char(e, c), rich::pos_from_char(e, c + 1));
        let right = if q.y == p.y && q.x > p.x { q.x } else { p.x };
        c >= 0 && x >= p.x && x < right && y >= p.y && y < p.y + lh
    };
    let hit = [cp, cp - 1].into_iter().find(|&c| over(c));
    let Some((a, b, url)) = hit.and_then(|c| rich::link_at(e, c).filter(|l| c >= l.0 && c < l.1)) else {
        tooltip::leave(e);
        return;
    };
    let cp = hit.unwrap_or(cp);
    // Under the part of the link the pointer is on.
    let t = rich::text(e);
    let from = rich::name_start(&t, a, b).max(a);
    let p0 = rich::pos_from_char(e, from);
    let pe = rich::pos_from_char(e, b);
    let p = rich::pos_from_char(e, cp);
    let (l, r) = if p0.y == pe.y { (p0.x, pe.x) } else { (p.x - 20, p.x + 20) };
    let anchor = crate::win::util::rect(l, p.y, (r - l).max(1), lh);
    let shown = url.strip_prefix("https://").or(url.strip_prefix("http://")).unwrap_or(&url).trim_end_matches('/');
    let shown: String = if shown.chars().count() > 60 { shown.chars().take(59).chain(std::iter::once('\u{2026}')).collect() } else { shown.to_string() };
    tooltip::hover(e, anchor, &shown, "");
}

pub(super) unsafe extern "system" fn edit_proc(e: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, parent: usize) -> LRESULT {
    let h = parent as HWND;
    match msg {
        // One scroller for the whole note (see `scrollbar::wheel`), never RichEdit's own.
        WM_MOUSEWHEEL => {
            super::scrollbar::wheel(h, (wp >> 16) as i16);
            return 0;
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            super::scrollbar::interrupt(h);
            let vk = wp as u16;
            let (ctrl, shift, alt) = (key_down(VK_CONTROL), key_down(VK_SHIFT), key_down(VK_MENU));
            let Some(st) = state(h) else { return unsafe { DefSubclassProc(e, msg, wp, lp) } };
            st.swallow = false;
            if !matches!(vk, VK_BACK | VK_SHIFT | VK_CONTROL | VK_MENU) {
                st.auto = None;
            }
            if ctrl && !shift && !alt && vk == b'Z' as u16 {
                if undo_toggle(e) {
                    mark_dirty(h);
                    st.swallow = true;
                    return 0;
                }
            } else if !matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU) {
                LAST_TOGGLE.with(|l| l.set(None));
            }
            if !alt {
                if let Some(cmd) = key_cmd(vk, ctrl, shift) {
                    st.swallow = true;
                    unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
                    return 0;
                }
                if vk == VK_BACK && !ctrl && revert_auto(h, e) {
                    st.swallow = true;
                    return 0;
                }
                if vk == VK_TAB && !ctrl && rich::nest(e, if shift { -1 } else { 1 }) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                if vk == VK_RETURN && !ctrl && !shift && handle_enter(h, e) {
                    st.swallow = true;
                    return 0;
                }
            }
        }
        WM_CHAR => {
            let c = wp as u32;
            if let Some(st) = state(h)
                && st.swallow && (c < 0x20 || c == 0x7F) {
                    st.swallow = false;
                    return 0;
                }
            if c == b' ' as u32 && try_auto_list(h, e) {
                return 0;
            }
        }
        WM_PASTE => {
            paste(h);
            return 0;
        }
        // A quick second click on a circle arrives as a double-click: tick again, rather
        // than letting RichEdit select the word under it.
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if let Some(st) = state(h) {
                st.auto = None;
            }
            if !key_down(VK_SHIFT)
                && let Some(cp) = checkbox_at(e, x_of(lp), y_of(lp)) {
                    unsafe { SetFocus(e) };
                    toggle_check(e, cp);
                    mark_dirty(h);
                    return 0;
                }
        }
        WM_CONTEXTMENU => {
            let Some(id) = id_of(h) else { return 0 };
            let (mut x, mut y) = (x_of(lp), y_of(lp));
            if x == -1 && y == -1 {
                let (a, _) = rich::sel(e);
                let mut p = rich::pos_from_char(e, a);
                unsafe { ClientToScreen(e, &mut p) };
                (x, y) = (p.x, p.y + 20);
            }
            let cmd = app::popup(app::note_menu(id, true), x, y, h);
            if cmd != 0 {
                unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
            }
            return 0;
        }
        WM_MOUSEMOVE => {
            let hot = checkbox_at(e, x_of(lp), y_of(lp));
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: e, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut tme) };
            crate::win::editor::checks::set_hot(e, hot);
            pointer_in(h);
            link_tip(e, x_of(lp), y_of(lp));
        }
        WM_MOUSELEAVE => {
            crate::win::ui::tooltip::leave(e);
            crate::win::editor::checks::set_hot(e, None);
            pointer_out(h);
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            let mut p = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut p);
                ScreenToClient(e, &mut p);
            }
            if checkbox_at(e, p.x, p.y).is_some() {
                unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
                return 1;
            }
        }
        WM_NCDESTROY => unsafe {
            crate::win::editor::checks::forget(e);
            RemoveWindowSubclass(e, Some(edit_proc), 1);
        },
        _ => {}
    }
    let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
    // RichEdit draws outside WM_PAINT too (typing, selecting, scrolling): put the
    // checkboxes back over their placeholder glyphs after anything that may have drawn.
    let drew = matches!(
        msg,
        WM_PAINT
            | WM_CHAR
            | WM_KEYDOWN
            | WM_KEYUP
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_VSCROLL
            | WM_MOUSEWHEEL
            | WM_TIMER
            | WM_SIZE
            | WM_IME_COMPOSITION
            | WM_IME_ENDCOMPOSITION
            | WM_CUT
            | WM_UNDO
            | rich::EM_REDO
    ) || (msg == WM_MOUSEMOVE && wp & 0x0001 != 0);
    if drew {
        crate::win::editor::checks::paint(e);
        super::scrollbar::sync(h);
    }
    r
}
