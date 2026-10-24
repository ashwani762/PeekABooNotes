// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor control's subclass: keys, clicks and the checklist overlay hooks.

use super::*;

thread_local! {
    /// An attachment pressed on: (where, its file), until the pointer moves far enough
    /// to drag it out or comes back up to open it.
    static PRESS: std::cell::RefCell<Option<(POINT, std::path::PathBuf, String)>> = const { std::cell::RefCell::new(None) };
}

/// The link under the pointer: (start, end, address, the character pointed at). A
/// picture counts as its attachment's link.
fn link_under(e: HWND, x: i32, y: i32) -> Option<(i32, i32, String, i32)> {
    let lh = (rich::body_size() as f32 / 15.0 * 1.45) as i32;
    let t = rich::text(e);
    let cp = rich::char_from_pos(e, x, y);
    let over = |c: i32| {
        let (p, q) = (rich::pos_from_char(e, c), rich::pos_from_char(e, c + 1));
        let right = if q.y == p.y && q.x > p.x { q.x } else { p.x };
        // A line is as tall as what's on it (a picture): down to where the next starts.
        let next_line = t.get(c.max(0) as usize..).and_then(|r| r.iter().position(|&ch| ch == 13)).map(|i| c + i as i32 + 1);
        let bottom = next_line.filter(|&n| (n as usize) < t.len()).map_or(p.y + lh, |n| rich::pos_from_char(e, n).y);
        c >= 0 && x >= p.x && x < right && y >= p.y && y < bottom.max(p.y + lh)
    };
    let hit = [cp, cp - 1].into_iter().find(|&c| over(c))?;
    if let Some((a, b, url)) = rich::item_link(e, &t, hit as usize) {
        return Some((a, b, url, hit));
    }
    rich::link_at(e, hit).filter(|l| hit >= l.0 && hit < l.1).map(|(a, b, url)| (a, b, url, hit))
}

/// Over a link, a tooltip shows where it goes.
fn link_tip(e: HWND, x: i32, y: i32) {
    use crate::win::ui::tooltip;
    let lh = (rich::body_size() as f32 / 15.0 * 1.45) as i32;
    let Some((a, b, url, cp)) = link_under(e, x, y) else {
        tooltip::leave(e);
        return;
    };
    // Under the part of the link the pointer is on.
    let t = rich::text(e);
    let from = rich::name_start(&t, a, b).max(a);
    let p0 = rich::pos_from_char(e, from);
    let pe = rich::pos_from_char(e, b);
    let p = rich::pos_from_char(e, cp);
    let (l, r) = if p0.y == pe.y { (p0.x, pe.x) } else { (p.x - 20, p.x + 20) };
    let anchor = crate::win::util::rect(l, p.y, (r - l).max(1), lh);
    // An attachment: its name, and what dragging does.
    if let Some(name) = crate::common::attach::name_of(&url) {
        // Under what the line shows: a picture reaches down to where the next line starts.
        let anchor = if rich::item_link(e, &t, cp as usize).is_some() {
            let (s0, s1) = rich::item_shown(&t, cp as usize);
            let (q0, q1) = (rich::pos_from_char(e, s0 as i32), rich::pos_from_char(e, s1 as i32));
            let next = t[cp as usize..].iter().position(|&c| c == 13).map(|i| cp + i as i32 + 1).filter(|&n| (n as usize) < t.len());
            let bottom = next.map_or(q0.y + lh, |n| rich::pos_from_char(e, n).y).min(q0.y + 600);
            crate::win::util::rect(q0.x, q0.y, (q1.x - q0.x).max(1), (bottom - q0.y).max(lh))
        } else {
            anchor
        };
        tooltip::hover(e, anchor, &name, "");
        return;
    }
    let shown = url.strip_prefix("https://").or(url.strip_prefix("http://")).unwrap_or(&url).trim_end_matches('/');
    let shown: String = if shown.chars().count() > 60 { shown.chars().take(59).chain(std::iter::once('\u{2026}')).collect() } else { shown.to_string() };
    tooltip::hover(e, anchor, &shown, "");
}

