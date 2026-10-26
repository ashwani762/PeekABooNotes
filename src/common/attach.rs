// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files dropped into notes. Each is copied into the `attachments` folder beside the
//! notes, and a note points at it as `attachment:<name>`: relative, so a portable copy
//! (or a moved data folder) keeps working.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const SCHEME: &str = "attachment:";

pub fn dir() -> PathBuf {
    crate::common::util::data_dir().join("attachments")
}

/// Pictures a note shows inline (everything else is a chip).
pub fn is_image(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "heic")
}

/// A name not yet taken in `dir`: "plan.pdf", then "plan 2.pdf", "plan 3.pdf".
pub fn free_name(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() {
        return name.to_string();
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    // "Plan (2).txt" taken goes on to "Plan (3).txt", not "Plan (2) (2).txt", as Windows names copies.
    let stem = stem
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .filter(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .map_or(stem, |(base, _)| base);
    (2..).map(|n| format!("{stem} ({n}){ext}")).find(|n| !dir.join(n).exists()).unwrap()
}

/// The largest file copied into a note: past this it's refused (a link to a disk image or a
/// film shouldn't fill the notes' folder, or hold the app up while it copies).
pub const MAX_FILE_BYTES: u64 = 100 << 20;

/// Copy `src` into the attachments folder `dir`; returns the name it got there.
fn copy_into(dir: &Path, src: &Path) -> std::io::Result<String> {
    if std::fs::metadata(src)?.len() > MAX_FILE_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::FileTooLarge, "larger than a note takes"));
    }
    std::fs::create_dir_all(dir)?;
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let name = free_name(dir, &name);
    std::fs::copy(src, dir.join(&name))?;
    Ok(name)
}

/// The files copied in by one operation (a drop, an import, an assistant's note), so a file
/// it names twice is copied once and both point at the one copy.
#[derive(Default)]
pub struct Copies {
    done: Vec<(PathBuf, String)>,
    /// A file the folder holds already under the same name, with the same bytes, is taken
    /// as it is rather than copied again (`reusing`).
    reuse: bool,
}

impl Copies {
    /// For an import, whose notes are saved as they come in: a file that's already in the
    /// attachments folder, by its name and bytes (notes exported from here and brought
    /// back), is linked there instead of copied in again as "photo (2).jpg". Not for a
    /// drop into an open note, which links what it copies in only through what's saved
    /// later: there, a file that a note deleted a moment ago links could go with it.
    pub fn reusing() -> Copies {
        Copies { done: Vec::new(), reuse: true }
    }

    /// Copy `src` into the attachments folder, unless this operation copied it in already:
    /// the name it got there.
    pub fn copy_in(&mut self, src: &Path) -> std::io::Result<String> {
        self.copy_into(&dir(), src)
    }

    fn copy_into(&mut self, dir: &Path, src: &Path) -> std::io::Result<String> {
        let key = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
        if let Some((_, name)) = self.done.iter().find(|(p, _)| *p == key) {
            return Ok(name.clone());
        }
        let there = src.file_name().and_then(|n| n.to_str()).filter(|n| self.reuse && same_content(src, &dir.join(n))).map(str::to_string);
        let name = match there {
            Some(name) => name,
            None => copy_into(dir, src)?,
        };
        self.done.push((key, name.clone()));
        Ok(name)
    }
}

/// Whether the files at `a` and `b` hold the same bytes, read a piece at a time.
pub fn same_content(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let Ok(n) = fa.read(&mut ba) else { return false };
        if n == 0 {
            // Both at their ends (of the same length).
            return fb.read(&mut bb[..1]).is_ok_and(|m| m == 0);
        }
        if fb.read_exact(&mut bb[..n]).is_err() || ba[..n] != bb[..n] {
            return false;
        }
    }
}

/// The start-up sweep's files go into `attachments\.trash\<day>\` rather than away, and
/// stay `TRASH_DAYS` there: a file another computer's notes still link (synced in, and
/// not seen here yet) can still be brought back.
pub const TRASH: &str = ".trash";
pub const TRASH_DAYS: i64 = 30;

/// Move file `name` in `dir` into the trash folder for `day` ("2026-10-26"), under a name
/// not taken there.
pub fn to_trash(dir: &Path, name: &str, day: &str) -> std::io::Result<()> {
    let t = dir.join(TRASH).join(day);
    std::fs::create_dir_all(&t)?;
    let to = free_name(&t, name);
    std::fs::rename(dir.join(name), t.join(to))
}

