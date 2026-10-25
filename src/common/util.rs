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
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
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
    if out.is_empty() {
        return crate::t!("file_untitled").into();
    }
    if reserved_device(&out) {
        out.insert(0, '_');
    }
    out
}

/// Windows opens a device, not a file, for a name whose part before the first dot is CON,
/// PRN, AUX, NUL, COM0-9 or LPT0-9 (any case, trailing spaces ignored), so "Con.md" or
/// "nul" as a note's title would export into nothing.
fn reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        s => {
            let port = s.strip_prefix("COM").or_else(|| s.strip_prefix("LPT"));
            port.is_some_and(|n| matches!(n, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
        }
    }
}

/// A text file's bytes as text, by its byte order mark: UTF-16 LE or BE (what Notepad's
/// "Unicode" saves), else UTF-8 with or without one. Bytes that don't decode become U+FFFD
/// rather than failing the file.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let (pairs, odd) = rest.as_chunks::<2>();
        let cut = !odd.is_empty();
        let units: Vec<u16> = pairs.iter().map(|&c| unit(c)).collect();
        let mut s = String::from_utf16_lossy(&units);
        if cut {
            s.push(char::REPLACEMENT_CHARACTER);
        }
        s
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_files_decode_by_their_mark() {
        let text = "Café ☑ 写真\r\n- one";
        let mut le = vec![0xFF, 0xFE];
        le.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(text.as_bytes());
        for bytes in [le, be, bom, text.as_bytes().to_vec()] {
            assert_eq!(decode_text(&bytes), text);
        }
        // A cut-off last unit or a stray byte comes through as a replacement, not an error.
        assert_eq!(decode_text(&[0xFF, 0xFE, b'a', 0, b'b']), "a\u{FFFD}");
        assert_eq!(decode_text(b"a\xFFb"), "a\u{FFFD}b");
        assert_eq!(decode_text(&[]), "");
    }

    #[test]
    fn device_names_are_kept_off() {
        for name in ["CON", "con", "Prn", "aux", "NUL", "COM1", "com9", "LPT1", "lpt9", "con.md", "Nul.txt", "CON .md", "COM¹"] {
            assert_eq!(file_safe(name), format!("_{name}"), "{name}");
        }
        for name in ["Console", "CON notes", "COM10", "LPT", "Auxiliary.md", "Standup - Tue", "nul-hypothesis"] {
            assert_eq!(file_safe(name), name, "{name}");
        }
        assert_eq!(file_safe("nul.: x"), "_nul. - x");
    }
}