/// The text menu (formatting and editing) at screen point `p`, or under the caret when
/// `p` is (-1, -1), as the keyboard asks for it.
fn text_menu(h: HWND, e: HWND, mut p: POINT) {
    if id_of(h).is_none() {
        return;
    }
    if p.x == -1 && p.y == -1 {
        let (a, _) = rich::sel(e);
        p = rich::pos_from_char(e, a);
        unsafe { ClientToScreen(e, &mut p) };
        p.y += 20;
    }
    let cmd = app::popup(app::edit_menu(e), p.x, p.y, h);
    if cmd != 0 {
        unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
    }
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
            // The keyboard's Menu key, or Shift+F10: the text menu at the caret.
            if vk == VK_APPS || (shift && !ctrl && vk == VK_F10) {
                st.swallow = true;
                text_menu(h, e, POINT { x: -1, y: -1 });
                return 0;
            }
            if !matches!(vk, VK_BACK | VK_SHIFT | VK_CONTROL | VK_MENU) {
                st.auto = None;
            }
            if ctrl && shift && !alt && vk == b'Z' as u16 {
                unsafe { DefSubclassProc(e, rich::EM_REDO, 0, 0) };
                st.swallow = true;
                return 0;
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
                if vk == VK_RETURN {
                    rich::link_word_before(e);
                }
                if vk == VK_RETURN && !ctrl && !shift && handle_enter(h, e) {
                    st.swallow = true;
                    return 0;
                }
            }
        }
        WM_CHAR => {
            let c = wp as u32;
            // An emoji's variation selector goes in as its hidden stand-in (see
            // rich::selectors): the real one would stop RichEdit drawing list bullets.
            if let Some(m) = rich::marker_for(c as u16) {
                let r = unsafe { DefSubclassProc(e, msg, m as usize, lp) };
                rich::hide_marks(e);
                return r;
            }
            if let Some(st) = state(h)
                && st.swallow && (c < 0x20 || c == 0x7F) {
                    st.swallow = false;
                    return 0;
                }
            if c == b' ' as u32 && try_auto_list(h, e) {
                return 0;
            }
            // A space or Tab ends a word: an address becomes a link.
            if c == b' ' as u32 || c == b'\t' as u32 {
                let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
                rich::link_word_before(e);
                return r;
            }
        }
        WM_PASTE => {
            paste(h);
            return 0;
        }
        // Clicking away ends the word being typed too.
        WM_KILLFOCUS => rich::link_word_before(e),
        // A quick second click on a circle arrives as a double-click: tick again, rather
        // than letting RichEdit select the word under it.
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if let Some(st) = state(h) {
                st.auto = None;
            }
            // An attachment: a double-click opens it; a press drags it out, or (let go
            // where it was) selects it.
            if msg == WM_LBUTTONDBLCLK
                && let Some((_, _, url, _)) = link_under(e, x_of(lp), y_of(lp))
                && super::attach::file_of(&url).is_some()
            {
                super::open_link(&url);
                return 0;
            }
            if msg == WM_LBUTTONDOWN
                && let Some((_, _, url, _)) = link_under(e, x_of(lp), y_of(lp))
                && let Some(path) = super::attach::file_of(&url)
            {
                PRESS.with(|p| *p.borrow_mut() = Some((POINT { x: x_of(lp), y: y_of(lp) }, path, url)));
                unsafe {
                    SetFocus(e);
                    SetCapture(e);
                }
                return 0;
            }
            if !key_down(VK_SHIFT)
                && let Some(cp) = checkbox_at(e, x_of(lp), y_of(lp)) {
                    unsafe { SetFocus(e) };
                    toggle_check(e, cp);
                    mark_dirty(h);
                    return 0;
                }
        }
        // RichEdit hands right-clicks to its OLE callback (see `attach`) rather than send
        // WM_CONTEXTMENU, so the text menu opens from here. A right-click outside the
        // selection moves the caret there first; inside it, the selection stays to act on.
        WM_RBUTTONDOWN => {
            let cp = rich::char_from_pos(e, x_of(lp), y_of(lp));
            let (a, b) = rich::sel(e);
            if a == b || cp < a.min(b) || cp > a.max(b) {
                rich::set_sel(e, cp, cp);
            }
            unsafe { SetFocus(e) };
            return 0;
        }
        WM_RBUTTONUP => {
            let mut p = POINT { x: x_of(lp), y: y_of(lp) };
            unsafe { ClientToScreen(e, &mut p) };
            text_menu(h, e, p);
            return 0;
        }
        WM_CONTEXTMENU => {
            text_menu(h, e, POINT { x: x_of(lp), y: y_of(lp) });
            return 0;
        }
        WM_MOUSEMOVE if PRESS.with(|p| p.borrow().is_some()) => {
            let far = PRESS.with(|p| {
                p.borrow().as_ref().is_some_and(|(at, _, _)| (x_of(lp) - at.x).abs() > 4 || (y_of(lp) - at.y).abs() > 4)
            });
            if far && let Some((_, path, url)) = PRESS.with(|p| p.borrow_mut().take()) {
                unsafe { ReleaseCapture() };
                crate::win::ui::tooltip::mute();
                super::attach::drag_out(e, &path, &url);
            }
            return 0;
        }
        WM_LBUTTONUP if PRESS.with(|p| p.borrow().is_some()) => {
            // Taken before letting go of the mouse: losing capture forgets the press.
            let press = PRESS.with(|p| p.borrow_mut().take());
            unsafe { ReleaseCapture() };
            if let Some((at, _, _)) = press
                && let Some((_, b, _, cp)) = link_under(e, at.x, at.y)
            {
                // Select it, as Explorer does with a file: it can be cut, copied or deleted.
                let (start, _) = rich::item_shown(&rich::text(e), cp as usize);
                rich::set_sel(e, start as i32, b);
            }
            return 0;
        }
        WM_CAPTURECHANGED => {
            PRESS.with(|p| p.borrow_mut().take());
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
            crate::win::editor::fade::forget(e);
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
        crate::win::editor::emoji::paint(e);
        if let Some((paper, s)) = crate::win::editor::checks::paper(e) {
            crate::win::editor::fade::paint(e, paper, s);
        }
        super::scrollbar::sync(h);
    }
    r
}