/// The trash's days (folders named "2026-10-26") more than `TRASH_DAYS` before `today`
/// (days since 1970) go, with what's in them. Others (a day ahead of the clock, a folder
/// by another name) stay. How many went.
pub fn purge_trash(dir: &Path, today: i64) -> usize {
    let Ok(days) = std::fs::read_dir(dir.join(TRASH)) else { return 0 };
    let mut gone = 0;
    for e in days.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let parts: Vec<i64> = name.split('-').filter_map(|p| p.parse().ok()).collect();
        let [y, m, d] = parts[..] else { continue };
        if today - crate::common::util::days_from_civil(y, m, d) > TRASH_DAYS && e.file_type().is_ok_and(|t| t.is_dir()) && std::fs::remove_dir_all(e.path()).is_ok() {
            gone += 1;
        }
    }
    gone
}

/// "attachment:Q3%20plan.pdf" for "Q3 plan.pdf": no spaces or quotes, which would end
/// the address inside a link, nor brackets: a Markdown link ends at ")", and "plan (2).pdf"
/// was cut at its own (export left the file behind, import made a dead link). Notes saved
/// before keep "(2)" as it was; `decode` reads both.
pub fn url(name: &str) -> String {
    let mut out = String::from(SCHEME);
    for b in name.bytes() {
        match b {
            b' ' | b'"' | b'%' | b'#' | b'?' | b'(' | b')' | b'\\' | 0..0x20 | 0x7F.. => out.push_str(&format!("%{b:02X}")),
            _ => out.push(b as char),
        }
    }
    out
}

/// "%20" and friends back to what they stand for.
pub fn decode(s: &str) -> Option<String> {
    let enc = s.as_bytes();
    let mut bytes = Vec::with_capacity(enc.len());
    let mut i = 0;
    while i < enc.len() {
        if enc[i] == b'%'
            && let Some(v) = enc.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            bytes.push(v);
            i += 3;
            continue;
        }
        bytes.push(enc[i]);
        i += 1;
    }
    String::from_utf8(bytes).ok()
}

/// The attachment an address names, if it names one.
pub fn name_of(url: &str) -> Option<String> {
    let name = decode(url.strip_prefix(SCHEME)?)?;
    // Only a plain file name in the folder: never a way out of it.
    (!name.is_empty() && !name.contains(['/', '\\']) && name != "..").then_some(name)
}

/// The file an attachment address names.
pub fn path_of(url: &str) -> Option<PathBuf> {
    name_of(url).map(|n| dir().join(n))
}

/// The attachments `text` links to, by name: a note's RTF (`HYPERLINK "attachment:..."`),
/// its light text (`](attachment:...)`), or Markdown as export and the saved versions
/// write it (`](attachments/...)`).
pub fn linked(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    // RTF may break a long line anywhere, also inside an address, and its line breaks
    // mean nothing: read it joined up as well as it stands.
    let joined = if text.starts_with("{\\rtf") { text.replace(['\r', '\n'], "") } else { String::new() };
    for s in [text, joined.as_str()] {
        for prefix in [SCHEME, "attachments/"] {
            for (i, _) in s.match_indices(prefix) {
                let rest = &s[i + prefix.len()..];
                let end = rest.find(|c: char| c.is_whitespace() || "\"'<>[]{}|\\".contains(c)).unwrap_or(rest.len());
                let mut token = &rest[..end];
                // A Markdown link's closing bracket, not one of the name's ("plan (2).pdf").
                while token.ends_with(')') && token.matches(')').count() > token.matches('(').count() {
                    token = &token[..token.len() - 1];
                }
                if let Some(name) = name_of(&format!("{SCHEME}{token}")) {
                    names.insert(name);
                }
            }
        }
    }
    names
}

