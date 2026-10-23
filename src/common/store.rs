// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note database: one ordinary JSON file, written atomically.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::common::log;
use crate::common::util::now;

/// notes.json, retrying briefly: a sync client or a virus scanner can hold it open for a
/// moment, and that must not look like an unreadable (let alone missing) file.
fn read_patiently(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let mut tries = 0;
    loop {
        match std::fs::read(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound && tries < 10 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// The copy of the last good notes.json, for when notes.json itself won't do.
fn from_backup(backup: &std::path::Path) -> Data {
    match std::fs::read(backup).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<Data>(&b).map_err(|e| format!("{:?} error", e.classify()))) {
        Ok(d) => {
            log::warn(&format!("restored {} notes from notes.backup.json", d.notes.len()));
            d
        }
        Err(e) => {
            log::error(&format!("notes.backup.json didn't help either ({e}); starting empty"));
            Data::default()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Note {
    pub id: u64,
    /// Rich content as RTF, as produced by the RichEdit control.
    #[serde(default)]
    pub rtf: String,
    /// Plain text with light Markdown list markers; used for titles, search, previews and export.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub color: u8,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub pinned: bool,
    /// Kept out of the edge deck (still active, still in All Notes).
    #[serde(default)]
    pub off_deck: bool,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub updated: i64,
    #[serde(default)]
    pub order: i64,
    /// Last window rectangle (screen coordinates), so notes reopen where you left them.
    #[serde(default)]
    pub rect: Option<[i32; 4]>,
    /// Where you were in the text, so the note reopens there scrolled as you left it: the
    /// caret, the first character in view (UTF-16 positions), and how many px of that
    /// character's line were scrolled out of view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<[i32; 3]>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    #[serde(default)]
    pub edge_left: bool,
    #[serde(default)]
    pub deck_hidden: bool,
    #[serde(default)]
    pub over_fullscreen: bool,
    /// Vertical centre of the deck as a fraction of the work area height.
    #[serde(default = "default_pos")]
    pub deck_pos: f32,
    /// 0 follows Windows, 1 light, 2 dark.
    #[serde(default)]
    pub theme: u8,
    #[serde(default)]
    pub reduce_motion: bool,
    /// Most cards the deck shows; the rest are a click away in All Notes.
    #[serde(default = "default_deck_max")]
    pub deck_max: u8,
    /// Colour for new notes; None cycles through every colour.
    #[serde(default)]
    pub new_color: Option<u8>,
    /// New notes join the top of the deck instead of the bottom.
    #[serde(default)]
    pub new_on_top: bool,
    /// Note typeface: 0 system, 1 serif, 2 mono, 3 handwriting.
    #[serde(default)]
    pub note_font: u8,
    /// Note text size: 0 small, 1 medium, 2 large.
    #[serde(default = "default_note_size")]
    pub note_size: u8,
    /// Interface language tag ("de", "pt-BR"); empty follows Windows. English to begin with.
    #[serde(default = "default_language")]
    pub language: String,
    /// The note last worked on, for "Reopen Last Note" (also after a restart).
    #[serde(default)]
    pub last_note: Option<u64>,
    /// Notes that stay open (pinned, or all with Keep Open) and were open at quit: they
    /// open again at the next start.
    #[serde(default)]
    pub reopen: Vec<u64>,
    /// Where the last export went; the folder picker starts there.
    #[serde(default)]
    pub export_dir: String,
    /// Unpinned notes stay open when you click away (by default they fly back to the deck).
    #[serde(default)]
    pub keep_open: bool,
    /// AI assistants may use the notes through MCP (off until turned on).
    #[serde(default)]
    pub mcp: bool,
}

fn default_note_size() -> u8 {
    1
}

fn default_language() -> String {
    "en".into()
}

fn default_pos() -> f32 {
    0.5
}

fn default_deck_max() -> u8 {
    8
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            edge_left: false,
            deck_hidden: false,
            over_fullscreen: false,
            deck_pos: default_pos(),
            theme: 0,
            reduce_motion: false,
            deck_max: default_deck_max(),
            new_color: None,
            new_on_top: false,
            note_font: 0,
            note_size: default_note_size(),
            language: default_language(),
            last_note: None,
            reopen: Vec::new(),
            export_dir: String::new(),
            keep_open: false,
            mcp: false,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Data {
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub settings: Settings,
}

pub struct Store {
    pub path: PathBuf,
    pub d: Data,
    /// notes.json was there but couldn't be read (or put aside): this session never
    /// writes it, so whatever is in it survives for the next start to try again.
    frozen: bool,
}

impl Store {
    pub fn load(dir: PathBuf) -> Store {
        let path = dir.join("notes.json");
        let backup = dir.join("notes.backup.json");
        let mut frozen = false;
        let d = match read_patiently(&path) {
            Ok(bytes) => match serde_json::from_slice::<Data>(&bytes) {
                Ok(d) => {
                    log::info(&format!("loaded {} notes ({} KB)", d.notes.len(), bytes.len().div_ceil(1024)));
                    // A copy of the last good file from the previous session.
                    if let Err(e) = std::fs::write(dir.join("notes.backup.json"), &bytes) {
                        log::warn(&format!("couldn't write notes.backup.json: {e}"));
                    }
                    d
                }
                Err(e) => {
                    // Never overwrite a file we failed to understand.
                    let aside = format!("notes.unreadable-{}.json", now());
                    let moved = std::fs::rename(&path, dir.join(&aside));
                    // Category and position only: serde's message can quote note text.
                    log::error(&format!(
                        "notes.json is unreadable ({:?} error at line {}, column {}); {}",
                        e.classify(),
                        e.line(),
                        e.column(),
                        match &moved {
                            Ok(()) => format!("moved aside as {aside}"),
                            Err(e) => format!("moving it aside failed ({e}), so it won't be written this session"),
                        }
                    ));
                    frozen = moved.is_err();
                    from_backup(&backup)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::info("no notes.json yet, starting fresh");
                Data::default()
            }
            Err(e) => {
                log::error(&format!("couldn't read notes.json ({e}), so it won't be written this session"));
                frozen = true;
                from_backup(&backup)
            }
        };
        let mut s = Store { path, d, frozen };
        let max = s.d.notes.iter().map(|n| n.id).max().unwrap_or(0);
        s.d.next_id = s.d.next_id.max(max);
        // Early builds stored the deck at 42% height; the deck is now centred by default.
        if (s.d.settings.deck_pos - 0.42).abs() < 0.001 {
            s.d.settings.deck_pos = 0.5;
        }
        s
    }

    pub fn save(&self) {
        if self.frozen {
            return; // see `frozen`; logged once, at load
        }
        let saved = serde_json::to_vec_pretty(&self.d).map_err(std::io::Error::other).and_then(|json| {
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, &self.path)
        });
        // Saves follow every edit: log when they start and stop failing, not each one.
        static FAILING: AtomicBool = AtomicBool::new(false);
        match saved {
            Err(e) if !FAILING.swap(true, Ordering::Relaxed) => log::error(&format!("saving notes.json failed: {e}")),
            Ok(()) if FAILING.swap(false, Ordering::Relaxed) => log::info("saving notes.json works again"),
            _ => {}
        }
    }

    pub fn note(&self, id: u64) -> Option<&Note> {
        self.d.notes.iter().find(|n| n.id == id)
    }

    pub fn note_mut(&mut self, id: u64) -> Option<&mut Note> {
        self.d.notes.iter_mut().find(|n| n.id == id)
    }

    /// Active (not archived) notes in deck order.
    pub fn active_ids(&self) -> Vec<u64> {
        let mut v: Vec<&Note> = self.d.notes.iter().filter(|n| !n.archived).collect();
        v.sort_by(|a, b| a.order.cmp(&b.order).then(b.created.cmp(&a.created)));
        v.into_iter().map(|n| n.id).collect()
    }

    pub fn create(&mut self, text: &str, rtf: &str) -> u64 {
        self.d.next_id += 1;
        let id = self.d.next_id;
        let t = now();
        let active = self.d.notes.iter().filter(|n| !n.archived).map(|n| n.order);
        let order = if self.d.settings.new_on_top { active.min().unwrap_or(0) - 1 } else { active.max().unwrap_or(0) + 1 };
        let hues = crate::common::theme::ORDER;
        let color = self.d.settings.new_color.unwrap_or(hues[self.d.notes.len() % hues.len()]);
        self.d.notes.push(Note {
            id,
            rtf: rtf.into(),
            text: text.into(),
            color,
            created: t,
            updated: t,
            order,
            ..Default::default()
        });
        id
    }

    pub fn remove(&mut self, id: u64) -> Option<(usize, Note)> {
        let i = self.d.notes.iter().position(|n| n.id == id)?;
        let n = self.d.notes.remove(i);
        // "Reopen Last Note" moves on to the note edited most recently.
        if self.d.settings.last_note == Some(id) {
            self.d.settings.last_note = self.d.notes.iter().filter(|n| !n.archived).max_by_key(|n| n.updated).map(|n| n.id);
        }
        Some((i, n))
    }

    pub fn insert_at(&mut self, i: usize, n: Note) {
        let i = i.min(self.d.notes.len());
        self.d.notes.insert(i, n);
    }

    pub fn reorder(&mut self, ids: &[u64]) {
        for (i, id) in ids.iter().enumerate() {
            if let Some(n) = self.note_mut(*id) {
                n.order = i as i64;
            }
        }
    }
}

const MARKERS: [&str; 6] = ["[ ] ", "[x] ", "- [ ] ", "- [x] ", "- ", "* "];

/// Whether `s` starts the way a list item, checklist item or heading is written in
/// Markdown, so that plain text starting so has to be written escaped ("\- not a list").
pub fn looks_like_marker(s: &str) -> bool {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    ["- ", "* ", "+ ", "• ", "[ ] ", "[x] ", "[X] "].iter().any(|m| s.starts_with(m))
        || ((1..=6).contains(&hashes) && s[hashes..].starts_with(' '))
        || (digits > 0 && s[digits..].starts_with(". "))
}

/// Markdown for reading outside the app. A line that runs on into the next one (not a
/// heading, not before a bullet or heading, which start their own block) ends in two
/// spaces, Markdown's line break: previewers then keep the note's lines apart instead of
/// joining them into one paragraph, and a plain text editor shows nothing extra.
pub fn hard_breaks(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out = String::with_capacity(md.len() + lines.len() * 2);
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        let Some(next) = lines.get(i + 1).map(|n| n.trim_start()) else { break };
        let starts_block = looks_like_marker(next) && !next.starts_with(|c: char| c.is_ascii_digit());
        if !l.trim().is_empty() && !l.trim_start().starts_with('#') && !next.is_empty() && !starts_block {
            out.push_str("  ");
        }
        out.push('\n');
    }
    out
}

/// A line written escaped because its text only looks like a marker: that text.
pub fn unescaped(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix('\\').filter(|r| looks_like_marker(r))
}

/// Strip a leading list marker ("- ", "1. ", "[ ] " ...) from a line.
pub fn strip_marker(line: &str) -> &str {
    if let Some(r) = unescaped(line) {
        return r;
    }
    let l = line.trim_start();
    for m in MARKERS {
        if let Some(rest) = l.strip_prefix(m) {
            return rest;
        }
    }
    let digits = l.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0
        && let Some(rest) = l[digits..].strip_prefix(". ") {
            return rest;
        }
    l
}

pub fn title_of(text: &str) -> String {
    let line = text.lines().map(|l| strip_marker(l).trim()).find(|l| !l.is_empty()).unwrap_or("");
    if line.is_empty() {
        return crate::t!("note_untitled").into();
    }
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    t
}

/// The most a preview ever shows: more than the tallest card holds, so its fade-out is
/// still the cut, while a huge note costs no more to preview than a long one.
const PREVIEW_LINES: usize = 20;
const PREVIEW_LINE_CHARS: usize = 300;

/// The start of everything after the title line, with markers turned into glyphs for
/// previews. Nested items keep two leading spaces per level (see `ui::draw_body`).
pub fn body_of(text: &str) -> String {
    let mut lines = text.lines().skip_while(|l| strip_marker(l).trim().is_empty());
    lines.next();
    preview(lines)
}

/// Like `body_of`, from paragraph `first` (0 is the title line) on: what a scrolled note
/// shows, for the card flying to or from its window.
pub fn body_from(text: &str, first: usize) -> String {
    preview(text.lines().skip(first))
}

fn preview<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for l in lines.take(PREVIEW_LINES) {
        let l: &str = match l.char_indices().nth(PREVIEW_LINE_CHARS) {
            Some((i, _)) => &l[..i],
            None => l,
        };
        let t = l.trim_start();
        let level = ((l.len() - t.len()) / 2).min(5);
        let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
        let pretty = if let Some(r) = unescaped(t) {
            r.to_string()
        } else if let Some(r) = t.strip_prefix("[ ] ") {
            format!("○  {r}")
        } else if let Some(r) = t.strip_prefix("[x] ") {
            format!("●  {r}")
        } else if let Some(r) = t.strip_prefix("- ") {
            format!("•  {r}")
        } else if let Some(r) = t[digits..].strip_prefix(". ").filter(|_| digits > 0) {
            format!("{}. {r}", list_number(t[..digits].parse().unwrap_or(1), level))
        } else {
            t.to_string()
        };
        out.push_str(&"  ".repeat(level));
        out.push_str(&pretty);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Item `n` of a numbered list at nesting `level`, styled like the editor: 1. a. i.
fn list_number(n: usize, level: usize) -> String {
    match level % 3 {
        0 => n.to_string(),
        1 => char::from(b'a' + ((n.max(1) - 1) % 26) as u8).to_string(),
        _ => {
            const R: [(usize, &str); 7] = [(50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
            let (mut n, mut out) = (n.clamp(1, 89), String::new());
            for (v, r) in R {
                while n >= v {
                    out.push_str(r);
                    n -= v;
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breaks_lines_for_previewers() {
        let md = "# Standup\nYesterday: retries\nToday: search\n- one\n  - nested\n- two\nafter\n\n![a.jpg](attachments/a.jpg)\nAll good\n1. first\n2. second";
        let want = "# Standup\nYesterday: retries  \nToday: search\n- one\n  - nested\n- two  \nafter\n\n![a.jpg](attachments/a.jpg)  \nAll good  \n1. first  \n2. second";
        assert_eq!(hard_breaks(md), want);
    }
}
