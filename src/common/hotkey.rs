// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shortcuts that work anywhere, as keys: what each one is by default, what the user
//! set it to (kept in Settings as text, `"Ctrl+Alt+N"`), and whether a combination will do.
//! Whether the keys type a character, or another app has them, is the platform's to say.

use std::collections::BTreeMap;

/// Modifier bits, as Windows' `MOD_` flags have them.
pub const ALT: u8 = 1;
pub const CTRL: u8 = 2;
pub const SHIFT: u8 = 4;
pub const WIN: u8 = 8;

/// The shortcuts, in the order Settings lists them: the name Settings keeps each under,
/// and its letter with Ctrl+Alt by default.
pub const ACTIONS: [(&str, u8); 7] = [
    ("new_note", b'N'),
    ("all_notes", b'A'),
    ("archive", b'L'),
    ("reopen", b'O'),
    ("toggle_deck", b'H'),
    ("move_deck", b'E'),
    ("deck_keys", b'K'),
];
pub const COUNT: usize = ACTIONS.len();

// Each action's place in `ACTIONS`.
pub const NEW_NOTE: usize = 0;
pub const ALL_NOTES: usize = 1;
pub const ARCHIVE: usize = 2;
pub const REOPEN: usize = 3;
pub const TOGGLE_DECK: usize = 4;
pub const MOVE_DECK: usize = 5;
pub const DECK_KEYS: usize = 6;

/// F1 to F12 as key codes (Windows' virtual-key codes, as letters and digits are their
/// ASCII capitals and digits).
const F1: u8 = 0x70;
const F12: u8 = 0x7B;

const ALNUM: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const FKEYS: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];

/// A key combination: modifiers and one main key (a letter, a digit or F1 to F12).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    pub mods: u8,
    pub key: u8,
}

/// Why a combination won't do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Fewer than two of Ctrl, Alt, Shift and Win.
    Modifiers,
    /// Its main key isn't a letter, a digit or F1 to F12 (or there's none yet).
    Key,
    /// Another of the shortcuts (its place) has these keys.
    UsedBy(usize),
    /// These keys type a character on one of the user's keyboards (AltGr).
    Types,
    /// Another app has these keys.
    Taken,
}

/// Whether `key` can end a shortcut: a letter, a digit or F1 to F12.
pub fn main_key(key: u8) -> bool {
    key.is_ascii_uppercase() || key.is_ascii_digit() || (F1..=F12).contains(&key)
}

impl Combo {
    /// Shortcut `i`'s keys as they came: Ctrl+Alt and its letter.
    pub fn default_for(i: usize) -> Combo {
        Combo { mods: CTRL | ALT, key: ACTIONS[i].1 }
    }

    /// Modifiers `mods` held as `key` went down, if they make a shortcut: two of Ctrl,
    /// Alt, Shift and Win, and a letter, a digit or F1 to F12.
    pub fn from_press(mods: u8, key: u8) -> Result<Combo, Refusal> {
        if !main_key(key) {
            return Err(Refusal::Key);
        }
        let mods = mods & (ALT | CTRL | SHIFT | WIN);
        if mods.count_ones() < 2 {
            return Err(Refusal::Modifiers);
        }
        Ok(Combo { mods, key })
    }

