// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! A note's earlier versions, for what Ctrl+Z can't reach: the editor's undo covers what
//! was done since the note was opened, and these go back further. One is kept when a note
//! is first changed after it was opened or after `IDLE` without changes (unless the last
//! one is only minutes old), and before an assistant changes it. Saving as you type
//! doesn't keep one. Kept as Markdown in `versions.json` beside the notes,
//! written only when a version is taken or dropped and read only then and when they're
//! listed, so nothing of them stays in memory.
//!
//! An assistant can't wear the user's versions away: what an assistant wrote is marked
//! so when it's kept, such versions have a budget of their own and go first when there
//! are too many, and a note's newest version of the user's own is never dropped for size.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Versions kept per note, newest last.
const PER_NOTE: usize = 10;
/// Of which at most this many of what an assistant wrote.
const ASSISTANT_PER_NOTE: usize = 5;
/// None older than this (seconds).
const MAX_AGE: i64 = 30 * 24 * 3600;
/// Nor more than this of them altogether: the oldest go first.
const MAX_BYTES: usize = 5 << 20;
/// How far ahead of the last prune's clock one prune may age versions (seconds): a clock
/// set years ahead for a moment can't take every version at once.
const STEP: i64 = 24 * 3600;
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
    /// The note as an assistant left it, rather than the user's own writing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub assistant: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct Book {
    #[serde(default)]
    notes: BTreeMap<u64, Vec<Version>>,
    /// The notes an assistant changed last: what they hold now is its writing.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    assistant_made: BTreeSet<u64>,
    /// The clock as the last prune had it (see `STEP`).
    #[serde(default)]
    clock: i64,
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

    /// Drop what's too old, too many, and (oldest first, across notes) too much. An
    /// assistant's versions go first; a note's newest version of the user's own stays
    /// whatever the size.
    fn prune(&mut self, now: i64) {
        // Back with the wall clock, but forward at most `STEP` a prune. A version from
        // ahead of it (taken while the clock was wrong) isn't aged at all.
        let clock = if self.clock == 0 { now } else { now.min(self.clock + STEP) };
        self.clock = clock;
        for list in self.notes.values_mut() {
            list.retain(|v| clock - v.taken < MAX_AGE);
            while list.iter().filter(|v| v.assistant).count() > ASSISTANT_PER_NOTE {
                drop_oldest(list, true);
            }
            while list.len() > PER_NOTE {
                let assistant = list.iter().any(|v| v.assistant);
                drop_oldest(list, assistant);
            }
        }
        let mut total: usize = self.notes.values().flatten().map(|v| v.md.len()).sum();
        while total > MAX_BYTES {
            let newest_own = |l: &[Version], i: usize| !l[i + 1..].iter().any(|v| !v.assistant);
            let pick = self.oldest(|l, i| l[i].assistant).or_else(|| self.oldest(|l, i| !newest_own(l, i)));
            let Some((id, i)) = pick else { break };
            let gone = self.notes.get_mut(&id).map(|l| l.remove(i));
            total -= gone.map_or(0, |v| v.md.len());
        }
        self.notes.retain(|_, l| !l.is_empty());
    }

    /// The oldest version, across notes, that `ok` (its note's list and its place there)
    /// lets go: its note and place.
    fn oldest(&self, ok: impl Fn(&[Version], usize) -> bool) -> Option<(u64, usize)> {
        self.notes.iter().flat_map(|(&id, l)| (0..l.len()).filter(|&i| ok(l, i)).map(move |i| (l[i].taken, id, i))).min().map(|(_, id, i)| (id, i))
    }
}

/// Drop the oldest version in `list` that is (`assistant`) or isn't an assistant's.
fn drop_oldest(list: &mut Vec<Version>, assistant: bool) {
    if let Some(i) = list.iter().position(|v| v.assistant == assistant) {
        list.remove(i);
    }
}

fn path() -> PathBuf {
    crate::common::util::data_dir().join("versions.json")
}

/// The versions in `p`: none when there's no file yet, Err (what went wrong) when it's
/// there but can't be read or understood, as then it must not be written over.
fn load_from(p: &Path) -> Result<Book, String> {
    match std::fs::read(p) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{:?} error", e.classify())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Book::default()),
        Err(e) => Err(format!("{:?}", e.kind())),
    }
}

fn load() -> Result<Book, String> {
    load_from(&path())
}

/// Write `b` to `p` as notes.json is written: a temp file, flushed, renamed.
fn store_to(p: &Path, b: &Book) -> bool {
    serde_json::to_vec(b).ok().is_some_and(|json| crate::common::store::replace_file(p, &json).is_ok())
}