/// Of the attachments `gone` (what notes deleted for good linked to), those nothing in
/// `kept` (the other notes' RTF and text, the saved versions) still links to: the files
/// that can go. A name kept anywhere in any form stays, so this errs on keeping a file.
/// Names are compared without regard to case, as Windows finds files: a link spelt
/// "Photo.PNG" opens "photo.png".
pub fn orphans<'a, K>(gone: &BTreeSet<String>, kept: K) -> Vec<String>
where
    K: IntoIterator<Item = &'a str>,
    K::IntoIter: Clone,
{
    let mut left: Vec<&String> = gone.iter().collect();
    let kept = kept.into_iter();
    // First the names each text links: most files are found there, in one pass.
    for k in kept.clone() {
        if left.is_empty() {
            break;
        }
        let named: BTreeSet<String> = linked(k).into_iter().map(|n| n.to_lowercase()).collect();
        left.retain(|n| !named.contains(&n.to_lowercase()));
    }
    // Then, for the few left, their address anywhere at all: a name the link reader
    // stops short in ("a{b}.png", which RTF writes "a\{b\}.png") is still seen.
    let forms: Vec<[String; 4]> = left
        .iter()
        .map(|n| {
            let u = url(n).to_lowercase();
            // Also as notes saved before brackets were escaped have it.
            let old = u.replace("%28", "(").replace("%29", ")");
            [u.replace('{', "\\{").replace('}', "\\}"), export_link(n).to_lowercase(), u, old]
        })
        .collect();
    let mut left: Vec<(&String, [String; 4])> = left.into_iter().zip(forms).collect();
    for k in kept {
        if left.is_empty() {
            break;
        }
        let joined = k.replace(['\r', '\n'], "").to_lowercase();
        left.retain(|(_, forms)| !forms.iter().any(|f| joined.contains(f.as_str())));
    }
    left.into_iter().map(|(n, _)| n.clone()).collect()
}

/// How long a file sits in the folder before the start-up sweep may take it: one younger
/// may be on its way into a note right now.
pub const SWEEP_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Whether a file put in the folder at `created` and last changed at `modified` is old
/// enough for the sweep at `now`. It must also be older than the notes' last save
/// (`notes_saved`): a file that came after it may belong to notes this copy hasn't seen
/// yet, as when the folder syncs from another computer. A time unknown, or ahead of the
/// clock, keeps the file.
pub fn sweepable(created: Option<std::time::SystemTime>, modified: Option<std::time::SystemTime>, notes_saved: Option<std::time::SystemTime>, now: std::time::SystemTime) -> bool {
    let (Some(c), Some(m), Some(saved)) = (created, modified, notes_saved) else { return false };
    let old = |t: std::time::SystemTime| now.duration_since(t).is_ok_and(|age| age >= SWEEP_AGE) && t <= saved;
    old(c) && old(m)
}

/// The start-up sweep: of `old` (the files old enough, by `sweepable`), those nothing in
/// `kept` links (as `orphans`), out of `in_folder` files in all. None when that would be
/// every file in the folder: that looks like notes read from somewhere else, not files
/// left behind, and keeping them costs only disk space.
pub fn sweep<'a, K>(old: &BTreeSet<String>, in_folder: usize, kept: K) -> Vec<String>
where
    K: IntoIterator<Item = &'a str>,
    K::IntoIter: Clone,
{
    let gone = orphans(old, kept);
    if gone.len() >= in_folder { Vec::new() } else { gone }
}

/// Where export puts attachments, beside the Markdown files.
pub const EXPORT_DIR: &str = "attachments";

/// How exported Markdown points at an attachment: "attachments/Q3%20plan.pdf".
pub fn export_link(name: &str) -> String {
    format!("{EXPORT_DIR}/{}", &url(name)[SCHEME.len()..])
}

/// The local file a Markdown link points at, if it is one that exists: a file:// URL,
/// or a path relative to the Markdown file's folder `base`. Web addresses are not.
pub fn local_file(link: &str, base: &Path) -> Option<PathBuf> {
    let link = link.trim();
    let lower = link.to_ascii_lowercase();
    let path = if let Some(rest) = lower.strip_prefix("file:///").map(|_| &link[8..]) {
        PathBuf::from(decode(rest)?.replace('/', "\\"))
    } else if lower.contains("://") || lower.starts_with("mailto:") || lower.starts_with(SCHEME) {
        return None;
    } else {
        let rel = decode(link)?;
        let p = Path::new(&rel);
        if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
    };
    path.is_file().then_some(path)
}

/// Which files a link may bring in: those under one of `roots`, and none under one of
/// `except` (the notes' own folder holds notes.json). Both sides fully resolved first, so
/// `..`, links and network paths can't reach outside.
pub struct Allowed {
    pub roots: Vec<PathBuf>,
    pub except: Vec<PathBuf>,
}

