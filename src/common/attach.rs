// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files dropped into notes. Each is copied into the `attachments` folder beside the
//! notes, and a note points at it as `attachment:<name>`: relative, so a portable copy
//! (or a moved data folder) keeps working.

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

/// Copy `src` into the attachments folder; returns the name it got there.
pub fn copy_in(src: &Path) -> std::io::Result<String> {
    let dir = dir();
    std::fs::create_dir_all(&dir)?;
    let name = src.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_string();
    let name = free_name(&dir, &name);
    std::fs::copy(src, dir.join(&name))?;
    Ok(name)
}

/// "attachment:Q3%20plan.pdf" for "Q3 plan.pdf": no spaces or quotes, which would end
/// the address inside a link.
pub fn url(name: &str) -> String {
    let mut out = String::from(SCHEME);
    for b in name.bytes() {
        match b {
            b' ' | b'"' | b'%' | b'#' | b'?' | b'\\' | 0..0x20 | 0x7F.. => out.push_str(&format!("%{b:02X}")),
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
