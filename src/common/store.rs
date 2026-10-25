// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note database: one ordinary JSON file, written atomically.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
    /// The display the deck is on: its monitor's device path, which stays the same when
    /// Windows renumbers its displays; empty is the main display.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deck_display: String,
    /// That display's name when it was chosen, to say which one is missing while it's
    /// not connected.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub deck_display_name: String,
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
            deck_display: String::new(),
            deck_display_name: String::new(),
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
    /// The form of light text (`LIGHT_TEXT`) the notes' `text` was last brought up to.
    #[serde(default)]
    pub light_text: u32,
}

/// The form the notes' light text is written in. 1: pictures and files on a line of their
/// own are links (`![name](attachment:name)`), so previews can show them. Raised when a
/// change needs older notes' text rebuilt from their RTF, which then happens once.
pub const LIGHT_TEXT: u32 = 1;

impl Data {
    /// The notes whose light text predates `LIGHT_TEXT` and has to be rebuilt: those with
    /// an attachment, whose links an older form left out. None once that's been done,
    /// since a link inside a sentence never gets the line form, and its note would
    /// otherwise look out of date at every start.
    pub fn stale_light_text(&self) -> Vec<u64> {
        if self.light_text >= LIGHT_TEXT {
            return Vec::new();
        }
        self.notes.iter().filter(|n| n.rtf.contains(crate::common::attach::SCHEME) && !n.text.contains("](attachment:")).map(|n| n.id).collect()
    }
}

pub struct Store {
    pub path: PathBuf,
    pub d: Data,
    /// notes.json was there but couldn't be read (or put aside): this session never
    /// writes it, so whatever is in it survives for the next start to try again.
    frozen: bool,
    /// The notes' content (see `content_stamp`) as the file last got it.
    saved_stamp: Option<u64>,
    /// Saves since the backup was last refreshed, and when that was.
    saves_since_backup: u32,
    backup_at: Option<Instant>,
}

/// The backup is refreshed every so many saves, or after an hour of them, whichever comes
/// first: a power cut then costs minutes of notes, not the whole session.
const BACKUP_EVERY: u32 = 50;
const BACKUP_AGE: Duration = Duration::from_secs(3600);

/// A sync client or a virus scanner can hold notes.json open for a moment: the rename
/// waits it out, briefly, rather than failing the save.
fn rename_patiently(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            // 32 and 33 are sharing and lock violations; a scanner's hold can also show
            // up as access denied.
            Err(e) if tries < 5 && (e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33))) => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            r => return r,
        }
    }
}

