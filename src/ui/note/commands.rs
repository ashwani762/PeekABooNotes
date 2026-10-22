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
        NC_PASTE => fmt(&|| rich::paste_plain(e)),
        NC_SELECT_ALL => rich::set_sel(e, 0, -1),
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
