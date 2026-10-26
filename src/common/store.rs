// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note database: one ordinary JSON file, written atomically.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::common::log;
use crate::common::util::now;

/// notes.json, retrying briefly: a sync client or a virus scanner can hold it open for a
/// moment, and that must not look like an unreadable (let alone missing) file.
fn read_patiently(path: &Path) -> std::io::Result<Vec<u8>> {
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

/// The newest copy of the notes the backups hold that can be read, and which file it was:
/// `notes.backup.json`, then the daily ones, then those kept from being replaced (newest
/// first). None when there's none.
fn from_backup(dir: &Path) -> Option<(Data, String)> {
    let mut names = vec![BACKUP.to_string()];
    names.extend(backups(dir, true));
    names.extend(backups(dir, false));
    for name in names {
        let read = std::fs::read(dir.join(&name)).map_err(|e| e.kind()).map(|b| serde_json::from_slice::<Data>(&b));
        match read {
            Ok(Ok(d)) => {
                log::warn(&format!("restored {} notes from {name}", d.notes.len()));
                return Some((d, name));
            }
            Ok(Err(e)) => log::error(&format!("{name} is unreadable too ({:?} error)", e.classify())),
            Err(std::io::ErrorKind::NotFound) => {}
            Err(kind) => log::error(&format!("couldn't read {name} ({kind:?})")),
        }
    }
    None
}

/// How many notes the notes file at `path` holds; None when it can't be read.
fn count_notes(path: &Path) -> Option<usize> {
    #[derive(Deserialize)]
    struct Count {
        #[serde(default)]
        notes: Vec<serde::de::IgnoredAny>,
    }
    serde_json::from_slice::<Count>(&std::fs::read(path).ok()?).ok().map(|c| c.notes.len())
}

/// Whether a file of `new` notes holds far fewer than a backup of `old`: then it isn't
/// allowed to take that backup's place (it's kept under another name first).
fn far_fewer(new: usize, old: usize) -> bool {
    old >= 4 && new * 2 < old
}

/// Whether `dir` is a new notes folder, with nothing in it notes would have left: no
/// notes file or backup of any kind, no attachments, no versions. Only then is a missing
/// notes.json a fresh start; otherwise it's a folder whose notes can't be seen right now
/// (a drive or share not there yet, a sync client part way), which must not be written.
fn folder_is_new(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("notes.") || name == "versions.json" {
            return false;
        }
        if name == "attachments" && std::fs::read_dir(e.path()).map_or(true, |mut d| d.next().is_some()) {
            return false;
        }
    }
    true
}

/// A quick 64-bit fingerprint of bytes, the same however they're fed in: for telling
/// whether notes.json still holds what this session wrote, and for content stamps. Not
/// for anything an adversary picks.
#[derive(Clone, Copy)]
pub struct Fingerprint {
    h: u64,
    buf: [u8; 8],
    n: usize,
    len: u64,
}

const MIX: u64 = 0x517c_c1b7_2722_0a95;

impl Default for Fingerprint {
    fn default() -> Self {
        Fingerprint { h: 0x9e37_79b9_7f4a_7c15, buf: [0; 8], n: 0, len: 0 }
    }
}

impl Fingerprint {
    fn word(&mut self, w: u64) {
        self.h = (self.h.rotate_left(5) ^ w).wrapping_mul(MIX);
    }

    pub fn update(&mut self, mut bytes: &[u8]) {
        self.len += bytes.len() as u64;
        if self.n > 0 {
            let take = (8 - self.n).min(bytes.len());
            self.buf[self.n..self.n + take].copy_from_slice(&bytes[..take]);
            self.n += take;
            bytes = &bytes[take..];
            if self.n < 8 {
                return;
            }
            self.word(u64::from_le_bytes(self.buf));
            self.n = 0;
        }
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.word(u64::from_le_bytes(*w));
        }
        self.buf[..rest.len()].copy_from_slice(rest);
        self.n = rest.len();
    }

    pub fn finish(mut self) -> u64 {
        let mut last = [0u8; 8];
        last[..self.n].copy_from_slice(&self.buf[..self.n]);
        self.word(u64::from_le_bytes(last));
        self.word(self.len);
        self.h
    }

    pub fn of(bytes: &[u8]) -> u64 {
        let mut f = Fingerprint::default();
        f.update(bytes);
        f.finish()
    }
}

/// A writer that fingerprints what goes through it.
struct Fingerprinting<W> {
    inner: W,
    print: Fingerprint,
}

