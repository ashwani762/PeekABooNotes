// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! A note's earlier versions, for what Ctrl+Z can't reach: the editor's undo covers what
//! was done since the note was opened, and these go back further. One is kept when a note
//! is first changed after it was opened or after `IDLE` without changes (unless the last
//! one is only minutes old), and before an assistant changes it. Saving as you type
//! doesn't keep one. Kept as Markdown in `versions.json` beside the notes,
//! written only when a version is taken or dropped and read only then and when they're
//! listed, so nothing of them stays in memory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Versions kept per note, newest last.
const PER_NOTE: usize = 10;
/// None older than this (seconds).
const MAX_AGE: i64 = 30 * 24 * 3600;
/// Nor more than this of them altogether: the oldest go first.
const MAX_BYTES: usize = 5 << 20;
/// Editing a note again this soon (seconds) after its last version was taken doesn't
/// take another: a note closed and reopened to fix a typo isn't a new version.
const QUIET: i64 = 600;
/// A note left open this long without a change starts a new stretch of work when it's
/// next changed.
pub const IDLE: std::time::Duration = std::time::Duration::from_secs(QUIET as u64);

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Version {
    /// When the note was last saved as this.
    pub at: i64,
    /// When the version was taken.
    pub taken: i64,
    /// The note, as Markdown (pictures and files by their attachment links).
    pub md: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Book {
    #[serde(default)]
    notes: BTreeMap<u64, Vec<Version>>,
}

impl Book {
    /// Keep `v` as note `id`'s newest version, unless one already there is the same (a
    /// version put back and then edited again would otherwise be listed twice), or
    /// (`quiet`) the newest was taken less than `QUIET` ago. True when kept.
    fn add(&mut self, id: u64, v: Version, now: i64, quiet: bool) -> bool {
        if v.md.trim().is_empty() {
            return false;
        }
        let list = self.notes.entry(id).or_default();
        if list.iter().any(|x| x.md == v.md) || list.last().is_some_and(|last| quiet && now - last.taken < QUIET) {
            return false;
        }
        list.push(v);
        self.prune(now);
        true
    }

    /// Drop what's too old, too many, and (oldest first, across notes) too much.
    fn prune(&mut self, now: i64) {
        for list in self.notes.values_mut() {
            list.retain(|v| now - v.taken < MAX_AGE);
            let extra = list.len().saturating_sub(PER_NOTE);
            list.drain(..extra);
        }
        let mut total: usize = self.notes.values().flatten().map(|v| v.md.len()).sum();
        while total > MAX_BYTES {
            let Some((&id, _)) = self.notes.iter().filter(|(_, l)| !l.is_empty()).min_by_key(|(_, l)| l[0].taken) else { break };
            let gone = self.notes.get_mut(&id).map(|l| l.remove(0));
            total -= gone.map_or(0, |v| v.md.len());
        }
        self.notes.retain(|_, l| !l.is_empty());
    }
}

fn path() -> PathBuf {
    crate::common::util::data_dir().join("versions.json")
}

fn load() -> Book {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn store(b: &Book) {
    let p = path();
    let tmp = p.with_extension("json.tmp");
    let ok = serde_json::to_vec(b).ok().is_some_and(|json| std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &p).is_ok());
    if !ok {
        crate::common::log::warn("versions: couldn't save");
    }
}

/// Note `id` as it is now in the store, as a version (None if there's no such note).
fn current(id: u64) -> Option<Version> {
    let (rtf, text, at) = crate::win::app::store(|s| s.note(id).map(|n| (n.rtf.clone(), n.text.clone(), n.updated)))?;
    let md = if rtf.is_empty() { text } else { crate::win::app::rtf_to_markdown(&rtf) };
    Some(Version { at, taken: crate::win::util::now(), md })
}

/// Note `id` is about to be changed by someone other than the person at its window (an
/// assistant), or put back to an earlier version: keep it as it is now.
pub fn keep(id: u64) {
    take(id, false);
}

/// Note `id` was just changed for the first time since its window opened: keep it as it
/// was saved before, unless its last version is only minutes old.
pub(super) fn keep_for_edit(id: u64) {
    take(id, true);
}

fn take(id: u64, quiet: bool) {
    let Some(v) = current(id) else { return };
    let mut b = load();
    if b.add(id, v, crate::win::util::now(), quiet) {
        store(&b);
    }
}

/// Note `id`'s versions, newest first.
pub fn list(id: u64) -> Vec<Version> {
    let mut l = load().notes.remove(&id).unwrap_or_default();
    l.reverse();
    l
}

/// Notes `ids` are gone for good: their versions too.
pub fn forget(ids: &[u64]) {
    let mut b = load();
    let before = b.notes.len();
    b.notes.retain(|id, _| !ids.contains(id));
    if b.notes.len() != before {
        store(&b);
    }
}

/// Every version kept, of every note, as Markdown: what still links to attachments when a
/// note's files are about to go. None when `versions.json` is there but can't be read, as
/// then nobody can say which files its versions need.
pub fn markdown() -> Option<Vec<String>> {
    let b: Book = match std::fs::read(path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).ok()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Book::default(),
        Err(_) => return None,
    };
    Some(b.notes.into_values().flatten().map(|v| v.md).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(md: &str, taken: i64) -> Version {
        Version { at: taken, taken, md: md.into() }
    }

    #[test]
    fn same_or_empty_isnt_kept_again() {
        let mut b = Book::default();
        assert!(b.add(1, v("a", 100), 100, false));
        assert!(!b.add(1, v("a", 200), 200, false));
        assert!(!b.add(1, v("  ", 300), 300, false));
        assert!(b.add(1, v("b", 400), 400, false));
        assert_eq!(b.notes[&1].len(), 2);
    }

    #[test]
    fn a_version_put_back_isnt_kept_twice() {
        let mut b = Book::default();
        assert!(b.add(1, v("a", 100), 100, false));
        assert!(b.add(1, v("b", 200), 200, false));
        // "a" put back, then edited (or undone) an hour on: it's in the list already.
        assert!(!b.add(1, v("a", 4000), 4000, true));
        assert_eq!(b.notes[&1].iter().map(|v| v.md.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn edits_close_together_make_one_version() {
        let mut b = Book::default();
        assert!(b.add(1, v("a", 0), 0, true));
        // Opened again to fix a typo a few minutes later: no new version.
        assert!(!b.add(1, v("b", 300), 300, true));
        // An assistant's change still keeps one, however soon.
        assert!(b.add(1, v("b", 310), 310, false));
        // And editing well after does.
        assert!(b.add(1, v("c", 2000), 2000, true));
        assert_eq!(b.notes[&1].iter().map(|v| v.md.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn kept_within_bounds() {
        let mut b = Book::default();
        for i in 0..15 {
            b.add(1, v(&format!("v{i}"), i * 1000), i * 1000, false);
        }
        assert_eq!(b.notes[&1].len(), PER_NOTE);
        assert_eq!(b.notes[&1][0].md, "v5");
        // A month on, the old ones are gone.
        b.add(2, v("x", MAX_AGE + 20_000), MAX_AGE + 20_000, false);
        assert!(!b.notes.contains_key(&1));
        // Too much altogether: the oldest go first, from any note.
        let big = "x".repeat(MAX_BYTES / 3);
        let t = MAX_AGE + 30_000;
        b.add(3, v(&format!("a{big}"), t), t, false);
        b.add(4, v(&format!("b{big}"), t + 1), t + 1, false);
        b.add(5, v(&format!("c{big}"), t + 2), t + 2, false);
        b.add(6, v(&format!("d{big}"), t + 3), t + 3, false);
        assert!(!b.notes.contains_key(&3), "the oldest big one went");
        assert!(b.notes.contains_key(&6));
        assert!(b.notes.values().flatten().map(|v| v.md.len()).sum::<usize>() <= MAX_BYTES);
    }
}
