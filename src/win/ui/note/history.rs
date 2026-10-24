// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Each note's history beyond its editor's own undo: the versions it was saved as, so a
//! note closed and opened again, left in All Notes for another one, or changed by an
//! assistant can still go back. RichEdit's undo covers what was done since the editor
//! opened; when it runs out, Ctrl+Z steps back through these, and redo forward again.
//! Kept in memory for the session, up to `STEPS` versions per note.

use std::cell::RefCell;
use std::collections::HashMap;

/// Versions kept per note.
pub const STEPS: usize = 20;
/// And at most this much of them (a note's pictures are in its RTF).
const MAX_BYTES: usize = 2 << 20;

#[derive(Default)]
struct Versions {
    /// Older versions (RTF), oldest first.
    back: Vec<String>,
    /// Versions stepped back from, for redo; the next one last.
    forward: Vec<String>,
}

thread_local! {
    static NOTES: RefCell<HashMap<u64, Versions>> = RefCell::new(HashMap::new());
}

fn with<R>(id: u64, f: impl FnOnce(&mut Versions) -> R) -> R {
    NOTES.with(|n| f(n.borrow_mut().entry(id).or_default()))
}

/// Note `id` is being saved with new content; `before` is what it was saved as last.
pub fn saved(id: u64, before: &str) {
    with(id, |v| {
        if v.back.last().map(String::as_str) == Some(before) {
            return;
        }
        v.back.push(before.to_string());
        v.forward.clear();
        while v.back.len() > STEPS || (v.back.len() > 1 && v.back.iter().map(String::len).sum::<usize>() > MAX_BYTES) {
            v.back.remove(0);
        }
    });
}

/// Ctrl+Z with the editor's own undo spent: the version to go back to, if there is one;
/// `current` is kept for redo. When the note is as one of its versions (its editor undid
/// its way back to how it opened), it's the one before that; otherwise the latest.
pub fn back(id: u64, current: &str) -> Option<String> {
    with(id, |v| {
        if let Some(i) = v.back.iter().rposition(|b| b == current) {
            v.back.truncate(i);
        }
        let prev = v.back.pop()?;
        v.forward.push(current.to_string());
        Some(prev)
    })
}

/// Redo with the editor's own redo spent: the version stepped back from last, if any;
/// `current` goes back on the undo side.
pub fn forward(id: u64, current: &str) -> Option<String> {
    with(id, |v| {
        let next = v.forward.pop()?;
        v.back.push(current.to_string());
        Some(next)
    })
}

/// Note `id` is gone for good (deleted): forget its versions.
pub fn forget(id: u64) {
    NOTES.with(|n| n.borrow_mut().remove(&id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_back_and_forward_across_sessions() {
        let id = 9_000_001;
        // Saved as "b" then "c": the versions before are "a" and "b".
        saved(id, "a");
        saved(id, "b");
        // Opened again as "c" (its own undo is empty): back to "b", then "a", then none.
        assert_eq!(back(id, "c").as_deref(), Some("b"));
        assert_eq!(back(id, "b").as_deref(), Some("a"));
        assert_eq!(back(id, "a"), None);
        // Redo: forward to "b", then "c".
        assert_eq!(forward(id, "a").as_deref(), Some("b"));
        assert_eq!(forward(id, "b").as_deref(), Some("c"));
        assert_eq!(forward(id, "c"), None);
        // A new edit ends the redo.
        assert_eq!(back(id, "c").as_deref(), Some("b"));
        saved(id, "b");
        assert_eq!(forward(id, "b2"), None);
        forget(id);
    }

    #[test]
    fn versions_the_editor_already_undid_are_passed_over() {
        let id = 9_000_002;
        saved(id, "old"); // from an earlier session
        // Opened as "x", edited and saved twice, then undone in the editor back to "x".
        saved(id, "x");
        saved(id, "y");
        assert_eq!(back(id, "x").as_deref(), Some("old"));
        forget(id);
    }

    #[test]
    fn at_most_twenty() {
        let id = 9_000_003;
        for k in 0..30 {
            saved(id, &format!("v{k}"));
        }
        let mut n = 0;
        let mut cur = "now".to_string();
        while let Some(prev) = back(id, &cur) {
            cur = prev;
            n += 1;
        }
        assert_eq!(n, STEPS);
        assert_eq!(cur, "v10");
        forget(id);
    }
}
