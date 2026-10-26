// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shortcuts that work anywhere: Ctrl+Alt and a letter.
//!
//! Ctrl+Alt is also AltGr, and on many keyboards AltGr and a letter types a character: €
//! with E on German, French and Spanish ones, ą ę ł ń ó with A, E, L, N and O on Polish
//! ones. A hotkey would swallow that character in every app, so a shortcut whose keys type
//! one that's written every day is left unregistered: any symbol (€), and letters on a
//! layout for another language than English. An English layout's extra accented letters
//! (English (India) types ā ē ō ṇ ḥ with AltGr) are rare in English text, so there the
//! shortcut wins. The layouts are looked at again when they change. A shortcut another app
//! already has can't be registered either. Both are logged, and Settings shows them as off.
//!
//! Stepping aside for other languages' keyboards is a beta: only English layouts (US,
//! English (India)) have been tried on a real machine.

use std::cell::RefCell;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, HKL, MAPVK_VK_TO_VSC, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MapVirtualKeyExW, RegisterHotKey, ToUnicodeEx,
    UnregisterHotKey, VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::*;

/// Ctrl+Alt and a letter. The deck's keys are on K rather than D: Ctrl+Alt+D is
/// Magnifier's docked view, likely in use by the very people who want the deck by keyboard.
pub(super) const HOTKEYS: [(u32, u8); 7] = [
    (CMD_NEW, b'N'),
    (CMD_ALL, b'A'),
    (CMD_ARCHIVE, b'L'),
    (CMD_REOPEN, b'O'),
    (CMD_TOGGLE_DECK, b'H'),
    (CMD_SWAP_EDGE, b'E'),
    (CMD_DECK_KEYS, b'K'),
];

/// Why a shortcut is off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Off {
    /// Its keys type a character on one of the user's keyboard layouts (AltGr).
    Types,
    /// Another app has it.
    Taken,
}

#[derive(Default)]
struct State {
    /// The keyboard layouts the shortcuts were last checked against.
    layouts: Vec<isize>,
    /// Each shortcut's state, in `HOTKEYS` order: registered, or why not.
    on: [bool; HOTKEYS.len()],
    off: [Option<Off>; HOTKEYS.len()],
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Whether ToUnicodeEx's answer (`n` and what it wrote into `buf`) is a character typed:
/// a dead key (negative) counts, since it types the next one; control codes don't.
fn typed(n: i32, buf: &[u16]) -> bool {
    n < 0 || buf.iter().take(n.max(0) as usize).any(|&c| c >= 0x20 && c != 0x7F)
}

/// Whether what was typed is only letters (with any combining marks on them), as opposed
/// to a symbol such as €.
fn only_letters(n: i32, buf: &[u16]) -> bool {
    n > 0
        && char::decode_utf16(buf.iter().take(n as usize).copied())
            .all(|c| c.is_ok_and(|c| c.is_alphabetic() || ('\u{300}'..='\u{36F}').contains(&c)))
}

/// Whether a character typed with AltGr takes the shortcut's place: a symbol always does;
/// a letter does unless the layout is for English (`english`), whose text rarely needs it.
fn takes_place(n: i32, buf: &[u16], english: bool) -> bool {
    typed(n, buf) && !(english && only_letters(n, buf))
}

/// Whether Ctrl+Alt (AltGr) and key `vk` types a character on keyboard layout `hkl`.
fn types_char(hkl: HKL, vk: u8) -> bool {
    let mut keys = [0u8; 256];
    for k in [VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU] {
        keys[k as usize] = 0x80;
    }
    let mut buf = [0u16; 8];
    unsafe {
        let scan = MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_VSC, hkl);
        // Flag 4: leave the keyboard's state (a dead key pending) as it is.
        let n = ToUnicodeEx(vk as u32, scan, keys.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 4, hkl);
        // The layout's language is the low word's primary language (LANG_ENGLISH = 9).
        let english = (hkl as usize & 0x3FF) == 0x09;
        takes_place(n, &buf, english)
    }
}

/// The user's keyboard layouts.
fn layouts() -> Vec<isize> {
    unsafe {
        let n = GetKeyboardLayoutList(0, std::ptr::null_mut());
        let mut v: Vec<HKL> = vec![std::ptr::null_mut(); n.max(0) as usize];
        let got = GetKeyboardLayoutList(n, v.as_mut_ptr());
        v.truncate(got.max(0) as usize);
        let mut v: Vec<isize> = v.into_iter().map(|h| h as isize).collect();
        v.sort_unstable();
        v
    }
}

/// What to do with a shortcut: whether its keys type a character, and whether it's
/// registered now. Registering again (`Some(true)`) is only tried while it isn't.
fn plan(types: bool, registered: bool) -> Option<bool> {
    match (types, registered) {
        (true, true) => Some(false),
        (false, false) => Some(true),
        _ => None,
    }
}

