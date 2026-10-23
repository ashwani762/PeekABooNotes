// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The app log: `logs\peekaboo.log` in the data folder, one timestamped line per event.
//!
//! Only what helps with a bug report goes in: ids, counts, sizes, timings and errors.
//! Never note text, titles or file contents. Each line is opened, written and closed on
//! the spot: events are rare, nothing is held open while idle, and no buffer can lose
//! the last lines before a crash.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;


use crate::common::util::{data_dir, portable};
use crate::os;

/// Past this the log becomes `peekaboo.1.log` (replacing the previous one) and starts over.
const CAP: u64 = 512 * 1024;
/// Present while the app runs; still there at the next start means it didn't exit cleanly.
const MARKER: &str = "running";

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

/// Developer detail, written only with PEEKABOO_TRACE set.
pub fn debug(msg: &str) {
    if debug_enabled() {
        write("DEBUG", msg);
    }
}

pub fn debug_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PEEKABOO_TRACE").is_some())
}

/// `logs\` in the data folder (crash dumps go here too).
pub fn dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = data_dir().join("logs");
        let _ = std::fs::create_dir_all(&d);
        d
    })
    .clone()
}

/// Local time with milliseconds: "2026-10-22 14:03:07.412".
pub fn timestamp() -> String {
    let t = os::local_now();
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", t.year, t.month, t.day, t.hour, t.minute, t.second, t.millis)
}

fn write(level: &str, msg: &str) {
    // Continuation lines (a crash report's stack) are indented under their entry.
    let line = format!("{} {level:<5} {}\n", timestamp(), msg.trim_end().replace('\n', "\n    "));
    let _ = append(&dir().join("peekaboo.log"), &line, CAP);
}

fn append(path: &Path, text: &str, cap: u64) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= cap) {
        // Replaces the older file, so the log never takes more than twice the cap.
        let _ = std::fs::rename(path, path.with_extension("1.log"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(text.as_bytes())
}

/// The first lines of a session: what's running where, and whether the last session
/// ended cleanly.
pub fn startup() {
    START.get_or_init(Instant::now);
    info(&format!(
        "start: PeekABoo Notes {}, {}, {}, scale {}%, data {}",
        env!("CARGO_PKG_VERSION"),
        os::system_description(),
        if portable() { "portable" } else { "installed" },
        os::display_scale(),
        data_dir().display(),
    ));
    let marker = dir().join(MARKER);
    if marker.exists() {
        warn("the previous session didn't exit cleanly (crash, forced close or power loss)");
    }
    let _ = std::fs::write(&marker, b"");
}

static START: OnceLock<Instant> = OnceLock::new();

/// The last line of a clean exit; `how` says what ended it.
pub fn shutdown(how: &str) {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let up = START.get().map_or(0, |t| t.elapsed().as_secs());
    info(&format!("exit ({how}) after {}h {:02}m {:02}s", up / 3600, up / 60 % 60, up % 60));
    let _ = std::fs::remove_file(dir().join(MARKER));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_past_the_cap_keeping_one_old_file() {
        let dir = std::env::temp_dir().join(format!("peekaboo-log-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("peekaboo.log");
        let line = "x".repeat(99) + "\n";
        for _ in 0..25 {
            append(&log, &line, 1000).unwrap();
        }
        // 25 lines of 100 bytes with a 1000-byte cap: rotated after lines 10 and 20.
        assert_eq!(std::fs::metadata(&log).unwrap().len(), 500);
        assert_eq!(std::fs::metadata(dir.join("peekaboo.1.log")).unwrap().len(), 1000);
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
