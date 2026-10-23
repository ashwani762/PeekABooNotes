// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The portable half of the app: notes and their storage, the interface strings, the
//! design tokens, springs and the log. Nothing here calls an operating system directly;
//! what it needs from one (the local clock, the Documents folder, languages, locale
//! formats) comes through `crate::os`, which each platform implements.

#[macro_use]
pub mod i18n; // first, so t!/tn! are in scope for everything after it

pub mod anim;
pub mod attach;
pub mod emoji;
pub mod log;
pub mod mcp;
pub mod search;
pub mod store;
pub mod theme;
pub mod util;

#[cfg(test)]
mod tests {
    /// The portable half stays portable: no Windows APIs or Windows modules, only
    /// `crate::os` (which each platform implements).
    #[test]
    fn common_is_portable() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/common");
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap();
                    // Spelt in pieces so this test doesn't find them in itself.
                    for bad in [concat!("windows", "_sys"), concat!("windows", "::"), concat!("crate::", "win"), concat!("un", "safe")] {
                        assert!(!text.contains(bad), "{} uses {bad}", p.display());
                    }
                }
            }
        }
    }
}