/// Register the shortcuts that don't get in the way of typing, and let go of those that
/// now would (at start, and when the keyboard layouts change).
pub(super) fn register(main: HWND) {
    let list = layouts();
    let changed = STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.layouts = list.clone();
        let mut changed = false;
        for (i, (id, key)) in HOTKEYS.iter().copied().enumerate() {
            let types = list.iter().find(|&&h| types_char(h as HKL, key));
            let was = st.off[i];
            match plan(types.is_some(), st.on[i]) {
                Some(false) => {
                    unsafe { UnregisterHotKey(main, id as i32) };
                    st.on[i] = false;
                }
                Some(true) => {
                    st.on[i] = unsafe { RegisterHotKey(main, id as i32, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, key as u32) } != 0;
                }
                None => {}
            }
            st.off[i] = match types {
                Some(_) => Some(Off::Types),
                None if !st.on[i] => Some(Off::Taken),
                None => None,
            };
            if st.off[i] != was {
                changed = true;
                let k = key as char;
                match (st.off[i], types) {
                    (Some(Off::Types), Some(h)) => log::info(&format!("Ctrl+Alt+{k} left out: it types a character on keyboard layout {:08X}", *h as usize & 0xFFFF_FFFF)),
                    (Some(Off::Taken), _) => log::warn(&format!("Ctrl+Alt+{k} couldn't be registered: another app has it")),
                    _ if was.is_some() => log::info(&format!("Ctrl+Alt+{k} is on again")),
                    _ => {}
                }
            }
        }
        changed
    });
    if changed {
        settings::refresh();
    }
}

/// The keyboard layouts may have changed (one added or removed, or switched to): look
/// again if they did.
pub(super) fn recheck(main: HWND) {
    if STATE.with(|st| st.borrow().layouts != layouts()) {
        register(main);
    }
}

/// Hotkey `cmd` was pressed: whether to act on it. A layout added since the shortcuts were
/// last checked may type a character with these keys; then the shortcut steps aside from
/// now on, and this press (meant as that character) does nothing.
pub(super) fn pressed(main: HWND, cmd: u32) -> bool {
    let Some(i) = HOTKEYS.iter().position(|&(id, _)| id == cmd) else { return true };
    let front = unsafe {
        let fg = GetForegroundWindow();
        GetKeyboardLayout(if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) })
    };
    let known = STATE.with(|st| st.borrow().layouts.contains(&(front as isize)));
    if known && !types_char(front, HOTKEYS[i].1) {
        return true;
    }
    register(main);
    STATE.with(|st| st.borrow().off[i].is_none())
}

/// Why the shortcut Ctrl+Alt+`letter` is off, if it is.
pub fn hotkey_off(letter: u8) -> Option<Off> {
    let i = HOTKEYS.iter().position(|&(_, k)| k == letter)?;
    STATE.with(|st| st.borrow().off[i])
}

/// Ctrl+Alt+`letter` as menus and tooltips show it, or nothing while it's off.
pub fn hotkey_text(letter: u8) -> String {
    if hotkey_off(letter).is_some() {
        return String::new();
    }
    crate::common::i18n::keys(&["ctrl", "alt", &(letter as char).to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_count_control_codes_dont() {
        assert!(typed(1, &[0x20AC]), "AltGr+E: €");
        assert!(typed(1, &[0x0105]), "AltGr+A on a Polish keyboard: ą");
        assert!(typed(-1, &[0x00B4]), "a dead key types the next character");
        assert!(!typed(0, &[0; 8]), "nothing typed");
        assert!(!typed(1, &[0x0005]), "Ctrl and a letter: a control code");
        assert!(!typed(1, &[0x007F]));
        assert!(typed(2, &[0x0001, 0x00E9]), "anything printable in what was typed");
        assert!(!typed(1, &[0x0005, 0x00E9]), "only what was typed counts");
    }

    #[test]
    fn english_layouts_keep_their_shortcuts_over_extra_letters() {
        // English (India): AltGr+A types ā, AltGr+L types l with a ring below.
        assert!(!takes_place(1, &[0x0101], true));
        assert!(!takes_place(2, &[0x006C, 0x0325], true));
        // A symbol always wins, English layout or not.
        assert!(takes_place(1, &[0x20AC], true));
        assert!(takes_place(1, &[0x20AC], false));
        // Polish: ą is everyday writing.
        assert!(takes_place(1, &[0x0105], false));
        // A dead key types the next character, whatever it is.
        assert!(takes_place(-1, &[0x00B4], true));
        // Nothing typed, or a control code: no clash.
        assert!(!takes_place(0, &[0; 8], false));
        assert!(!takes_place(1, &[0x0005], false));
    }

    #[test]
    fn a_shortcut_steps_aside_and_comes_back() {
        assert_eq!(plan(true, true), Some(false), "it types now: let it go");
        assert_eq!(plan(true, false), None, "already off");
        assert_eq!(plan(false, false), Some(true), "free again, or taken last time: try again");
        assert_eq!(plan(false, true), None, "already on");
    }

    #[test]
    fn every_shortcut_is_a_letter_of_its_own() {
        let mut keys: Vec<u8> = HOTKEYS.iter().map(|&(_, k)| k).collect();
        assert!(keys.iter().all(u8::is_ascii_uppercase));
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), HOTKEYS.len());
    }
}