impl Allowed {
    pub fn admits(&self, file: &Path) -> bool {
        let Ok(f) = std::fs::canonicalize(file) else { return false };
        let under = |dir: &PathBuf| std::fs::canonicalize(dir).is_ok_and(|d| f.starts_with(d));
        self.roots.iter().any(under) && !self.except.iter().any(under)
    }
}

/// "file:///C:/Users/me/Documents/PeekABoo%20Notes/attachments/plan.pdf".
#[cfg(test)]
fn file_url(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file:///");
    for b in s.bytes() {
        match b {
            b' ' | b'"' | b'%' | b'#' | b'?' | 0..0x20 | 0x7F.. => out.push_str(&format!("%{b:02X}")),
            _ => out.push(b as char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name with brackets: the link holds them escaped, so a Markdown link around it ends
    /// at its own ")", and links saved before (brackets as they are) still count.
    #[test]
    fn brackets_in_names_stay_in_the_link() {
        assert_eq!(export_link("Lisbon (2).jpg"), "attachments/Lisbon%20%282%29.jpg");
        assert_eq!(name_of(&url("Lisbon (2).jpg")).as_deref(), Some("Lisbon (2).jpg"));
        let md = format!("![Lisbon (2).jpg]({}) and more", export_link("Lisbon (2).jpg"));
        assert!(linked(&md).contains("Lisbon (2).jpg"));
        let old = "{\\rtf1 HYPERLINK \"attachment:Lisbon%20(2).jpg\"}";
        assert!(linked(old).contains("Lisbon (2).jpg"));
        let gone: BTreeSet<String> = ["Lisbon (2).jpg".to_string()].into();
        assert!(orphans(&gone, [old]).is_empty(), "an old-style link still keeps its file");
    }

    #[test]
    fn addresses_round_trip() {
        for name in ["plan.pdf", "Q3 plan (final).pdf", "café notes.txt", "a%b#c?.md", "写真.jpg"] {
            let u = url(name);
            assert!(!u.contains(' ') && !u.contains('"'), "{u}");
            assert_eq!(name_of(&u).as_deref(), Some(name));
        }
        assert_eq!(name_of("attachment:..%2Fsecret"), None);
        assert_eq!(name_of("attachment:a%5Cb"), None);
        assert_eq!(name_of("https://x.org"), None);
    }

    #[test]
    fn only_files_nothing_links_are_orphans() {
        let rtf = format!(r#"{{\rtf1 {{\field{{\*\fldinst{{HYPERLINK "{}"}}}}{{\fldrslt plan}}}} and {{\field{{\*\fldinst{{HYPERLINK "{}"}}}}{{\fldrslt x}}}}}}"#, url("Q3 plan (2).pdf"), url("sunset.jpg"));
        let text = format!("Trip\n[Q3 plan (2).pdf]({})\nSee [chart.png]({}) first", url("Q3 plan (2).pdf"), url("chart.png"));
        let gone: BTreeSet<String> = linked(&rtf).into_iter().chain(linked(&text)).collect();
        assert_eq!(gone.iter().map(String::as_str).collect::<Vec<_>>(), ["Q3 plan (2).pdf", "chart.png", "sunset.jpg"]);
        // An address broken across RTF lines is still read whole.
        assert!(linked("{\\rtf1 HYPERLINK \"attachment:long%20na\r\nme.pdf\"}").contains("long name.pdf"));
        // Not a way out of the folder.
        assert!(linked("](attachment:..%2Fnotes.json)").is_empty());

        // Another note's RTF still shows the picture; a saved version (Markdown, as export
        // writes it) still has the chart. Only the plan can go.
        let other = format!(r#"{{\rtf1 {{\field{{\*\fldinst{{HYPERLINK "{}"}}}}{{\fldrslt x}}}}}}"#, url("sunset.jpg"));
        let version = format!("Old\n![chart.png]({})", export_link("chart.png"));
        assert_eq!(orphans(&gone, [other.as_str(), version.as_str(), "unrelated"]), ["Q3 plan (2).pdf"]);
        assert_eq!(orphans(&gone, []).len(), 3, "nothing kept: all go");
        assert!(orphans(&BTreeSet::new(), [other.as_str()]).is_empty());
    }

    /// A file is kept when a link names it in another case, or when the link reader
    /// can't read its name whole but its address is there.
    #[test]
    fn orphans_err_on_keeping() {
        let gone: BTreeSet<String> = ["photo.png", "a{b}.png", "c[1].pdf", "lost.txt"].into_iter().map(String::from).collect();
        let rtf = format!(r#"{{\rtf1 {{\field{{\*\fldinst{{HYPERLINK "{}"}}}}{{\fldrslt x}}}} {{\field{{\*\fldinst{{HYPERLINK "{}"}}}}{{\fldrslt y}}}}}}"#, url("Photo.PNG"), url("a{b}.png").replace('{', "\\{").replace('}', "\\}"));
        let text = format!("[c[1].pdf]({})", url("c[1].pdf"));
        assert_eq!(orphans(&gone, [rtf.as_str(), text.as_str()]), ["lost.txt"]);
    }

    #[test]
    fn the_sweep_takes_only_old_unlinked_files() {
        use std::time::{Duration, SystemTime};
        let now = SystemTime::now();
        let ago = |h: u64| Some(now - Duration::from_secs(h * 3600));
        // Old, and older than the notes' last save.
        assert!(sweepable(ago(48), ago(30), ago(1), now));
        // Just put there (a copy keeps its source's modified time, not its created time).
        assert!(!sweepable(ago(1), ago(500), ago(0), now));
        assert!(!sweepable(ago(48), ago(2), ago(0), now));
        // Come since the notes were last saved: it may belong to notes not seen yet.
        assert!(!sweepable(ago(48), ago(48), ago(72), now));
        // Unknown times, or ahead of the clock.
        assert!(!sweepable(None, ago(48), ago(0), now));
        assert!(!sweepable(ago(48), ago(48), None, now));
        assert!(!sweepable(Some(now + Duration::from_secs(3600)), ago(48), ago(0), now));

        let old: BTreeSet<String> = ["kept.png", "left.pdf"].into_iter().map(String::from).collect();
        let note = format!("![kept.png]({})", url("kept.png"));
        assert_eq!(sweep(&old, 3, [note.as_str()]), ["left.pdf"]);
        // Never every file in the folder at once.
        assert!(sweep(&old, 2, ["no links here"]).is_empty());
        assert_eq!(sweep(&old, 3, ["no links here"]).len(), 2);
    }

    #[test]
    fn only_allowed_folders_are_admitted() {
        let dir = std::env::temp_dir().join(format!("peekaboo-allowed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["ok", "ok/notes", "ok/notes/attachments", "secret"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in ["ok/a.png", "ok/notes/notes.json", "ok/notes/attachments/b.png", "secret/key.txt"] {
            std::fs::write(dir.join(f), "x").unwrap();
        }
        let allowed = Allowed { roots: vec![dir.join("ok"), dir.join("ok/notes/attachments")], except: vec![dir.join("ok/notes")] };
        assert!(allowed.admits(&dir.join("ok/a.png")));
        assert!(!allowed.admits(&dir.join("secret/key.txt")), "outside every root");
        assert!(!allowed.admits(&dir.join("ok/../secret/key.txt")), "not by going up");
        assert!(!allowed.admits(&dir.join("ok/notes/notes.json")), "not the notes themselves");
        assert!(!allowed.admits(&dir.join("ok/missing.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_links_resolve() {
        let dir = std::env::temp_dir().join("peekaboo-attach-links");
        let _ = std::fs::create_dir_all(dir.join(EXPORT_DIR));
        std::fs::write(dir.join(EXPORT_DIR).join("Q3 plan.txt"), "x").unwrap();
        let link = export_link("Q3 plan.txt");
        assert_eq!(link, "attachments/Q3%20plan.txt");
        assert_eq!(local_file(&link, &dir), Some(dir.join("attachments").join("Q3 plan.txt")));
        assert_eq!(local_file(&file_url(&dir.join("attachments").join("Q3 plan.txt")), Path::new("C:\\elsewhere")), Some(dir.join("attachments").join("Q3 plan.txt")));
        assert_eq!(local_file("https://example.com/a.txt", &dir), None);
        assert_eq!(local_file("attachments/missing.txt", &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copies_are_capped_and_made_once() {
        let dir = std::env::temp_dir().join(format!("peekaboo-attach-copies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let into = dir.join("attachments");
        std::fs::write(dir.join("src").join("chart.png"), "png").unwrap();
        // Named twice by one note (by two spellings of its path): one copy.
        let mut copies = Copies::default();
        let a = copies.copy_into(&into, &dir.join("src").join("chart.png")).unwrap();
        let b = copies.copy_into(&into, &dir.join("src").join("..").join("src").join("chart.png")).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("chart.png", "chart.png"));
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 1);
        // A new operation copies it again, under a free name.
        assert_eq!(Copies::default().copy_into(&into, &dir.join("src").join("chart.png")).unwrap(), "chart (2).png");
        // An import takes the one there, by its name and bytes; another file of the name
        // is copied in beside it.
        assert_eq!(Copies::reusing().copy_into(&into, &dir.join("src").join("chart.png")).unwrap(), "chart.png");
        assert_eq!(std::fs::read_dir(&into).unwrap().count(), 2);
        std::fs::create_dir_all(dir.join("other")).unwrap();
        std::fs::write(dir.join("other").join("chart.png"), "another png").unwrap();
        assert_eq!(Copies::reusing().copy_into(&into, &dir.join("other").join("chart.png")).unwrap(), "chart (3).png");
        // Too big: refused before anything is read or made.
        let big = dir.join("src").join("disk.img");
        std::fs::File::create(&big).unwrap().set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(copy_into(&into, &big).unwrap_err().kind(), std::io::ErrorKind::FileTooLarge);
        assert!(!into.join("disk.img").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_content_is_by_bytes() {
        let dir = std::env::temp_dir().join(format!("peekaboo-attach-same-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let mut other = big.clone();
        other[150_000] ^= 1;
        std::fs::write(dir.join("a"), &big).unwrap();
        std::fs::write(dir.join("b"), &big).unwrap();
        std::fs::write(dir.join("c"), &other).unwrap();
        std::fs::write(dir.join("d"), &big[..1000]).unwrap();
        assert!(same_content(&dir.join("a"), &dir.join("b")));
        assert!(!same_content(&dir.join("a"), &dir.join("c")), "same size, different bytes");
        assert!(!same_content(&dir.join("a"), &dir.join("d")));
        assert!(!same_content(&dir.join("a"), &dir.join("missing")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_trash_keeps_a_month() {
        let dir = std::env::temp_dir().join(format!("peekaboo-attach-trash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["a.png", "b.png"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        to_trash(&dir, "a.png", "2026-09-01").unwrap();
        std::fs::write(dir.join("a.png"), "again").unwrap();
        to_trash(&dir, "a.png", "2026-09-01").unwrap();
        to_trash(&dir, "b.png", "2026-10-20").unwrap();
        assert!(!dir.join("a.png").exists() && !dir.join("b.png").exists());
        let day = dir.join(TRASH).join("2026-09-01");
        assert_eq!(std::fs::read_to_string(day.join("a.png")).unwrap(), "a.png");
        assert_eq!(std::fs::read_to_string(day.join("a (2).png")).unwrap(), "again");
        std::fs::create_dir_all(dir.join(TRASH).join("not a day")).unwrap();
        let today = crate::common::util::days_from_civil(2026, 10, 26);
        assert_eq!(purge_trash(&dir, today), 1);
        assert!(!day.exists(), "past 30 days");
        assert!(dir.join(TRASH).join("2026-10-20").join("b.png").exists());
        assert!(dir.join(TRASH).join("not a day").exists());
        // A clock gone back keeps a day "ahead" of it.
        assert_eq!(purge_trash(&dir, crate::common::util::days_from_civil(2020, 1, 1)), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_and_kinds() {
        let dir = std::env::temp_dir().join("peekaboo-attach-test");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        std::fs::write(dir.join("a (2).txt"), "x").unwrap();
        std::fs::write(dir.join("Q3 plan 2.txt"), "x").unwrap();
        assert_eq!(free_name(&dir, "a.txt"), "a (3).txt");
        assert_eq!(free_name(&dir, "a (2).txt"), "a (3).txt");
        assert_eq!(free_name(&dir, "Q3 plan 2.txt"), "Q3 plan 2 (2).txt");
        assert_eq!(free_name(&dir, "b.txt"), "b.txt");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(is_image("Photo.JPG") && !is_image("plan.pdf"));
        assert_eq!(file_url(Path::new(r"C:\My Notes\a.pdf")), "file:///C:/My%20Notes/a.pdf");
    }
}
