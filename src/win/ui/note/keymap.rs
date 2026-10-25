// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Ctrl+' (quote) and Ctrl+[ / Ctrl+] (previous / next note) by the character the key
//! makes on the keyboard layout in use, not by where it sits: on a German or French
//! keyboard those characters are on other keys, behind Shift or AltGr, which the US
//! layout's virtual-key codes would never match.

use super::*;

/// The command a chord's character stands for.
pub(super) fn chord_command(ch: char) -> Option<u32> {
    match ch {
        '\'' => Some(NC_QUOTE),
        '[' => Some(NC_PREV),
        ']' => Some(NC_NEXT),
        _ => None,
    }
}

/// The keyboard state to read the key's character with: Ctrl taken out (held, it turns
/// the key into a control character: Ctrl+[ is Esc), unless it is AltGr's half, which
/// some layouts need for the brackets (AltGr+8 is [ on a German keyboard).
pub(super) fn chord_state(mut keys: [u8; 256], altgr: bool) -> [u8; 256] {
    if !altgr {
        for vk in [VK_CONTROL, VK_LCONTROL, VK_RCONTROL] {
            keys[vk as usize] = 0;
        }
    }
    keys
}

/// Is `h` one of our note windows?
fn is_note(h: HWND) -> bool {
    if h.is_null() {
        return false;
    }
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
    n > 0 && crate::win::util::from_wide(&buf[..n as usize]) == CLASS
}

/// A Ctrl chord in a note's editor whose key makes ' [ or ] on this layout: runs its
/// command (posted, as the menus do) and takes the key. False lets the key through.
pub fn pre_translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN || !key_down(VK_CONTROL) {
        return false;
    }
    let h = unsafe { GetParent(msg.hwnd) };
    if !is_note(h) || state(h).is_none_or(|st| st.edit != msg.hwnd) {
        return false;
    }
    let mut keys = [0u8; 256];
    unsafe { GetKeyboardState(keys.as_mut_ptr()) };
    let altgr = keys[VK_RMENU as usize] & 0x80 != 0;
    let keys = chord_state(keys, altgr);
    let mut out = [0u16; 4];
    let scan = ((msg.lParam >> 16) & 0xFF) as u32;
    // Flag 4: don't touch the keyboard's dead-key state, which TranslateMessage owns.
    let n = unsafe { ToUnicode(msg.wParam as u32, scan, keys.as_ptr(), out.as_mut_ptr(), out.len() as i32, 4) };
    if n != 1 {
        return false;
    }
    let Some(cmd) = char::from_u32(out[0] as u32).and_then(chord_command) else { return false };
    unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_characters() {
        assert_eq!(chord_command('\''), Some(NC_QUOTE));
        assert_eq!(chord_command('['), Some(NC_PREV));
        assert_eq!(chord_command(']'), Some(NC_NEXT));
        for c in ['a', '4', '8', '#', '\u{1b}', 'ß'] {
            assert_eq!(chord_command(c), None, "{c:?}");
        }
    }

    #[test]
    fn control_is_taken_out_unless_altgr() {
        let mut keys = [0u8; 256];
        for vk in [VK_CONTROL, VK_LCONTROL, VK_SHIFT, VK_LSHIFT] {
            keys[vk as usize] = 0x80;
        }
        // Ctrl+Shift+#: Shift stays, so the key reads as the shifted character.
        let plain = chord_state(keys, false);
        assert_eq!(plain[VK_CONTROL as usize], 0);
        assert_eq!(plain[VK_LCONTROL as usize], 0);
        assert_eq!(plain[VK_SHIFT as usize], 0x80);
        assert_eq!(plain[VK_LSHIFT as usize], 0x80);
        // AltGr+8: Windows holds AltGr as Right Alt + Left Ctrl; both must stay for the
        // layout to give the third-level character.
        keys[VK_RMENU as usize] = 0x80;
        keys[VK_MENU as usize] = 0x80;
        let altgr = chord_state(keys, true);
        assert_eq!(altgr, keys);
    }
}
