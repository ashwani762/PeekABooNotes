//! The editor control's subclass: keys, clicks and the checklist overlay hooks.

use super::*;

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
            rich::paste_plain(e);
            return 0;
        }
        WM_LBUTTONDOWN => {
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
            crate::editor::checks::set_hot(e, hot);
            pointer_in(h);
        }
        WM_MOUSELEAVE => {
            crate::editor::checks::set_hot(e, None);
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
            crate::editor::checks::forget(e);
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
        crate::editor::checks::paint(e);
        super::scrollbar::sync(h);
    }
    r
}