fn store(b: &Book) {
    if !store_to(&path(), b) {
        crate::common::log::warn("versions: couldn't save");
    }
}

/// Note `id` as it is now in the store, as a version (None if there's no such note).
fn current(id: u64) -> Option<Version> {
    let (rtf, text, at) = crate::win::app::store(|s| s.note(id).map(|n| (n.rtf.clone(), n.text.clone(), n.updated)))?;
    let md = if rtf.is_empty() { text } else { crate::win::app::rtf_to_markdown(&rtf) };
    Some(Version { at, taken: crate::win::util::now(), md, assistant: false })
}

/// Note `id` is about to be put back to an earlier version: keep it as it is now.
pub fn keep(id: u64) {
    take(id, false, false);
}

/// Note `id` is about to be changed by an assistant: keep it as it is now, and mark what
/// it will hold as the assistant's.
pub fn keep_for_assistant(id: u64) {
    take(id, false, true);
}

/// Note `id` was just changed for the first time since its window opened: keep it as it
/// was saved before, unless its last version is only minutes old.
pub(super) fn keep_for_edit(id: u64) {
    take(id, true, false);
}

/// Keep note `id` as it is now (`quiet`: see `Book::add`); `assistant_next` says whether
/// what it holds after is an assistant's writing. Nothing is kept, nor written, when
/// `versions.json` is there but can't be read.
fn take(id: u64, quiet: bool, assistant_next: bool) {
    let Some(mut v) = current(id) else { return };
    let mut b = match load() {
        Ok(b) => b,
        Err(e) => {
            crate::common::log::warn(&format!("versions: none kept, versions.json can't be read ({e})"));
            return;
        }
    };
    v.assistant = b.assistant_made.contains(&id);
    let added = b.add(id, v, crate::win::util::now(), quiet);
    let marked = if assistant_next { b.assistant_made.insert(id) } else { b.assistant_made.remove(&id) };
    if added || marked {
        store(&b);
    }
}

/// Note `id`'s versions, newest first.
pub fn list(id: u64) -> Vec<Version> {
    let mut l = load().unwrap_or_default().notes.remove(&id).unwrap_or_default();
    l.reverse();
    l
}

/// Notes `ids` are gone for good: their versions too.
pub fn forget(ids: &[u64]) {
    let mut b = match load() {
        Ok(b) => b,
        Err(e) => {
            crate::common::log::warn(&format!("versions: deleted notes' versions kept, versions.json can't be read ({e})"));
            return;
        }
    };
    let before = (b.notes.len(), b.assistant_made.len());
    b.notes.retain(|id, _| !ids.contains(id));
    b.assistant_made.retain(|id| !ids.contains(id));
    if (b.notes.len(), b.assistant_made.len()) != before {
        store(&b);
    }
}

/// Every version kept, of every note, as Markdown: what still links to attachments when a
/// note's files are about to go. None when `versions.json` is there but can't be read, as
/// then nobody can say which files its versions need.
pub fn markdown() -> Option<Vec<String>> {
    Some(load().ok()?.notes.into_values().flatten().map(|v| v.md).collect())
}

/// The highest note id `versions.json` keeps versions for (0 for none): a new note must
/// never take one, or it would show a lost note's versions as its own.
pub fn max_id() -> u64 {
    max_id_in(&path())
}

