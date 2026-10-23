// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small portable helpers: colours, the clock, where the data lives, file names.
//! (Dates and times for display are formatted per language in `i18n::time`.)

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Linear mix of two 0xRRGGBB colours; t = 0 -> a, t = 1 -> b.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let m = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32).min(255) << s;
    m(16) | m(8) | m(0)
}

/// A moment on the local wall clock (from `crate::os`, which knows the time zone).
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("peekaboo-notes.exe"))
}

/// Notes live in `Documents\PeekABoo Notes`, unless a file named `portable` sits next to the
/// exe, in which case everything stays beside it in `PeekABooData` (carry it on a USB
/// stick).
pub fn data_dir() -> PathBuf {
    // Worked out (and created) once; it's asked for on every save and some paints.
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = if portable() { exe_dir().join("PeekABooData") } else { crate::os::documents_dir().join("PeekABoo Notes") };
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

fn exe_dir() -> PathBuf {
    exe_path().parent().map(PathBuf::from).unwrap_or_default()
}

/// A file named `portable` beside the exe keeps the data beside it too.
pub fn portable() -> bool {
    exe_dir().join("portable").exists()
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Day number of a local date, for "today" / "yesterday" comparisons.
pub fn local_day(t: &LocalTime) -> i64 {
    days_from_civil(t.year as i64, t.month as i64, t.day as i64)
}

pub fn stamp() -> String {
    let t = crate::os::local_time(now());
    format!("{:04}-{:02}-{:02} {:02}.{:02}.{:02}", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// Make a string safe to use as a file name.
pub fn file_safe(s: &str) -> String {
    // "Standup: Tue" reads best as "Standup - Tue", "1:1" as "1-1".
    let s = s.replace(": ", " - ");
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '-' } else { c })
        .take(60)
        .collect();
    out = out.trim().trim_end_matches('.').to_string();
    if out.is_empty() { crate::t!("file_untitled").into() } else { out }
}
