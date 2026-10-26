// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shortcuts that work anywhere: Ctrl+Alt and a letter, or the keys the user chose for
//! them in Settings (`common::hotkey`, kept in `Settings::hotkeys`).
//!
//! Ctrl+Alt is also AltGr, and on many keyboards AltGr and a letter types a character: €
//! with E on German, French and Spanish ones, ą ę ł ń ó with A, E, L, N and O on Polish
//! ones. A hotkey would swallow that character in every app, so a shortcut whose keys type
//! one that's written every day is left unregistered: any symbol (€), and letters on a
//! layout for another language than English. An English layout's extra accented letters
//! (English (India) types ā ē ō ṇ ḥ with AltGr) are rare in English text, so there the
//! shortcut wins. The layouts are looked at again when they change. A shortcut another app
//! already has can't be registered either. Both are logged, and Settings shows them as off.
//! Keys the user chooses go through the same test (with Shift, when they hold it), and are
//! refused there and then when they'd type a character or another app has them.
//!
//! Stepping aside for other languages' keyboards is a beta: only English layouts (US,
//! English (India)) have been tried on a real machine.

use std::cell::RefCell;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, GetKeyboardLayoutList, HKL, MAPVK_VK_TO_VSC, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, MapVirtualKeyExW, RegisterHotKey,
    ToUnicodeEx, UnregisterHotKey, VK_CONTROL, VK_LCONTROL, VK_LSHIFT, VK_MENU, VK_RMENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::*;
use crate::common::hotkey::{self, COUNT, Combo, Refusal};

/// Each shortcut's command, in `hotkey::ACTIONS` order. The deck's keys are on K rather than
/// D by default: Ctrl+Alt+D is Magnifier's docked view, likely in use by the very people
/// who want the deck by keyboard.
pub(super) const HOTKEYS: [u32; COUNT] = [CMD_NEW, CMD_ALL, CMD_ARCHIVE, CMD_REOPEN, CMD_TOGGLE_DECK, CMD_SWAP_EDGE, CMD_DECK_KEYS];

// `hotkey`'s modifier bits are Windows' own.
const _: () = assert!(MOD_ALT == hotkey::ALT as u32 && MOD_CONTROL == hotkey::CTRL as u32 && MOD_SHIFT == hotkey::SHIFT as u32 && MOD_WIN == hotkey::WIN as u32);

/// The id a combination is tried with before it's kept, apart from every shortcut's own.
const PROBE_ID: i32 = 0xBF00;

/// Why a shortcut is off (other than being turned off by the user).
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
    /// Each shortcut's keys as last registered (or tried), in `HOTKEYS` order.
    keys: [Option<Combo>; COUNT],
    /// Registered, or why not.
    on: [bool; COUNT],
    off: [Option<Off>; COUNT],
    /// Settings is waiting for a shortcut's new keys: none of ours is registered meanwhile,
    /// so the keys pressed reach it, ours too.
    paused: bool,
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

/// Whether keys `c` can type anything at all: only with Ctrl and Alt both (AltGr), and
/// not with Win, which types nothing.
fn may_type(c: Combo) -> bool {
    c.mods & (hotkey::CTRL | hotkey::ALT) == hotkey::CTRL | hotkey::ALT && c.mods & hotkey::WIN == 0
}