fn max_id_in(p: &Path) -> u64 {
    #[derive(Deserialize)]
    struct Ids {
        #[serde(default)]
        notes: BTreeMap<u64, serde::de::IgnoredAny>,
    }
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Ids>(&b).ok()).and_then(|i| i.notes.keys().max().copied()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(md: &str, taken: i64) -> Version {
        Version { at: taken, taken, md: md.into(), assistant: false }
    }

    /// What an assistant wrote.
    fn va(md: &str, taken: i64) -> Version {
        Version { assistant: true, ..v(md, taken) }
    }

    fn mds(b: &Book, id: u64) -> Vec<&str> {
        b.notes.get(&id).map(|l| l.iter().map(|v| v.md.as_str()).collect()).unwrap_or_default()
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
        assert_eq!(mds(&b, 1), ["a", "b"]);
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
        assert_eq!(mds(&b, 1), ["a", "b", "c"]);
    }

    #[test]
    fn kept_within_bounds() {
        let mut b = Book::default();
        for i in 0..15 {
            b.add(1, v(&format!("v{i}"), i * 1000), i * 1000, false);
        }
        assert_eq!(b.notes[&1].len(), PER_NOTE);
        assert_eq!(b.notes[&1][0].md, "v5");
        // A month on (a day at a time, as the clock goes), the old ones are gone.
        let mut t = 14_000;
        while t < MAX_AGE + 20_000 {
            t += STEP;
            b.add(2, v(&format!("x{t}"), t), t, false);
        }
        assert!(!b.notes.contains_key(&1));
        // Too much altogether: an assistant's go first, then the user's older ones, from
        // any note; never a note's newest of the user's own.
        let mut b = Book::default();
        let big = "x".repeat(MAX_BYTES / 3);
        let t = 1_000_000;
        b.add(3, v(&format!("a1{big}"), t), t, false);
        b.add(3, v(&format!("a2{big}"), t + 1), t + 1, false);
        b.add(4, va(&format!("b{big}"), t + 2), t + 2, false);
        b.add(5, v(&format!("c{big}"), t + 3), t + 3, false);
        assert!(!b.notes.contains_key(&4), "the assistant's went first");
        assert_eq!(b.notes[&3].len(), 1, "then the older of note 3's");
        assert!(b.notes[&3][0].md.starts_with("a2") && b.notes.contains_key(&5));
        assert!(b.notes.values().flatten().map(|v| v.md.len()).sum::<usize>() <= MAX_BYTES);
        // However much more there is, each note's newest of the user's own stays.
        b.add(6, v(&format!("d{big}"), t + 4), t + 4, false);
        b.add(7, v(&format!("e{big}"), t + 5), t + 5, false);
        assert!([3, 5, 6, 7].iter().all(|id| b.notes.contains_key(id)));
    }

    /// An assistant blanking a note and rewriting it again and again can't wear away the
    /// user's own versions, by count or by size.
    #[test]
    fn an_assistant_cant_flush_the_users_versions() {
        let mut b = Book::default();
        let t = 1_000_000;
        for i in 0..4 {
            b.add(1, v(&format!("mine {i}"), t + i), t + i, false);
        }
        for i in 0..30 {
            b.add(1, va(&format!("assistant {i}"), t + 100 + i), t + 100 + i, false);
        }
        let l = &b.notes[&1];
        assert_eq!(l.iter().filter(|v| !v.assistant).count(), 4, "all the user's stay");
        assert_eq!(l.iter().filter(|v| v.assistant).count(), ASSISTANT_PER_NOTE);
        assert_eq!(l.last().unwrap().md, "assistant 29");
        // By size: big writes of its own push out only its own and the user's older ones.
        let big = "y".repeat(MAX_BYTES / 4);
        for i in 0..12 {
            b.add(2, va(&format!("{i}{big}"), t + 200 + i), t + 200 + i, false);
        }
        assert!(b.notes[&1].iter().any(|v| v.md == "mine 3"), "the user's newest stays");
        assert!(b.notes.values().flatten().map(|v| v.md.len()).sum::<usize>() <= MAX_BYTES);
    }

    /// A clock set far ahead for a moment ages versions a day at a time, not all at once;
    /// one gone back keeps versions from "ahead" of it.
    #[test]
    fn clock_jumps_dont_age_everything() {
        let mut b = Book::default();
        let t = 1_000_000;
        b.add(1, v("old", t), t, false);
        let years = t + 5 * 365 * 24 * 3600;
        b.add(2, v("set ahead", years), years, false);
        assert_eq!(mds(&b, 1), ["old"], "one prune moves the clock a day");
        // Back to the right time: the version taken "in the future" isn't aged.
        b.add(3, v("back", t + 10), t + 10, false);
        assert_eq!(b.clock, t + 10);
        assert!(b.notes.contains_key(&1) && b.notes.contains_key(&2));
    }

    #[test]
    fn an_unreadable_book_is_never_written_over() {
        let dir = std::env::temp_dir().join(format!("peekaboo-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("versions.json");
        assert!(load_from(&p).is_ok_and(|b| b.notes.is_empty()), "none yet");
        std::fs::write(&p, "{ damaged").unwrap();
        assert!(load_from(&p).is_err());
        // Written whole, and read back; the highest id is there for new notes to stay above.
        let mut b = Book::default();
        b.add(41, v("kept", 100), 100, false);
        b.add(7, v("too", 100), 100, false);
        assert!(store_to(&p, &b));
        assert!(!dir.join("versions.json.tmp").exists());
        assert_eq!(mds(&load_from(&p).unwrap(), 41), ["kept"]);
        assert_eq!(max_id_in(&p), 41);
        assert_eq!(max_id_in(&dir.join("missing.json")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
