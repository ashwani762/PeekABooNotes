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

/// Strip a leading list marker ("- ", "1. ", "[ ] " ...) or quote mark ("> ") from a line.
/// A code fence or a table's separator row has no text at all.
pub fn strip_marker(line: &str) -> &str {
    if let Some(r) = unescaped(line) {
        return r;
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
        // A code block's lines are its text exactly; quotes and tables are laid out from
        // their syntax.
        if code || quoted(l).is_some() || table_cells(l).is_some() {
            out.push_str(l);
            out.push('\n');
            continue;
        }
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

/// A line of a preview (from `body_of`) as text for a one-line summary: quote marks, code
/// fences and table syntax aside, and list glyphs too.
pub fn summary_line(l: &str) -> std::borrow::Cow<'_, str> {
    let l = l.trim();
    if is_fence(l) || table_align(l).is_some() {
        return "".into();
    }
    if let Some(cells) = table_cells(l) {
        return cells.into_iter().map(cell_text).filter(|c| !c.is_empty()).collect::<Vec<_>>().join(" \u{b7} ").into();
    }
    let l = unescaped(l).or(quoted(l)).unwrap_or(l);
    l.trim().trim_start_matches(['○', '●', '•', ' ']).into()
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
    fn searches_block_text() {
        let text = "Title\n> quoted\n```\n- code\n```\n| A | B |\n| --- | --- |\n| x<br>y | \\| z |";
        assert_eq!(search_rest(text), "quoted\n- code\nA B\nx y | z");
        assert_eq!(search_rest("Plain\n- item\n1. one"), "item\none");
    }
}