/// Whether keys `c` (AltGr, Shift too if it's held, and the main key) type a character on
/// keyboard layout `hkl`.
fn types_char(hkl: HKL, c: Combo) -> bool {
    if !may_type(c) {
        return false;
    }
    let mut keys = [0u8; 256];
    for k in [VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU] {
        keys[k as usize] = 0x80;
    }
    if c.mods & hotkey::SHIFT != 0 {
        for k in [VK_SHIFT, VK_LSHIFT] {
            keys[k as usize] = 0x80;
        }
    }
    let mut buf = [0u16; 8];
    unsafe {
        let scan = MapVirtualKeyExW(c.key as u32, MAPVK_VK_TO_VSC, hkl);
        // Flag 4: leave the keyboard's state (a dead key pending) as it is.
        let n = ToUnicodeEx(c.key as u32, scan, keys.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 4, hkl);
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

/// Each shortcut's keys as Settings has them (none: turned off).
fn wanted() -> [Option<Combo>; COUNT] {
    store(|s| hotkey::resolve(&s.d.settings.hotkeys))
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

/// Register the shortcuts with the keys Settings has for them, unless they'd get in the way
/// of typing, and let go of those that now would or whose keys changed (at start, when the
/// keyboard layouts change, and when the user changes a shortcut).
pub(super) fn register(main: HWND) {
    if STATE.with(|st| st.borrow().paused) {
        return;
    }
    let want = wanted();
    let list = layouts();
    let changed = STATE.with(|st| {
        let mut st = st.borrow_mut();
        // At start, the keys the user chose are no news.
        let first = st.layouts.is_empty();
        st.layouts = list.clone();
        let mut changed = false;
        for (i, id) in HOTKEYS.iter().map(|&id| id as i32).enumerate() {
            let was = st.off[i];
            if st.keys[i] != want[i] {
                // New keys: the old ones go first.
                if st.on[i] {
                    unsafe { UnregisterHotKey(main, id) };
                    st.on[i] = false;
                }
                match want[i] {
                    _ if first => {}
                    Some(c) => log::info(&format!("Shortcut {} is now {}", hotkey::ACTIONS[i].0, c.spec())),
                    None => log::info(&format!("Shortcut {} is turned off", hotkey::ACTIONS[i].0)),
                }
                st.keys[i] = want[i];
                changed = true;
            }
            let Some(c) = want[i] else {
                st.off[i] = None;
                continue;
            };
            let types = list.iter().find(|&&h| types_char(h as HKL, c));
            match plan(types.is_some(), st.on[i]) {
                Some(false) => {
                    unsafe { UnregisterHotKey(main, id) };
                    st.on[i] = false;
                }
                Some(true) => {
                    st.on[i] = unsafe { RegisterHotKey(main, id, c.mods as u32 | MOD_NOREPEAT, c.key as u32) } != 0;
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
                let k = c.spec();
                match (st.off[i], types) {
                    (Some(Off::Types), Some(h)) => log::info(&format!("{k} left out: it types a character on keyboard layout {:08X}", *h as usize & 0xFFFF_FFFF)),
                    (Some(Off::Taken), _) => log::warn(&format!("{k} couldn't be registered: another app has it")),
                    _ if was.is_some() => log::info(&format!("{k} is on again")),
                    _ => {}
                }
            }
        }
        changed
    });
    if changed {
        settings::refresh();
        // The empty list names the New Note shortcut.
        let h = allnotes::hwnd();
        if !h.is_null() {
            unsafe { InvalidateRect(h, std::ptr::null(), 0) };
        }
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
    let Some(i) = HOTKEYS.iter().position(|&id| id == cmd) else { return true };
    let Some(c) = STATE.with(|st| st.borrow().keys[i]) else { return false };
    let front = unsafe {
        let fg = GetForegroundWindow();
        GetKeyboardLayout(if fg.is_null() { 0 } else { GetWindowThreadProcessId(fg, std::ptr::null_mut()) })
    };
    let known = STATE.with(|st| st.borrow().layouts.contains(&(front as isize)));
    if known && !types_char(front, c) {
        return true;
    }
    register(main);
    STATE.with(|st| st.borrow().off[i].is_none())
}

/// Let go of every shortcut while Settings waits for new keys, so whatever is pressed
/// reaches it (until `resume_hotkeys`).
pub fn pause_hotkeys() {
    let main = ui(|u| u.main);
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        st.paused = true;
        for (i, id) in HOTKEYS.iter().enumerate() {
            if st.on[i] {
                unsafe { UnregisterHotKey(main, *id as i32) };
                st.on[i] = false;
            }
        }
    });
}

/// Settings has its keys: every shortcut is registered again, with the keys it has now.
pub fn resume_hotkeys() {
    STATE.with(|st| st.borrow_mut().paused = false);
    register(ui(|u| u.main));
}

/// Give shortcut `i` the keys `mods` (`hotkey::CTRL` and so on) and `key` were pressed as,
/// if they will do: a shortcut's form, not another shortcut's keys, not keys that type a
/// character, and not keys another app has. Kept in Settings, and registered unless
/// Settings is still waiting (then `resume_hotkeys` does it).
pub fn set_hotkey(i: usize, mods: u8, key: u8) -> Result<(), Refusal> {
    let keys = wanted();
    let c = hotkey::check(&keys, i, mods, key)?;
    if keys[i] == Some(c) {
        return Ok(());
    }
    if let Some(&h) = layouts().iter().find(|&&h| types_char(h as HKL, c)) {
        log::info(&format!("{} refused: it types a character on keyboard layout {:08X}", c.spec(), h as usize & 0xFFFF_FFFF));
        return Err(Refusal::Types);
    }
    let main = ui(|u| u.main);
    if unsafe { RegisterHotKey(main, PROBE_ID, c.mods as u32 | MOD_NOREPEAT, c.key as u32) } == 0 {
        log::info(&format!("{} refused: another app has it", c.spec()));
        return Err(Refusal::Taken);
    }
    unsafe { UnregisterHotKey(main, PROBE_ID) };
    update_settings(|s| hotkey::set(&mut s.hotkeys, i, Some(c)));
    register(main);
    Ok(())
}

/// Turn shortcut `i` off.
pub fn clear_hotkey(i: usize) {
    update_settings(|s| hotkey::set(&mut s.hotkeys, i, None));
    register(ui(|u| u.main));
}

/// Every shortcut back to Ctrl+Alt and its letter.
pub fn reset_hotkeys() {
    update_settings(|s| hotkey::reset(&mut s.hotkeys));
    register(ui(|u| u.main));
}

/// Whether every shortcut has its default keys.
pub fn hotkeys_default() -> bool {
    store(|s| hotkey::all_default(&s.d.settings.hotkeys))
}

/// Shortcut `i`'s keys (`hotkey::NEW_NOTE` and so on), or none when it's turned off.
pub fn hotkey_keys(i: usize) -> Option<Combo> {
    wanted()[i]
}

/// Why shortcut `i` is off, if it is for a reason of the computer's (not turned off).
pub fn hotkey_off(i: usize) -> Option<Off> {
    STATE.with(|st| st.borrow().off[i])
}

/// Shortcut `i` as menus and tooltips show it, or nothing while it's off.
pub fn hotkey_text(i: usize) -> String {
    match hotkey_keys(i) {
        Some(c) if hotkey_off(i).is_none() => c.text(),
        _ => String::new(),
    }
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
    fn only_altgr_can_type() {
        use hotkey::{ALT, CTRL, SHIFT, WIN};
        let c = |mods, key| Combo { mods, key };
        assert!(may_type(c(CTRL | ALT, b'E')));
        assert!(may_type(c(CTRL | ALT | SHIFT, b'E')), "AltGr+Shift types capitals and more");
        assert!(!may_type(c(CTRL | SHIFT, b'E')));
        assert!(!may_type(c(ALT | SHIFT, b'E')));
        assert!(!may_type(c(WIN | CTRL | ALT, b'E')));
    }

    #[test]
    fn every_shortcut_has_a_command_of_its_own() {
        let mut cmds = HOTKEYS.to_vec();
        cmds.sort_unstable();
        cmds.dedup();
        assert_eq!(cmds.len(), HOTKEYS.len());
        assert!(!HOTKEYS.contains(&(PROBE_ID as u32)));
    }
}