    /// The main key's name: "N", "7", "F5".
    fn key_name(&self) -> &'static str {
        match self.key {
            k @ (b'A'..=b'Z') => &ALNUM[(k - b'A') as usize..][..1],
            k @ (b'0'..=b'9') => &ALNUM[26 + (k - b'0') as usize..][..1],
            k @ F1..=F12 => FKEYS[(k - F1) as usize],
            _ => "?",
        }
    }

    /// The keys in order, modifiers by their names in the strings ("ctrl"): Win, Ctrl,
    /// Alt, Shift, then the main key.
    pub fn parts(&self) -> Vec<&'static str> {
        let mut v = modifier_parts(self.mods);
        v.push(self.key_name());
        v
    }

    /// As menus and Settings show it, in this language's key names ("Strg+Alt+N").
    pub fn text(&self) -> String {
        crate::common::i18n::keys(&self.parts())
    }

    /// As Settings keeps it: "Ctrl+Alt+N", "Win+Shift+F5".
    pub fn spec(&self) -> String {
        let names: Vec<&str> = self
            .parts()
            .into_iter()
            .map(|p| match p {
                "win" => "Win",
                "ctrl" => "Ctrl",
                "alt" => "Alt",
                "shift" => "Shift",
                k => k,
            })
            .collect();
        names.join("+")
    }

    /// What `spec` wrote (modifiers in any order and case), if it's a shortcut that will do.
    pub fn parse(s: &str) -> Option<Combo> {
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let key = parts.pop()?;
        let mut mods = 0u8;
        for p in parts {
            let m = match p.to_ascii_lowercase().as_str() {
                "win" => WIN,
                "ctrl" => CTRL,
                "alt" => ALT,
                "shift" => SHIFT,
                _ => return None,
            };
            mods |= m;
        }
        let up = key.to_ascii_uppercase();
        let code = match up.as_bytes() {
            [c] => *c,
            [b'F', ..] => match up[1..].parse::<u8>() {
                Ok(n @ 1..=12) => F1 + n - 1,
                _ => return None,
            },
            _ => return None,
        };
        Combo::from_press(mods, code).ok()
    }
}

/// Modifiers `mods` by their names in the strings, in the order shortcuts show them:
/// Win, Ctrl, Alt, Shift.
pub fn modifier_parts(mods: u8) -> Vec<&'static str> {
    [(WIN, "win"), (CTRL, "ctrl"), (ALT, "alt"), (SHIFT, "shift")].iter().filter(|(m, _)| mods & m != 0).map(|&(_, n)| n).collect()
}

/// Which of the other shortcuts already has `c`, if one does (shortcut `i` itself aside).
pub fn used_by(keys: &[Option<Combo>; COUNT], i: usize, c: Combo) -> Option<usize> {
    (0..COUNT).find(|&j| j != i && keys[j] == Some(c))
}

/// Whether `mods` and `key`, pressed for shortcut `i`, will do as far as the shortcuts
/// themselves can say: a shortcut's form, and not one another shortcut has.
pub fn check(keys: &[Option<Combo>; COUNT], i: usize, mods: u8, key: u8) -> Result<Combo, Refusal> {
    let c = Combo::from_press(mods, key)?;
    match used_by(keys, i, c) {
        Some(j) => Err(Refusal::UsedBy(j)),
        None => Ok(c),
    }
}

/// Each shortcut's keys from what Settings keeps (`hotkeys`): the default where it keeps
/// nothing (or something this version can't read), none where it keeps "" (turned off).
pub fn resolve(kept: &BTreeMap<String, String>) -> [Option<Combo>; COUNT] {
    std::array::from_fn(|i| match kept.get(ACTIONS[i].0) {
        Some(s) if s.trim().is_empty() => None,
        Some(s) => Some(Combo::parse(s).unwrap_or(Combo::default_for(i))),
        None => Some(Combo::default_for(i)),
    })
}

/// Keep `keys` (none: off) for shortcut `i`; its default is kept as nothing, so the
/// settings of someone who never changed a shortcut stay empty.
pub fn set(kept: &mut BTreeMap<String, String>, i: usize, keys: Option<Combo>) {
    let name = ACTIONS[i].0.to_string();
    match keys {
        Some(c) if c == Combo::default_for(i) => {
            kept.remove(&name);
        }
        Some(c) => {
            kept.insert(name, c.spec());
        }
        None => {
            kept.insert(name, String::new());
        }
    }
}

/// Every shortcut back to its default. Names this version doesn't know (from a newer
/// one) are kept.
pub fn reset(kept: &mut BTreeMap<String, String>) {
    for (name, _) in ACTIONS {
        kept.remove(name);
    }
}