impl<W: std::io::Write> std::io::Write for Fingerprinting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.print.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Replace the file at `path` with `bytes`, atomically and onto the disk: a temp file,
/// flushed, then renamed over it. A temp file left by a failure is removed.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_of(path);
    let written = (|| {
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_patiently(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// `path` with `.tmp` added: where a new copy of it is written before it takes its place.
fn tmp_of(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".tmp");
    PathBuf::from(p)
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
    /// Fields from a newer version of the app, kept as they are so a save doesn't lose them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
    /// The shortcuts that work anywhere, where the user changed them: each one's name in
    /// `hotkey::ACTIONS` and its keys ("Win+Shift+N"), "" for one turned off. Empty, and
    /// left out of the file, while every shortcut has its default keys.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub hotkeys: std::collections::BTreeMap<String, String>,
    /// Settings from a newer version of the app, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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
            hotkeys: std::collections::BTreeMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Data {
    /// The form of the file (`FORMAT`). One from a newer version is read, but not written.
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub next_id: u64,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub settings: Settings,
    /// The form of light text (`LIGHT_TEXT`) the notes' `text` was last brought up to.
    #[serde(default)]
    pub light_text: u32,
    /// The form pictures' sizes in the notes' RTF were last brought up to
    /// (`PICTURE_SIZES`).
    #[serde(default)]
    pub picture_sizes: u32,
    /// Anything else a newer version of the app wrote, kept as it is.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The form notes.json is written in. A file of a later form comes from a newer version
/// of the app, which may mean something by it this one can't keep: it's read, and the
/// session doesn't write it. Fields this version doesn't know are kept in every form
/// (`extra`), so a file only a little newer survives a save whole.
pub const FORMAT: u32 = 1;

/// The form the notes' light text is written in. 1: pictures and files on a line of their
/// own are links (`![name](attachment:name)`), so previews can show them. Raised when a
/// change needs older notes' text rebuilt from their RTF, which then happens once.
pub const LIGHT_TEXT: u32 = 1;

/// The form pictures' sizes are stored in. 1: a picture keeps its own size (as made, or as
/// the user dragged it to), never the narrower one a narrow note showed it at; before, it
/// was stored as last shown and widened back to its pixels' size whenever there was room.
pub const PICTURE_SIZES: u32 = 1;

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
    /// notes.json was read and understood at start: these are all the notes there are,
    /// not the backup's or none. Only then may anything be judged unused by every note.
    pub read_whole: bool,
    /// The session began in a new folder, without a notes.json: one that appears later
    /// (a sync client bringing the notes down) is never written over.
    fresh: bool,
    /// notes.json as this session last read or wrote it; None while it hasn't.
    disk: Option<Seen>,
    /// The notes' content (see `content_stamp`) as the file last got it.
    saved_stamp: Option<u64>,
    /// Saves since the backup was last refreshed, and when that was.
    saves_since_backup: u32,
    backup_at: Option<Instant>,
    /// How many notes notes.backup.json holds, once known.
    backup_notes: Option<usize>,
    /// What the user is to be told, for the platform to show (`take_notices`).
    notices: Vec<Notice>,
    /// Another writer's notes.json has been set aside this session (the user is told once).
    told_conflict: bool,
}

/// notes.json as seen on disk: its size and last change, and its bytes' fingerprint.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Seen {
    len: u64,
    modified: Option<SystemTime>,
    print: u64,
}

/// What happened to the notes file that the user should hear about.
#[derive(Clone, PartialEq, Debug)]
pub enum Notice {
    /// notes.json couldn't be read: the backup is open, and the file was kept under this
    /// name.
    SetAside(String),
    /// notes.json couldn't be read (another program holds it, say): the backup is open,
    /// and nothing is saved this session.
    Locked,
    /// notes.json was missing, with a backup beside it: the backup is open.
    Missing,
    /// notes.json was changed by another program or computer since this session last read
    /// or wrote it: that copy was kept under this name before the notes were saved.
    Conflict(String),
}

const NOTES: &str = "notes.json";
const BACKUP: &str = "notes.backup.json";
/// Daily backups, `notes.backup-2026-10-26.json`: the backup as it was when the app first
/// started that day. The newest `DAILY_KEEP` stay.
const DAILY_KEEP: usize = 3;
/// A backup that a file of far fewer notes would have replaced, kept under this name
/// (`notes.backup-kept-<time>.json`) instead and never removed by the app.
const KEPT: &str = "notes.backup-kept-";
/// Another writer's notes.json, set aside before this session's notes were saved.
pub const CONFLICT: &str = "notes.conflict-";
/// A notes.json that couldn't be read at start, set aside.
pub const UNREADABLE: &str = "notes.unreadable-";

/// A free name `<prefix><unix time>.json` in `dir` (with `-2`, `-3` when taken): set-aside
/// files must never land on one another.
fn aside_name(dir: &Path, prefix: &str) -> String {
    let t = now();
    let mut name = format!("{prefix}{t}.json");
    let mut k = 2;
    while dir.join(&name).exists() {
        name = format!("{prefix}{t}-{k}.json");
        k += 1;
    }
    name
}

/// Size and last change of the file at `path`.
fn stat(path: &Path) -> std::io::Result<(u64, Option<SystemTime>)> {
    std::fs::metadata(path).map(|m| (m.len(), m.modified().ok()))
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

/// Write `d` to `path` as JSON and see it onto the disk before returning: a power cut
/// right after the rename then can't leave an empty notes.json. It goes out through a
/// small buffer, so a save never holds a second copy of all the notes in memory. Gives
/// the written bytes' fingerprint.
fn write_synced(path: &Path, d: &Data) -> std::io::Result<u64> {
    use std::io::Write;
    let file = Fingerprinting { inner: std::fs::File::create(path)?, print: Fingerprint::default() };
    let mut w = std::io::BufWriter::with_capacity(64 * 1024, file);
    serde_json::to_writer_pretty(&mut w, d).map_err(std::io::Error::other)?;
    w.flush()?;
    let file = w.into_inner().map_err(std::io::IntoInnerError::into_error)?;
    file.inner.sync_all()?;
    Ok(file.print.finish())
}

/// Copy `from` over `to` the way `replace_file` writes: a temp copy, flushed, renamed.
fn copy_synced(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = tmp_of(to);
    let copied = (|| {
        std::fs::copy(from, &tmp)?;
        std::fs::OpenOptions::new().write(true).open(&tmp)?.sync_all()?;
        rename_patiently(&tmp, to)
    })();
    if copied.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    copied
}

/// The backups in `dir`: the daily ones (`daily`) or those kept from being replaced,
/// newest first.
fn backups(dir: &Path, daily: bool) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| n.starts_with("notes.backup-") && n.ends_with(".json") && n.starts_with(KEPT) != daily)
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}

/// Whether notes set aside unread, or another writer's notes.json set aside, are kept
/// in `dir`: notes that may come back, with the files they link. True when it can't be
/// told.
pub fn others_kept(dir: &Path) -> bool {
    std::fs::read_dir(dir).map_or(true, |d| d.flatten().any(|e| e.file_name().to_str().is_none_or(|n| n.starts_with(UNREADABLE) || n.starts_with(CONFLICT))))
}

impl Store {
    pub fn load(dir: PathBuf) -> Store {
        let mut s = Store {
            path: dir.join(NOTES),
            d: Data::default(),
            frozen: false,
            read_whole: false,
            fresh: false,
            disk: None,
            saved_stamp: None,
            saves_since_backup: 0,
            backup_at: Some(Instant::now()),
            backup_notes: None,
            notices: Vec::new(),
            told_conflict: false,
        };
        // What a save that failed part way left.
        let _ = std::fs::remove_file(tmp_of(&s.path));
        // When it last changed, from before it's read: a change made while it's being read
        // then shows at the first save.
        let modified = stat(&s.path).ok().and_then(|m| m.1);
        match read_patiently(&s.path) {
            Ok(bytes) => match serde_json::from_slice::<Data>(&bytes) {
                Ok(d) => {
                    s.read_whole = true;
                    s.disk = Some(Seen { len: bytes.len() as u64, modified, print: Fingerprint::of(&bytes) });
                    log::info(&format!("loaded {} notes ({} KB)", d.notes.len(), bytes.len().div_ceil(1024)));
                    // A copy of the last good file from the previous session (one from a
                    // newer version is left to its own backups).
                    if d.version <= FORMAT {
                        s.back_up(&dir, Some(&bytes), d.notes.len(), true);
                    }
                    s.d = d;
                }
                Err(e) => {
                    // Never overwrite a file we failed to understand.
                    let aside = aside_name(&dir, UNREADABLE);
                    let moved = std::fs::rename(&s.path, dir.join(&aside));
                    // Category and position only: serde's message can quote note text.
                    log::error(&format!(
                        "notes.json is unreadable ({:?} error at line {}, column {}); {}",
                        e.classify(),
                        e.line(),
                        e.column(),
                        match &moved {
                            Ok(()) => format!("moved aside as {aside}"),
                            Err(e) => format!("moving it aside failed ({:?}), so it won't be written this session", e.kind()),
                        }
                    ));
                    s.frozen = moved.is_err();
                    s.open_backup(&dir);
                    s.notices.push(if moved.is_ok() { Notice::SetAside(aside) } else { Notice::Locked });
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some((d, _)) = from_backup(&dir) {
                    s.d = d;
                    // Gone from beside its backup (deleted, or a sync client part way):
                    // the backup's notes are used and saved as notes.json; should the file
                    // come back meanwhile, it's set aside before a save (see `write`).
                    log::warn("notes.json is missing: the backup is open instead");
                    s.notices.push(Notice::Missing);
                } else if folder_is_new(&dir) {
                    log::info("no notes.json yet, starting fresh");
                    s.fresh = true;
                } else {
                    // A drive, share or synced folder that isn't there yet, or notes.json
                    // gone from a folder notes were kept in: writing would start the notes
                    // over on top of the real ones when they come back.
                    log::error("notes.json is missing from a folder that held notes (or the folder can't be reached), so it won't be written this session");
                    s.frozen = true;
                }
            }
            Err(e) => {
                log::error(&format!("couldn't read notes.json ({:?}, code {:?}), so it won't be written this session", e.kind(), e.raw_os_error()));
                s.frozen = true;
                s.open_backup(&dir);
                s.notices.push(Notice::Locked);
            }
        }
        if s.d.version > FORMAT && !s.frozen {
            log::error(&format!("the notes are in a newer form ({}) than this version writes ({FORMAT}), so they won't be written this session", s.d.version));
            s.frozen = true;
        }
        let max = s.d.notes.iter().map(|n| n.id).max().unwrap_or(0);
        s.d.next_id = s.d.next_id.max(max);
        // Early builds stored the deck at 42% height; the deck is now centred by default.
        if (s.d.settings.deck_pos - 0.42).abs() < 0.001 {
            s.d.settings.deck_pos = 0.5;
        }
        s
    }

    /// The newest backup that can be read, in place of notes.json. False when there's none
    /// (the notes start empty).
    fn open_backup(&mut self, dir: &Path) -> bool {
        match from_backup(dir) {
            Some((d, _)) => {
                self.d = d;
                true
            }
            None => {
                log::warn("no backup could be read");
                false
            }
        }
    }

    /// Refresh notes.backup.json with the notes just read (`bytes`) or just written (the
    /// file at `path`), which hold `count` notes. At the day's first start the backup there
    /// is kept first as that day's; and a backup of far more notes than these isn't
    /// replaced but kept under a name of its own.
    fn back_up(&mut self, dir: &Path, bytes: Option<&[u8]>, count: usize, starting: bool) {
        let main = dir.join(BACKUP);
        let old = self.backup_notes.or_else(|| count_notes(&main));
        if let Some(old) = old.filter(|&old| far_fewer(count, old)) {
            let kept = aside_name(dir, KEPT);
            if let Err(e) = rename_patiently(&main, &dir.join(&kept)) {
                log::warn(&format!("notes.backup.json holds {old} notes and the notes now {count}; it stays, as it couldn't be kept aside ({:?})", e.kind()));
                return;
            }
            log::warn(&format!("notes.backup.json held {old} notes and the notes now {count}: kept it as {kept}"));
        } else if starting && old.is_some() {
            let t = crate::os::local_time(now());
            let daily = format!("notes.backup-{:04}-{:02}-{:02}.json", t.year, t.month, t.day);
            if !dir.join(&daily).exists() {
                match rename_patiently(&main, &dir.join(&daily)) {
                    Ok(()) => {
                        for gone in backups(dir, true).into_iter().skip(DAILY_KEEP) {
                            let _ = std::fs::remove_file(dir.join(gone));
                        }
                    }
                    Err(e) => log::warn(&format!("couldn't keep the day's backup ({:?})", e.kind())),
                }
            }
        }
        let written = match bytes {
            Some(b) => replace_file(&main, b),
            None => copy_synced(&self.path, &main),
        };
        match written {
            Ok(()) => self.backup_notes = Some(count),
            Err(e) => log::warn(&format!("couldn't refresh notes.backup.json ({:?})", e.kind())),
        }
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
        self.set_aside_others()?;
        self.d.version = FORMAT;
        let tmp = tmp_of(&self.path);
        let written = write_synced(&tmp, &self.d).and_then(|print| rename_patiently(&tmp, &self.path).map(|()| print));
        let print = match written {
            Ok(print) => print,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let (len, modified) = stat(&self.path).unwrap_or((0, None));
        self.disk = Some(Seen { len, modified, print });
        self.fresh = false;
        self.saved_stamp = Some(self.content_stamp());
        self.saves_since_backup += 1;
        if self.saves_since_backup >= BACKUP_EVERY || self.backup_at.is_none_or(|t| t.elapsed() >= BACKUP_AGE) {
            // A copy of the file just written, so the backup is always one that was whole.
            // Tried again only when the next is due, should it fail.
            let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
            self.back_up(&dir, None, self.d.notes.len(), false);
            self.saves_since_backup = 0;
            self.backup_at = Some(Instant::now());
        }
        Ok(())
    }

    /// Before a write: a notes.json that isn't the one this session last read or wrote
    /// (another program or computer wrote it, or it appeared) is never written over. It's
    /// moved aside as `notes.conflict-<time>.json`, and the user told; in a session that
    /// began without one, the notes there are the real ones, and this session stops
    /// writing instead. Err when the file couldn't be moved aside.
    fn set_aside_others(&mut self) -> std::io::Result<()> {
        // None there, or none to be seen (the write will show why).
        let Ok((len, modified)) = stat(&self.path) else { return Ok(()) };
        if let Some(seen) = self.disk {
            if modified.is_some() && (seen.len, seen.modified) == (len, modified) {
                return Ok(());
            }
            // Touched, but holding what was written (a sync client marking it as synced).
            if read_patiently(&self.path).is_ok_and(|b| Fingerprint::of(&b) == seen.print) {
                self.disk = Some(Seen { len, modified, ..seen });
                return Ok(());
            }
        }
        if self.fresh {
            self.frozen = true;
            log::error("a notes.json appeared after this session began without one; it won't be written this session");
            return Err(std::io::Error::other("notes.json appeared since the start, so it isn't written this session"));
        }
        let dir = self.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let aside = aside_name(&dir, CONFLICT);
        rename_patiently(&self.path, &dir.join(&aside))?;
        log::warn(&format!("notes.json was changed by another program or computer since this session last read or wrote it: kept that copy as {aside}"));
        if !self.told_conflict {
            self.told_conflict = true;
            self.notices.push(Notice::Conflict(aside));
        }
        Ok(())
    }

    /// What the user is to be told since this was last asked, once each.
    pub fn take_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// Whether these are all the notes there are and this session writes them: only then
    /// may a file be judged unused by every note.
    pub fn whole(&self) -> bool {
        self.read_whole && !self.frozen
    }

    /// A fingerprint of everything in the notes but where the user is in them (window
    /// rectangles, carets, scroll positions, the last note): what has to reach the disk
    /// at once. The text goes through the quick `Fingerprint` (a fraction of a
    /// millisecond for megabytes of notes), so an edit that leaves a note as long as it
    /// was still counts as one.
    fn content_stamp(&self) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        (self.d.next_id, self.d.light_text).hash(&mut h);
        for n in &self.d.notes {
            let (text, rtf) = (Fingerprint::of(n.text.as_bytes()), Fingerprint::of(n.rtf.as_bytes()));
            (n.id, n.color, n.archived, n.pinned, n.off_deck, n.created, n.updated, n.order, text, rtf).hash(&mut h);
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
        self.active_notes().into_iter().map(|n| n.id).collect()
    }

    /// Active notes in deck order, themselves rather than their ids (looking each id up
    /// again would go through every note once per note).
    pub fn active_notes(&self) -> Vec<&Note> {
        let mut v: Vec<&Note> = self.d.notes.iter().filter(|n| !n.archived).collect();
        v.sort_by(|a, b| a.order.cmp(&b.order).then(b.created.cmp(&a.created)));
        v
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

    /// Notes `remove` took out, with the places it gave, in the order it took them: each
    /// goes back where it was, the last one taken first, so every place is among the
    /// notes there were when it was taken.
    pub fn put_back(&mut self, removed: Vec<(usize, Note)>) {
        for (i, n) in removed.into_iter().rev() {
            self.insert_at(i, n);
        }
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
/// a list"). An empty item's marker alone ("-", "2.") is one, and so is any `>`
/// (">>> print", which import reads as quotes).
pub fn looks_like_marker(s: &str) -> bool {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    ["- ", "* ", "+ ", "• ", "[ ] ", "[x] ", "[X] ", ">", "```"].iter().any(|m| s.starts_with(m))
        || ["-", "*", "+", "•", "[ ]", "[x]", "[X]"].contains(&s)
        || ((1..=6).contains(&hashes) && s[hashes..].starts_with(' '))
        || (digits > 0 && (s[digits..].starts_with(". ") || &s[digits..] == "."))
}

/// Whether a plain line `s` has to be written escaped to come back as the text it is: it
/// starts as a marker would, after any spaces ("  - not a list"), or as an escaped line
/// does ("\- " typed out).
pub fn needs_escape(s: &str) -> bool {
    looks_like_marker(s.trim_start()) || unescaped(s).is_some()
}

/// A code block's opening or closing line.
pub fn is_fence(l: &str) -> bool {
    l.trim() == "```"
}

/// A code block's opening line, as Markdown has it: three backticks or more, then the
/// language or nothing (```rust): how many backticks, and the language. Backticks after
/// the first ones make it a line of text (```inline```).
pub fn fence(l: &str) -> Option<(usize, &str)> {
    let t = l.trim();
    let ticks = t.bytes().take_while(|&b| b == b'`').count();
    let info = t[ticks..].trim();
    (ticks >= 3 && !info.contains('`')).then_some((ticks, info))
}

/// Whether `l` closes a code block opened with `ticks` backticks: as many or more, alone.
pub fn closes_fence(l: &str, ticks: usize) -> bool {
    let t = l.trim();
    t.len() >= ticks && t.bytes().all(|b| b == b'`')
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
/// as more of the table (or, after a quote, as more of the quote). What a quote holds
/// (a quote or code block in it too) breaks the same way, a level in.
pub fn hard_breaks(md: &str) -> String {
    let lines: Vec<&str> = md.split('\n').collect();
    breaks(&lines).join("\n")
}

/// Whether `l` is a list or checklist item, as `hard_breaks` tells one.
pub fn list_line(l: &str) -> bool {
    let t = l.trim_start();
    looks_like_marker(t) && !t.starts_with('#') && !t.starts_with('>') && !t.starts_with("```")
}

/// Whether `l` is a line of plain text, as `hard_breaks` tells one: not empty, a list item,
/// a quote, a table row, a code fence or a heading. One right after a list item gets a
/// blank line before it, which import takes out again.
pub fn plain_line(l: &str) -> bool {
    !l.trim().is_empty() && !list_line(l) && quoted(l).is_none() && table_cells(l).is_none() && fence(l).is_none() && !l.trim_start().starts_with('#')
}

fn breaks(lines: &[&str]) -> Vec<String> {
    let quote = |l: &str| quoted(l).is_some();
    let row = |l: &str| table_cells(l).is_some();
    let item = list_line;
    let plain = plain_line;
    let lead = |l: &str| l.len() - l.trim_start().len();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    // The backticks of the code block the lines are in.
    let mut code: Option<usize> = None;
    // Whether the line is a table's row: under a header and its separator. A line that only
    // looks like one is text.
    let mut table = false;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(ticks) = code {
            // Exactly as written.
            out.push(l.to_string());
            if closes_fence(l, ticks) {
                code = None;
            }
            i += 1;
            continue;
        }
        if let Some((ticks, _)) = fence(l) {
            out.push(l.to_string());
            code = Some(ticks);
            table = false;
            i += 1;
            continue;
        }
        table = row(l) && (table || table_starts(lines[i..].iter().copied()));
        // A quote's lines (as far in as its first) are taken together, broken as what they
        // hold, and put back in the quote.
        let (end, mut piece) = if quote(l) {
            let k = lead(l);
            let end = i + lines[i..].iter().take_while(|x| quote(x) && lead(x) == k).count();
            let inner: Vec<&str> = lines[i..end].iter().filter_map(|x| quoted(x)).collect();
            let pre = &l[..k];
            let piece = breaks(&inner).into_iter().map(|x| if x.is_empty() { format!("{pre}>") } else { format!("{pre}> {x}") }).collect();
            (end, piece)
        } else {
            (i + 1, vec![l.to_string()])
        };
        let last = lines[end - 1];
        if let Some(&next) = lines.get(end) {
            if (plain(last) && plain(next)) || (quote(last) && quote(next)) {
                if let Some(p) = piece.last_mut() {
                    p.push_str("  ");
                }
            } else if ((item(last) || quote(last) || table) && plain(next)) || (plain(last) && table_starts(lines[end..].iter().copied())) {
                piece.push(String::new());
            }
        }
        out.extend(piece);
        i = end;
    }
    out
}

/// A line written escaped because its text only looks like a marker (or a table's first
/// row, `\| a | b |`): that text, with any spaces it starts with.
pub fn unescaped(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix('\\').filter(|r| needs_escape(r) || r.trim_start().starts_with('|'))
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

/// The deepest nesting level of a list item (0 is at the margin): nine levels, as in Word.
/// Markdown deeper than that comes in at this level.
pub const MAX_LIST_LEVEL: usize = 8;

/// A body line as previews show it: list markers as the glyphs the editor draws (• ◦ ▪,
/// ○ ●, 1. a. i.), nested two spaces a level.
pub fn pretty_line(l: &str) -> String {
    let t = l.trim_start();
    let level = ((l.len() - t.len()) / 2).min(MAX_LIST_LEVEL);
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    let pretty = if let Some(r) = unescaped(t) {
        // Text that only looks like a quote or a fence stays escaped, or the preview
        // would draw it as one (`ui::draw_body` shows it unescaped).
        if quoted(r).is_some() || r.starts_with("```") || r.trim_start().starts_with('|') { t.to_string() } else { r.to_string() }
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
    fn hotkeys_default_until_changed() {
        use crate::common::hotkey::{self, Combo};
        // Settings saved before the shortcuts could be changed: every one has its default.
        let d: Data = serde_json::from_str(r#"{"settings":{"edge_left":true,"mcp":true}}"#).unwrap();
        assert!(d.settings.hotkeys.is_empty() && d.settings.edge_left && d.settings.mcp);
        assert!(hotkey::all_default(&d.settings.hotkeys));
        // Left out while they're the defaults, so such files stay as they were.
        assert!(!serde_json::to_string(&d).unwrap().contains("hotkeys"));
        let mut d = d;
        hotkey::set(&mut d.settings.hotkeys, hotkey::NEW_NOTE, Some(Combo { mods: hotkey::WIN | hotkey::SHIFT, key: 0x74 }));
        hotkey::set(&mut d.settings.hotkeys, hotkey::REOPEN, None);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""hotkeys":{"new_note":"Win+Shift+F5","reopen":""}"#), "{json}");
        let back: Data = serde_json::from_str(&json).unwrap();
        let keys = hotkey::resolve(&back.settings.hotkeys);
        assert_eq!(keys[hotkey::NEW_NOTE], Some(Combo { mods: hotkey::WIN | hotkey::SHIFT, key: 0x74 }));
        assert_eq!(keys[hotkey::REOPEN], None);
        assert_eq!(keys[hotkey::ALL_NOTES], Some(Combo::default_for(hotkey::ALL_NOTES)));
        // Not mistaken for a field from a newer version.
        assert!(back.settings.extra.is_empty());
    }


    #[test]
    fn breaks_lines_for_previewers() {
        let md = "# Standup\nYesterday: retries\nToday: search\n- one\n  - nested\n- two\nafter\n\n![a.jpg](attachments/a.jpg)\nAll good\n1. first\n2. second";
        let want = "# Standup\nYesterday: retries  \nToday: search\n- one\n  - nested\n- two\n\nafter\n\n![a.jpg](attachments/a.jpg)  \nAll good\n1. first\n2. second";
        assert_eq!(hard_breaks(md), want);
    }

    /// What a quote holds breaks as it would outside it, and code is left as it is, in a
    /// quote or with its language on the fence.
    #[test]
    fn breaks_inside_quotes_and_code() {
        let md = "> a\n> > b\n> c\n> ```rust\n> x\n> y\n> ```\nafter\n```py\none\ntwo\n```\nend";
        let want = "> a\n> > b\n>\n> c\n> ```rust\n> x\n> y\n> ```\n\nafter\n```py\none\ntwo\n```\nend";
        assert_eq!(hard_breaks(md), want);
        // A longer fence holds a shorter one.
        let md = "````\n```\nkept\n```\n````\nafter";
        assert_eq!(hard_breaks(md), md);
        assert_eq!(fence("```rust"), Some((3, "rust")));
        assert_eq!(fence("```inline```"), None);
        assert!(closes_fence("````", 3) && !closes_fence("```", 4) && !closes_fence("```js", 3));
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
        assert_eq!(title_of("```\n```\nReal title"), "Real title");
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
        // Nested as deep as the editor goes, each level with its mark (• ◦ ▪, 1. a. i.);
        // deeper than that, at the deepest.
        assert_eq!(
            body_of("T\n            - six\n              1. seven\n                - eight\n                  - nine"),
            "            •  six\n              a. seven\n                ▪  eight\n                ▪  nine"
        );
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
        assert!(looks_like_marker("> a") && looks_like_marker("```js") && looks_like_marker(">a"));
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
        // An edit that leaves the note as long as it was is content too.
        let before = s.note(id).unwrap().text.clone();
        s.note_mut(id).unwrap().text = before.replace("charger", "chargeR");
        assert!(!s.only_view_changed(), "same length, other text");
        s.note_mut(id).unwrap().text = before;
        assert!(s.only_view_changed());
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

    /// Notes deleted one after another (the first and last of three, as two deletes in a
    /// row make one Undo) come back each in its place, whatever the order they went in.
    #[test]
    fn deleted_notes_go_back_where_they_were() {
        let notes = |ids: &[u64]| ids.iter().map(|&id| Note { id, ..Default::default() }).collect::<Vec<_>>();
        let ids = |s: &Store| s.d.notes.iter().map(|n| n.id).collect::<Vec<_>>();
        let dir = folder("put-back");
        let mut s = Store::load(dir.clone());
        for order in [[1, 3], [3, 1]] {
            s.d.notes = notes(&[1, 2, 3, 4]);
            let removed: Vec<(usize, Note)> = order.iter().filter_map(|&id| s.remove(id)).collect();
            assert_eq!(ids(&s), [2, 4]);
            s.put_back(removed);
            assert_eq!(ids(&s), [1, 2, 3, 4], "deleted in the order {order:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A temp folder of the test's own, empty.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peekaboo-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A notes file of `n` notes.
    fn notes_json(n: u64) -> String {
        let d = Data { notes: (1..=n).map(|id| Note { id, text: format!("note {id}"), ..Default::default() }).collect(), next_id: n, ..Default::default() };
        serde_json::to_string(&d).unwrap()
    }

    #[test]
    fn a_folder_that_held_notes_isnt_started_over() {
        // A new folder (the log is the only thing in it) starts fresh, and saves.
        let dir = folder("new");
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        let mut s = Store::load(dir.clone());
        assert!(s.fresh && !s.frozen && s.d.notes.is_empty());
        s.create("first", "");
        s.save().unwrap();
        assert!(dir.join("notes.json").exists());
        // Notes were kept here (attachments, versions), but notes.json can't be seen: a
        // drive or share not there yet. Nothing is written.
        for (what, make) in [("attachments", "attachments/a.png"), ("versions", "versions.json")] {
            let dir = folder(what);
            if let Some(parent) = dir.join(make).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(dir.join(make), "x").unwrap();
            let mut s = Store::load(dir.clone());
            assert!(s.frozen, "{what}");
            s.create("typed meanwhile", "");
            assert!(s.save().is_err());
            assert!(!dir.join("notes.json").exists(), "{what}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        // The folder itself not there (a drive letter missing).
        let gone = std::env::temp_dir().join(format!("peekaboo-store-nowhere-{}", std::process::id())).join("PeekABoo Notes");
        let mut s = Store::load(gone.clone());
        assert!(s.frozen && s.save().is_err() && !gone.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session that began without notes.json never writes over one that turns up later
    /// (a sync client bringing the notes down to a new computer).
    #[test]
    fn a_fresh_start_never_writes_over_notes_that_appear() {
        let dir = folder("appear");
        let mut s = Store::load(dir.clone());
        s.create("typed before the sync", "");
        let real = notes_json(5);
        std::fs::write(dir.join("notes.json"), &real).unwrap();
        assert!(s.save().is_err());
        assert_eq!(std::fs::read_to_string(dir.join("notes.json")).unwrap(), real);
        assert!(s.save().is_err(), "and stays so");
        assert_eq!(Store::load(dir.clone()).d.notes.len(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_notes_file_opens_the_backup() {
        let dir = folder("missing");
        std::fs::write(dir.join("notes.backup.json"), notes_json(3)).unwrap();
        let mut s = Store::load(dir.clone());
        assert_eq!(s.d.notes.len(), 3);
        assert!(!s.read_whole && !s.whole(), "files aren't judged by the backup's notes");
        assert_eq!(s.take_notices(), [Notice::Missing]);
        assert!(s.take_notices().is_empty(), "said once");
        let id = s.create("more", "");
        assert!(id > 3);
        s.save().unwrap();
        assert_eq!(Store::load(dir.clone()).d.notes.len(), 4);
        // Only a damaged daily backup is there: the one before it is used.
        let dir2 = folder("missing-daily");
        std::fs::write(dir2.join("notes.backup.json"), "{ damaged").unwrap();
        std::fs::write(dir2.join("notes.backup-2020-01-25.json"), notes_json(2)).unwrap();
        assert_eq!(Store::load(dir2.clone()).d.notes.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn an_unreadable_notes_file_is_set_aside_and_named() {
        let dir = folder("unreadable");
        std::fs::write(dir.join("notes.json"), "{ \"notes\": [ oops").unwrap();
        std::fs::write(dir.join("notes.backup.json"), notes_json(2)).unwrap();
        let mut s = Store::load(dir.clone());
        assert_eq!(s.d.notes.len(), 2);
        let notices = s.take_notices();
        let [Notice::SetAside(name)] = notices.as_slice() else { panic!("{notices:?}") };
        assert!(name.starts_with(UNREADABLE) && dir.join(name).exists());
        assert!(others_kept(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backups_go_back_days_and_never_shrink_far() {
        let dir = folder("backups");
        // Older days' backups, and the one from the last session.
        for day in 20..25 {
            std::fs::write(dir.join(format!("notes.backup-2020-01-{day}.json")), notes_json(10)).unwrap();
        }
        std::fs::write(dir.join("notes.backup.json"), notes_json(10)).unwrap();
        std::fs::write(dir.join("notes.json"), notes_json(9)).unwrap();
        let s = Store::load(dir.clone());
        // The last session's backup became today's; the three newest days stay.
        let t = crate::os::local_time(now());
        let today = format!("notes.backup-{:04}-{:02}-{:02}.json", t.year, t.month, t.day);
        assert_eq!(count_notes(&dir.join(&today)), Some(10));
        let days = backups(&dir, true);
        assert_eq!(days.len(), DAILY_KEEP, "{days:?}");
        assert_eq!(days[0], today);
        assert_eq!(count_notes(&dir.join(BACKUP)), Some(9));
        drop(s);
        // A notes file of far fewer notes (a sync client brought back an old one): the
        // backup of 9 isn't replaced but kept.
        std::fs::write(dir.join("notes.json"), notes_json(2)).unwrap();
        let mut s = Store::load(dir.clone());
        let kept = backups(&dir, false);
        assert_eq!(kept.len(), 1);
        assert_eq!(count_notes(&dir.join(&kept[0])), Some(9));
        assert_eq!(count_notes(&dir.join(BACKUP)), Some(2));
        // In the session too, and the backup is written whole (no temp file left).
        s.d.notes.clear();
        s.backup_at = None;
        s.save().unwrap();
        assert_eq!(count_notes(&dir.join(BACKUP)), Some(0));
        assert!(!dir.join("notes.backup.json.tmp").exists());
        assert_eq!(backups(&dir, false).len(), 1, "2 to 0 isn't far fewer than 4 notes' worth");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Another program or computer wrote notes.json since this session read it: its copy
    /// is kept before this one's notes are saved, and the user told once.
    #[test]
    fn another_writers_notes_are_set_aside() {
        let dir = folder("conflict");
        std::fs::write(dir.join("notes.json"), notes_json(3)).unwrap();
        let mut s = Store::load(dir.clone());
        s.create("mine", "");
        s.save().unwrap();
        // The same bytes written again (a sync client touching it) isn't a change.
        let ours = std::fs::read(dir.join("notes.json")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("notes.json"), &ours).unwrap();
        s.save().unwrap();
        assert!(!others_kept(&dir));
        // Another computer's notes.
        let theirs = notes_json(7);
        std::fs::write(dir.join("notes.json"), &theirs).unwrap();
        s.create("mine too", "");
        s.save().unwrap();
        let notices = s.take_notices();
        let [Notice::Conflict(name)] = notices.as_slice() else { panic!("{notices:?}") };
        assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), theirs);
        assert_eq!(Store::load(dir.clone()).d.notes.len(), 5);
        assert!(others_kept(&dir));
        // Again: kept again under a name of its own, but said only once.
        std::fs::write(dir.join("notes.json"), notes_json(1)).unwrap();
        s.save().unwrap();
        assert!(s.take_notices().is_empty());
        let conflicts = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(CONFLICT)).count();
        assert_eq!(conflicts, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    // Read-only and back, on Windows: the file's attribute, nothing world-writable.
    #[allow(clippy::permissions_set_readonly_false)]
    fn a_failed_save_leaves_no_temp_file() {
        let dir = folder("failed");
        let mut s = Store::load(dir.clone());
        s.create("a", "");
        s.save().unwrap();
        let notes = dir.join("notes.json");
        let mut p = std::fs::metadata(&notes).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&notes, p.clone()).unwrap();
        s.create("b", "");
        let failed = s.save();
        p.set_readonly(false);
        std::fs::set_permissions(&notes, p).unwrap();
        assert!(failed.is_err());
        assert!(!dir.join("notes.json.tmp").exists());
        // And one left from before goes at the next start.
        std::fs::write(dir.join("notes.json.tmp"), "half").unwrap();
        drop(Store::load(dir.clone()));
        assert!(!dir.join("notes.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file from a newer version keeps what this one doesn't know through a save, and
    /// one of a newer form isn't written at all.
    #[test]
    fn newer_files_keep_what_this_version_doesnt_know() {
        let dir = folder("newer");
        let file = r#"{"version":1,"next_id":2,"notes":[{"id":2,"text":"a","tags":["work"],"rect":[1,2,300,400]}],"settings":{"theme":2,"accent":"teal"},"sync":{"peer":"x"}}"#;
        std::fs::write(dir.join("notes.json"), file).unwrap();
        let mut s = Store::load(dir.clone());
        assert!(s.whole());
        s.note_mut(2).unwrap().text = "b".into();
        s.save().unwrap();
        let back: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("notes.json")).unwrap()).unwrap();
        assert_eq!(back["version"], FORMAT);
        assert_eq!(back["notes"][0]["tags"][0], "work");
        assert_eq!(back["notes"][0]["text"], "b");
        assert_eq!(back["notes"][0]["rect"][2], 300);
        assert_eq!(back["settings"]["accent"], "teal");
        assert_eq!(back["settings"]["theme"], 2);
        assert_eq!(back["sync"]["peer"], "x");
        // A later form: read, never written.
        let later = file.replace(r#""version":1"#, &format!(r#""version":{}"#, FORMAT + 1));
        std::fs::write(dir.join("notes.json"), &later).unwrap();
        let mut s = Store::load(dir.clone());
        assert_eq!(s.d.notes.len(), 1);
        assert!(!s.whole());
        assert!(s.save().is_err());
        assert_eq!(std::fs::read_to_string(dir.join("notes.json")).unwrap(), later);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprints_dont_depend_on_how_bytes_come() {
        let bytes: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let whole = Fingerprint::of(&bytes);
        let mut f = Fingerprint::default();
        for piece in bytes.chunks(3) {
            f.update(piece);
        }
        assert_eq!(f.finish(), whole);
        assert_ne!(Fingerprint::of(&bytes[..999]), whole);
        assert_ne!(Fingerprint::of(b"ab"), Fingerprint::of(b"ba"));
    }

    #[test]
    fn searches_block_text() {
        let text = "Title\n> quoted\n```\n- code\n```\n| A | B |\n| --- | --- |\n| x<br>y | \\| z |";
        assert_eq!(search_rest(text), "quoted\n- code\nA B\nx y | z");
        assert_eq!(search_rest("Plain\n- item\n1. one"), "item\none");
    }
}