/// Write `bytes` to `path` and see them onto the disk before returning: a power cut right
/// after the rename then can't leave an empty notes.json.
fn write_synced(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
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
        let mut s = Store { path, d, frozen, saved_stamp: None, saves_since_backup: 0, backup_at: Some(Instant::now()) };
        let max = s.d.notes.iter().map(|n| n.id).max().unwrap_or(0);
        s.d.next_id = s.d.next_id.max(max);
        // Early builds stored the deck at 42% height; the deck is now centred by default.
        if (s.d.settings.deck_pos - 0.42).abs() < 0.001 {
            s.d.settings.deck_pos = 0.5;
        }
        s
    }

    /// Write the notes (atomically: a synced temp file, then a rename). Err when they
    /// couldn't be, for the caller to show; the next save tries again.
    pub fn save(&mut self) -> std::io::Result<()> {
        let saved = self.write();
        // Saves follow every edit: log when they start and stop failing, not each one.
        // The error's kind and code only: its text can name the user's folder.
        static FAILING: AtomicBool = AtomicBool::new(false);
        match &saved {
            Err(e) if !FAILING.swap(true, Ordering::Relaxed) => log::error(&format!("saving notes.json failed ({:?}, code {:?})", e.kind(), e.raw_os_error())),
            Ok(()) if FAILING.swap(false, Ordering::Relaxed) => log::info("saving notes.json works again"),
            _ => {}
        }
        saved
    }

    fn write(&mut self) -> std::io::Result<()> {
        if self.frozen {
            return Err(std::io::Error::other("notes.json couldn't be read at start, so it isn't written this session"));
        }
        let json = serde_json::to_vec_pretty(&self.d).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        write_synced(&tmp, &json)?;
        rename_patiently(&tmp, &self.path)?;
        self.saved_stamp = Some(self.content_stamp());
        self.saves_since_backup += 1;
        if self.saves_since_backup >= BACKUP_EVERY || self.backup_at.is_none_or(|t| t.elapsed() >= BACKUP_AGE) {
            // The same bytes, so the backup is always a file that parsed.
            match std::fs::write(self.path.with_file_name("notes.backup.json"), &json) {
                Ok(()) => {
                    self.saves_since_backup = 0;
                    self.backup_at = Some(Instant::now());
                }
                Err(e) => log::warn(&format!("couldn't refresh notes.backup.json ({:?})", e.kind())),
            }
        }
        Ok(())
    }

    /// A fingerprint of everything in the notes but where the user is in them (window
    /// rectangles, carets, scroll positions, the last note): what has to reach the disk
    /// at once. Cheap: a few fields a note, never the text.
    fn content_stamp(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        (self.d.next_id, self.d.light_text).hash(&mut h);
        for n in &self.d.notes {
            (n.id, n.color, n.archived, n.pinned, n.off_deck, n.created, n.updated, n.order, n.text.len(), n.rtf.len()).hash(&mut h);
        }
        let mut settings = self.d.settings.clone();
        settings.last_note = None;
        serde_json::to_string(&settings).unwrap_or_default().hash(&mut h);
        h.finish()
    }

    /// Whether nothing but places (see `content_stamp`) changed since the last save that
    /// worked: such a save can wait a few seconds and go with the next.
    pub fn only_view_changed(&self) -> bool {
        self.saved_stamp == Some(self.content_stamp())
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

/// The most characters a note holds (about 16,000 words): typing and pasting stop there,
/// and imports and assistants bring in nothing longer. Notes are short, so this is far past
/// what they need, and a note this long still opens at once and keeps up with typing.
pub const MAX_NOTE_CHARS: usize = 100_000;

/// Whether `s` starts the way a list item, checklist item, heading, quote or code fence is
/// written in Markdown, so that plain text starting so has to be written escaped ("\- not
/// a list").
pub fn looks_like_marker(s: &str) -> bool {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    ["- ", "* ", "+ ", "• ", "[ ] ", "[x] ", "[X] ", "> ", "```"].iter().any(|m| s.starts_with(m))
        || s == ">"
        || ((1..=6).contains(&hashes) && s[hashes..].starts_with(' '))
        || (digits > 0 && s[digits..].starts_with(". "))
}

/// A code block's opening or closing line.
pub fn is_fence(l: &str) -> bool {
    l.trim() == "```"
}

/// The text of a quote's paragraph, when `l` is one ("> Ship it on Friday").
pub fn quoted(l: &str) -> Option<&str> {
    let t = l.trim_start();
    t.strip_prefix("> ").or_else(|| (t.trim_end() == ">").then_some(""))
}

/// How a table column lines up, from the colons in its separator cell.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// The cells of a table row ("| Name | Qty |") as written (see `cell_text`); None when `l`
/// isn't one. An escaped pipe ("\|") stays inside its cell.
pub fn table_cells(l: &str) -> Option<Vec<&str>> {
    let t = l.trim().strip_prefix('|')?;
    let (mut cells, mut start, mut esc) = (Vec::new(), 0, false);
    for (i, ch) in t.char_indices() {
        if ch == '|' && !esc {
            cells.push(&t[start..i]);
            start = i + 1;
        }
        esc = ch == '\\' && !esc;
    }
    // The closing pipe is optional.
    if !t[start..].trim().is_empty() {
        cells.push(&t[start..]);
    }
    (!cells.is_empty()).then_some(cells)
}

/// The column alignments in a table's separator row ("| --- | :---: | ---: |"); None when
/// `l` isn't one.
pub fn table_align(l: &str) -> Option<Vec<Align>> {
    table_cells(l)?
        .iter()
        .map(|c| {
            let c = c.trim();
            let dashes = c.trim_start_matches(':').trim_end_matches(':');
            (!dashes.is_empty() && dashes.bytes().all(|b| b == b'-')).then_some(match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}

/// A table cell as one line of text: its line breaks become spaces.
pub fn cell_text(c: &str) -> String {
    let t = c.replace("<br>", " ").replace("<br/>", " ").replace("<br />", " ").replace("\\|", "|");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `lines` begin with a table: a row, then a separator row.
fn table_starts<'a>(mut lines: impl Iterator<Item = &'a str>) -> bool {
    lines.next().and_then(table_cells).is_some() && lines.next().and_then(table_align).is_some()
}

/// Markdown for reading outside the app, so viewers show the note's lines as it has them:
/// a plain line followed by another ends in two spaces (Markdown's line break) rather
/// than joining it into one paragraph, and a plain line right after a list gets a blank
/// line before it, which would otherwise read as part of the last item. A plain text
/// editor shows nothing extra.
///
/// Code blocks are left exactly as they are. Quote paragraphs break like plain lines, and
/// a table gets a blank line on either side, without which the text next to it would read
/// as more of the table (or, after a quote, as more of the quote).
pub fn hard_breaks(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    let quote = |l: &str| quoted(l).is_some();
    let row = |l: &str| table_cells(l).is_some();
    let item = |l: &str| {
        let t = l.trim_start();
        looks_like_marker(t) && !t.starts_with('#') && !quote(t) && !t.starts_with("```")
    };
    let plain = |l: &str| !l.trim().is_empty() && !item(l) && !quote(l) && !row(l) && !is_fence(l) && !l.trim_start().starts_with('#');
    let mut out = String::with_capacity(md.len() + lines.len() * 2);
    let mut code = false;
    for (i, l) in lines.iter().enumerate() {
        out.push_str(l);
        let Some(&next) = lines.get(i + 1) else { break };
        if is_fence(l) {
            code = !code;
        } else if code {
            // Exactly as written.
        } else if (plain(l) && plain(next)) || (quote(l) && quote(next)) {
            out.push_str("  ");
        } else if ((item(l) || quote(l) || row(l)) && plain(next)) || (plain(l) && table_starts(lines[i + 1..].iter().copied())) {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// A line written escaped because its text only looks like a marker: that text.
pub fn unescaped(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix('\\').filter(|r| looks_like_marker(r))
}

/// A picture or file on a line of its own, as a note's light text has it
/// (`![name](attachment:name)`, `[name](attachment:name)`): whether it's a picture, and its
/// name.
pub fn attachment_line(l: &str) -> Option<(bool, &str)> {
    let t = l.trim();
    let (picture, rest) = match t.strip_prefix("![") {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('[')?),
    };
    let (name, url) = rest.split_once("](")?;
    let url = url.strip_suffix(')')?;
    url.starts_with(crate::common::attach::SCHEME).then_some((picture, name))
}

/// Strip a leading list marker ("- ", "1. ", "[ ] " ...) or quote mark ("> ") from a line.
/// A code fence or a table's separator row has no text at all; a picture or file line is
/// its name.
pub fn strip_marker(line: &str) -> &str {
    if let Some(r) = unescaped(line) {
        return r;
    }
    if let Some((_, name)) = attachment_line(line) {
        return name;
    }
    if is_fence(line) || table_align(line).is_some() {
        return "";
    }
    if let Some(q) = quoted(line) {
        return strip_marker(q);
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

/// Where a note's title comes from: its text, the line its body starts at, and whether
/// the body starts inside the code block the title was taken from. The title is the first
/// line with text: in a code block, its first line; in a table, its first row with text,
/// as its cells (the whole table stays in the body, so the preview still shows it).
fn head(text: &str) -> (String, usize, bool) {
    let mut lines = text.lines().enumerate().peekable();
    while let Some((i, l)) = lines.next() {
        if is_fence(l) {
            for (k, c) in lines.by_ref() {
                if is_fence(c) {
                    break;
                }
                if !c.trim().is_empty() {
                    return (c.trim().to_string(), k + 1, true);
                }
            }
            continue;
        }
        if table_cells(l).is_some() && lines.peek().is_some_and(|(_, n)| table_align(n).is_some()) {
            let mut row = Some(l);
            while let Some(r) = row {
                let cells: Vec<String> = table_cells(r).unwrap_or_default().into_iter().map(cell_text).filter(|c| !c.is_empty()).collect();
                if table_align(r).is_none() && !cells.is_empty() {
                    return (cells.join(" \u{b7} "), i, false);
                }
                row = lines.next_if(|(_, n)| table_cells(n).is_some()).map(|(_, n)| n);
            }
            continue;
        }
        let t = strip_marker(l).trim();
        if !t.is_empty() {
            return (t.to_string(), i + 1, false);
        }
    }
    (String::new(), usize::MAX, false)
}

pub fn title_of(text: &str) -> String {
    let (line, _, _) = head(text);
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
/// previews. Nested items keep two leading spaces per level (see `ui::draw_body`). Quotes,
/// code blocks and tables stay as written, for `ui::draw_body` to lay out; a code block
/// the title came from opens again where the body starts.
pub fn body_of(text: &str) -> String {
    let (_, start, code) = head(text);
    let rest = text.lines().skip(start);
    if code { preview(std::iter::once("```").chain(rest)) } else { preview(rest) }
}

/// Like `body_of`, from line `first` of `text` (0 is the title line) on: what a scrolled
/// note shows, for the card flying to or from its window. A code block or table that
/// `first` is partway into opens again (a table without its header row).
pub fn body_from(text: &str, first: usize) -> String {
    let (mut code, mut sep, mut prev) = (false, None, "");
    for l in text.lines().take(first) {
        if is_fence(l) {
            code = !code;
            sep = None;
        } else if !code {
            sep = match sep {
                Some(s) if table_cells(l).is_some() => Some(s),
                _ if table_cells(prev).is_some() && table_align(l).is_some() => Some(l),
                _ => None,
            };
        }
        prev = l;
    }
    let rest = text.lines().skip(first);
    match sep {
        _ if code => preview(std::iter::once("```").chain(rest)),
        Some(s) if text.lines().nth(first).is_some_and(|l| table_cells(l).is_some()) => preview(std::iter::once(s).chain(rest)),
        _ => preview(rest),
    }
}

fn preview<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    let mut code = false;
    for l in lines.take(PREVIEW_LINES) {
        let l: &str = match l.char_indices().nth(PREVIEW_LINE_CHARS) {
            Some((i, _)) => &l[..i],
            None => l,
        };
        if is_fence(l) {
            code = !code;
            out.push_str("```\n");
            continue;
        }
        // A code block's lines are its text exactly; quotes, tables, pictures and files are
        // laid out from their syntax.
        if code || quoted(l).is_some() || table_cells(l).is_some() || attachment_line(l).is_some() {
            out.push_str(l);
            out.push('\n');
            continue;
        }
        out.push_str(&pretty_line(l));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// A body line as previews show it: list markers as the glyphs the editor draws (• ◦ ▪,
/// ○ ●, 1. a. i.), nested two spaces a level.
pub fn pretty_line(l: &str) -> String {
    let t = l.trim_start();
    let level = ((l.len() - t.len()) / 2).min(5);
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    let pretty = if let Some(r) = unescaped(t) {
        // Text that only looks like a quote or a fence stays escaped, or the preview
        // would draw it as one (`ui::draw_body` shows it unescaped).
        if quoted(r).is_some() || r.starts_with("```") { t.to_string() } else { r.to_string() }
    } else if let Some(r) = t.strip_prefix("[ ] ") {
        format!("○  {r}")
    } else if let Some(r) = t.strip_prefix("[x] ") {
        format!("●  {r}")
    } else if let Some(r) = t.strip_prefix("- ") {
        // The editor's bullet for the level: • ◦ ▪.
        format!("{}  {r}", ['•', '◦', '▪'][level % 3])
    } else if let Some(r) = t[digits..].strip_prefix(". ").filter(|_| digits > 0) {
        format!("{}. {r}", list_number(t[..digits].parse().unwrap_or(1), level))
    } else {
        t.to_string()
    };
    format!("{}{pretty}", "  ".repeat(level))
}

/// A line of a preview (from `body_of`) as text for a one-line summary: quote marks, code
/// fences and table syntax aside, and list glyphs too.
pub fn summary_line(l: &str) -> std::borrow::Cow<'_, str> {
    let l = l.trim();
    if let Some((_, name)) = attachment_line(l) {
        return name.into();
    }
    if is_fence(l) || table_align(l).is_some() {
        return "".into();
    }
    if let Some(cells) = table_cells(l) {
        return cells.into_iter().map(cell_text).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" \u{b7} ").into();
    }
    let l = unescaped(l).or(quoted(l)).unwrap_or(l);
    l.trim().trim_start_matches(['○', '●', '•', '◦', '▪', ' ']).into()
}

/// What a search looks through besides the title: everything after the title line as
/// plain text. List markers and quote marks go, and so do code fences and table syntax:
/// a row is its cells, separated by spaces.
pub fn search_rest(text: &str) -> String {
    let (_, start, mut code) = head(text);
    let mut out: Vec<std::borrow::Cow<str>> = Vec::new();
    for l in text.lines().skip(start) {
        if is_fence(l) {
            code = !code;
        } else if code {
            out.push(l.into());
        } else if table_align(l).is_some() {
            // A table's separator row is only syntax.
        } else if let Some(cells) = table_cells(l) {
            out.push(cells.into_iter().map(cell_text).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" ").into());
        } else {
            out.push(strip_marker(l).into());
        }
    }
    out.join("\n")
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
    fn attachment_lines() {
        assert_eq!(attachment_line("![sunset.jpg](attachment:sunset.jpg)"), Some((true, "sunset.jpg")));
        assert_eq!(attachment_line("[Q3 plan.pdf](attachment:Q3%20plan.pdf)"), Some((false, "Q3 plan.pdf")));
        assert_eq!(attachment_line("[site](https://example.com)"), None);
        let text = "Trip\n![sunset.jpg](attachment:sunset.jpg)\n[plan.pdf](attachment:plan.pdf)\nend";
        assert_eq!(body_of(text), "![sunset.jpg](attachment:sunset.jpg)\n[plan.pdf](attachment:plan.pdf)\nend");
        assert_eq!(strip_marker("[plan.pdf](attachment:plan.pdf)"), "plan.pdf");
        assert_eq!(summary_line("![sunset.jpg](attachment:sunset.jpg)"), "sunset.jpg");
        assert_eq!(title_of("![sunset.jpg](attachment:sunset.jpg)\nmore"), "sunset.jpg");
    }

    #[test]
    fn light_text_is_rebuilt_once() {
        let note = |id, text: &str| Note { id, rtf: r#"{\rtf1 {\field{\*\fldinst{HYPERLINK "attachment:plan.pdf"}}{\fldrslt plan.pdf}}}"#.into(), text: text.into(), ..Default::default() };
        let mut d = Data {
            notes: vec![
                // Saved before previews showed files: just the name.
                note(1, "Trip\nplan.pdf"),
                // Its link inside a sentence, which the light text never writes as a link.
                note(2, "Trip\nSee plan.pdf before Friday"),
                note(3, "Trip\n[plan.pdf](attachment:plan.pdf)"),
                Note { id: 4, rtf: r"{\rtf1 plain}".into(), text: "plain".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        assert_eq!(d.stale_light_text(), [1, 2]);
        // What refresh_light_text does: rebuild them (note 2 comes out the same) and mark it done.
        d.notes[0].text = "Trip\n[plan.pdf](attachment:plan.pdf)".into();
        d.light_text = LIGHT_TEXT;
        assert!(d.stale_light_text().is_empty(), "note 2 isn't picked again");
        // And so after the next load.
        let d: Data = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(d.stale_light_text().is_empty());
    }

    #[test]
    fn deck_display_defaults_to_the_main_one() {
        // Settings saved before the deck could go on another display.
        let d: Data = serde_json::from_str(r#"{"settings":{"edge_left":true,"deck_pos":0.3,"language":"de"}}"#).unwrap();
        assert!(d.settings.edge_left && d.settings.language == "de");
        assert!(d.settings.deck_display.is_empty() && d.settings.deck_display_name.is_empty());
        // Left out while it's the main display, so such files stay as they were.
        assert!(!serde_json::to_string(&d).unwrap().contains("deck_display"));
        let mut d = d;
        d.settings.deck_display = r"\\?\DISPLAY#DEL40F4#5&1&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}".into();
        d.settings.deck_display_name = "DELL U2720Q".into();
        let back: Data = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.settings.deck_display, d.settings.deck_display);
        assert_eq!(back.settings.deck_display_name, "DELL U2720Q");
    }


    #[test]
    fn breaks_lines_for_previewers() {
        let md = "# Standup\nYesterday: retries\nToday: search\n- one\n  - nested\n- two\nafter\n\n![a.jpg](attachments/a.jpg)\nAll good\n1. first\n2. second";
        let want = "# Standup\nYesterday: retries  \nToday: search\n- one\n  - nested\n- two\n\nafter\n\n![a.jpg](attachments/a.jpg)  \nAll good\n1. first\n2. second";
        assert_eq!(hard_breaks(md), want);
    }

    #[test]
    fn breaks_around_blocks() {
        let md = "Notes\n```\nlet a = 1;\nlet b = 2;\n```\n> one\n> two\nafter\n| A | B |\n| - | - |\n| 1 | 2 |\nend";
        let want = "Notes\n```\nlet a = 1;\nlet b = 2;\n```\n> one  \n> two\n\nafter\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nend";
        assert_eq!(hard_breaks(md), want);
    }

    #[test]
    fn titles_from_blocks() {
        assert_eq!(title_of("```\n\n  fn main() {\n}\n```\nafter"), "fn main() {");
        assert_eq!(title_of("> Ship it on Friday\n> Tell Sam"), "Ship it on Friday");
        assert_eq!(title_of("| Name | Qty | Note |\n| --- | :---: | ---: |\n| Tea | 2 | green |"), "Name \u{b7} Qty \u{b7} Note");
        assert_eq!(title_of("|  |  |\n| - | - |\n| Tea | 2<br>cups |"), "Tea \u{b7} 2 cups");
        assert_eq!(title_of("```\n\n```\nReal title"), "Real title");
        assert_eq!(title_of("| not | a table\nafter"), "| not | a table");
        assert_eq!(title_of("\\> not a quote"), "> not a quote");
    }

    #[test]
    fn bodies_keep_blocks() {
        // A code block the title came from opens again; its lines stay as they are.
        assert_eq!(body_of("```\nfn main() {\n  - x\n}\n```\n- after"), "```\n  - x\n}\n```\n•  after");
        assert_eq!(body_of("> Ship it\n> Tell Sam\nDone"), "> Tell Sam\nDone");
        // A table's title is its header; the preview still shows all of it.
        let t = "| Name | Qty |\n| --- | ---: |\n| Tea | 2 |";
        assert_eq!(body_of(t), t);
        assert_eq!(body_of("Title\n```\n[ ] not a task\n```\n[ ] a task"), "```\n[ ] not a task\n```\n○  a task");
        // Text that only looks like a quote or a fence stays escaped.
        assert_eq!(body_of("Title\n\\> not a quote\n\\```"), "\\> not a quote\n\\```");
    }

    #[test]
    fn scrolled_bodies_reopen_blocks() {
        let code = "Title\n```\na\nb\n```\nafter";
        assert_eq!(body_from(code, 3), "```\nb\n```\nafter");
        assert_eq!(body_from(code, 5), "after");
        let table = "Title\n| A | B |\n| - | -: |\n| 1 | 2 |\n| 3 | 4 |";
        assert_eq!(body_from(table, 1), "| A | B |\n| - | -: |\n| 1 | 2 |\n| 3 | 4 |");
        assert_eq!(body_from(table, 4), "| - | -: |\n| 3 | 4 |");
    }

    #[test]
    fn markers_of_blocks() {
        assert_eq!(strip_marker("> quoted"), "quoted");
        assert_eq!(strip_marker("> - quoted item"), "quoted item");
        assert_eq!(strip_marker(">"), "");
        assert_eq!(strip_marker("```"), "");
        assert_eq!(strip_marker("| --- | :---: | ---: |"), "");
        assert_eq!(strip_marker("| a | b |"), "| a | b |");
        assert_eq!(strip_marker("\\> literal"), "> literal");
        assert_eq!(table_align("| --- | :---: | ---: |"), Some(vec![Align::Left, Align::Center, Align::Right]));
        assert_eq!(table_align("| a | --- |"), None);
        assert_eq!(table_cells("| a \\| b | c"), Some(vec![" a \\| b ", " c"]));
        assert!(looks_like_marker("> a") && looks_like_marker("```js") && !looks_like_marker(">a"));
    }

    #[test]
    fn saves_atomically_and_tells_places_from_content() {
        let dir = std::env::temp_dir().join(format!("peekaboo-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Store::load(dir.clone());
        let id = s.create("Trip\n- passport", "{\\rtf1 Trip}");
        assert!(!s.only_view_changed(), "a new note has to be written");
        s.save().unwrap();
        assert!(s.only_view_changed() && !dir.join("notes.json.tmp").exists());
        // Where the note is on screen can wait; what it says can't.
        s.note_mut(id).unwrap().rect = Some([1, 2, 3, 4]);
        s.note_mut(id).unwrap().view = Some([5, 0, 0]);
        s.d.settings.last_note = Some(id);
        assert!(s.only_view_changed());
        s.note_mut(id).unwrap().text.push_str("\n- charger");
        s.note_mut(id).unwrap().updated += 1;
        assert!(!s.only_view_changed());
        s.d.settings.theme = 2;
        s.save().unwrap();
        // The backup goes with a save once it's due, from the same bytes.
        assert!(!dir.join("notes.backup.json").exists());
        s.backup_at = None;
        s.save().unwrap();
        assert_eq!(std::fs::read(dir.join("notes.backup.json")).unwrap(), std::fs::read(dir.join("notes.json")).unwrap());
        let again = Store::load(dir.clone());
        assert_eq!(again.note(id).unwrap().text, "Trip\n- passport\n- charger");
        assert_eq!(again.d.settings.theme, 2);
        // A frozen store says so instead of writing.
        let mut frozen = Store::load(dir.clone());
        frozen.frozen = true;
        assert!(frozen.save().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn searches_block_text() {
        let text = "Title\n> quoted\n```\n- code\n```\n| A | B |\n| --- | --- |\n| x<br>y | \\| z |";
        assert_eq!(search_rest(text), "quoted\n- code\nA B\nx y | z");
        assert_eq!(search_rest("Plain\n- item\n1. one"), "item\none");
    }
}