/// Whether every shortcut has its default keys.
pub fn all_default(kept: &BTreeMap<String, String>) -> bool {
    resolve(kept).iter().enumerate().all(|(i, k)| *k == Some(Combo::default_for(i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(mods: u8, key: u8) -> Combo {
        Combo { mods, key }
    }

    #[test]
    fn a_shortcut_takes_two_modifiers_and_a_letter_digit_or_f_key() {
        assert_eq!(Combo::from_press(CTRL | ALT, b'N'), Ok(c(CTRL | ALT, b'N')));
        assert_eq!(Combo::from_press(CTRL | SHIFT, b'7'), Ok(c(CTRL | SHIFT, b'7')));
        assert_eq!(Combo::from_press(ALT | SHIFT, 0x74), Ok(c(ALT | SHIFT, 0x74)), "Alt+Shift+F5");
        assert_eq!(Combo::from_press(WIN | SHIFT, 0x7B), Ok(c(WIN | SHIFT, 0x7B)), "Win+Shift+F12");
        assert_eq!(Combo::from_press(WIN | CTRL | ALT | SHIFT, b'Z'), Ok(c(WIN | CTRL | ALT | SHIFT, b'Z')));
        // One modifier, or none.
        assert_eq!(Combo::from_press(CTRL, b'N'), Err(Refusal::Modifiers));
        assert_eq!(Combo::from_press(SHIFT, 0x70), Err(Refusal::Modifiers));
        assert_eq!(Combo::from_press(0, b'N'), Err(Refusal::Modifiers));
        // A modifier alone, Esc, Tab, Enter, Space, F13, the number pad: not a main key.
        for key in [0x10, 0x11, 0x12, 0x5B, 0x1B, 0x09, 0x0D, 0x20, 0x7C, 0x60, 0x2E, 0x08] {
            assert_eq!(Combo::from_press(CTRL | ALT, key), Err(Refusal::Key), "key {key:#04X}");
        }
        // Anything but the four modifiers is let go of.
        assert_eq!(Combo::from_press(CTRL | ALT | 0x40, b'N'), Ok(c(CTRL | ALT, b'N')));
    }

    #[test]
    fn combinations_go_out_and_back_as_text() {
        for k in [c(CTRL | ALT, b'N'), c(WIN | SHIFT, 0x74), c(CTRL | ALT | SHIFT, b'0'), c(WIN | CTRL | ALT | SHIFT, 0x7B)] {
            assert_eq!(Combo::parse(&k.spec()), Some(k), "{}", k.spec());
        }
        assert_eq!(c(CTRL | ALT, b'N').spec(), "Ctrl+Alt+N");
        assert_eq!(c(SHIFT | WIN, 0x74).spec(), "Win+Shift+F5");
        assert_eq!(c(SHIFT | ALT | CTRL, b'9').spec(), "Ctrl+Alt+Shift+9");
        // Any order, any case, spaces around.
        assert_eq!(Combo::parse("shift + ctrl + f10"), Some(c(CTRL | SHIFT, 0x79)));
        assert_eq!(Combo::parse("ALT+CTRL+k"), Some(c(CTRL | ALT, b'K')));
        // Not a shortcut that will do.
        for bad in ["", "N", "Ctrl+N", "Ctrl+Alt", "Ctrl+Alt+", "Ctrl+Alt+F13", "Ctrl+Alt+F0", "Ctrl+Alt+Esc", "Ctrl+Meta+N", "Ctrl+Alt+NN", "Ctrl+Alt+€"] {
            assert_eq!(Combo::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn shown_with_win_shift_and_f_keys_in_this_language() {
        crate::common::i18n::init("en");
        assert_eq!(c(WIN | SHIFT, 0x74).text(), "Win+Shift+F5");
        assert_eq!(c(CTRL | ALT | SHIFT, 0x7B).text(), "Ctrl+Alt+Shift+F12");
        assert_eq!(c(CTRL | ALT, b'N').text(), "Ctrl+Alt+N");
        assert_eq!(c(WIN | CTRL, b'3').parts(), ["win", "ctrl", "3"]);
        crate::common::i18n::init("de");
        assert_eq!(c(CTRL | SHIFT, 0x70).text(), "Strg+Umschalt+F1");
        crate::common::i18n::init("en");
    }

    #[test]
    fn a_shortcut_another_one_has_is_refused_by_name() {
        let mut keys: [Option<Combo>; COUNT] = std::array::from_fn(|i| Some(Combo::default_for(i)));
        // New Note onto All Notes' keys.
        assert_eq!(check(&keys, NEW_NOTE, CTRL | ALT, b'A'), Err(Refusal::UsedBy(ALL_NOTES)));
        // Its own keys again are no clash.
        assert_eq!(check(&keys, NEW_NOTE, CTRL | ALT, b'N'), Ok(c(CTRL | ALT, b'N')));
        assert_eq!(check(&keys, NEW_NOTE, CTRL | ALT | SHIFT, b'A'), Ok(c(CTRL | ALT | SHIFT, b'A')));
        // A shortcut turned off holds no keys.
        keys[ALL_NOTES] = None;
        assert_eq!(check(&keys, NEW_NOTE, CTRL | ALT, b'A'), Ok(c(CTRL | ALT, b'A')));
        assert_eq!(used_by(&keys, DECK_KEYS, c(CTRL | ALT, b'H')), Some(TOGGLE_DECK));
        // The form is checked first.
        assert_eq!(check(&keys, NEW_NOTE, CTRL, b'A'), Err(Refusal::Modifiers));
    }

    #[test]
    fn kept_keys_resolve_against_the_defaults() {
        let mut kept = BTreeMap::new();
        assert!(all_default(&kept));
        let keys = resolve(&kept);
        assert_eq!(keys[NEW_NOTE], Some(c(CTRL | ALT, b'N')));
        assert_eq!(keys[DECK_KEYS], Some(c(CTRL | ALT, b'K')));
        set(&mut kept, ARCHIVE, Some(c(WIN | SHIFT, b'L')));
        set(&mut kept, REOPEN, None);
        assert_eq!(kept.get("archive").map(String::as_str), Some("Win+Shift+L"));
        assert_eq!(kept.get("reopen").map(String::as_str), Some(""));
        let keys = resolve(&kept);
        assert_eq!(keys[ARCHIVE], Some(c(WIN | SHIFT, b'L')));
        assert_eq!(keys[REOPEN], None);
        assert!(!all_default(&kept));
        // Set back to its default, a shortcut is kept as nothing.
        set(&mut kept, ARCHIVE, Some(Combo::default_for(ARCHIVE)));
        assert!(!kept.contains_key("archive"));
        // What can't be read is the default; a newer version's names stay through a reset.
        kept.insert("move_deck".into(), "Ctrl+Hyper+E".into());
        kept.insert("future_action".into(), "Ctrl+Alt+Q".into());
        assert_eq!(resolve(&kept)[MOVE_DECK], Some(Combo::default_for(MOVE_DECK)));
        reset(&mut kept);
        assert!(all_default(&kept));
        assert_eq!(kept.len(), 1);
        assert!(kept.contains_key("future_action"));
    }

    #[test]
    fn the_defaults_are_ctrl_alt_and_a_letter_each() {
        let mut letters: Vec<u8> = ACTIONS.iter().map(|&(_, k)| k).collect();
        assert_eq!(letters, b"NALOHEK");
        letters.sort_unstable();
        letters.dedup();
        assert_eq!(letters.len(), COUNT);
        let names: std::collections::HashSet<&str> = ACTIONS.iter().map(|&(n, _)| n).collect();
        assert_eq!(names.len(), COUNT);
    }
}
