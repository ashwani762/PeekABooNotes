//! Small helpers shared by every module: UTF-16 strings, colours, time, paths.
//! (Dates and times for display are formatted per language in `i18n::time`.)

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{FILETIME, RECT, SYSTEMTIME};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn from_wide(s: &[u16]) -> String {
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end])
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 0xRRGGBB -> COLORREF (0x00BBGGRR)
pub const fn cref(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

/// Linear mix of two 0xRRGGBB colours; t = 0 -> a, t = 1 -> b.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xFF) as f32;
    let m = |s: u32| ((ch(a, s) * (1.0 - t) + ch(b, s) * t).round() as u32).min(255) << s;
    m(16) | m(8) | m(0)
}

pub fn x_of(lp: isize) -> i32 {
    (lp & 0xFFFF) as u16 as i16 as i32
}

pub fn y_of(lp: isize) -> i32 {
    ((lp >> 16) & 0xFFFF) as u16 as i16 as i32
}

pub fn key_down(vk: u16) -> bool {
    unsafe { (GetKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn pt_in(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub fn documents_dir() -> PathBuf {
    unsafe {
        let mut p: *mut u16 = std::ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut p) == 0 && !p.is_null() {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            CoTaskMemFree(p as _);
            return PathBuf::from(s);
        }
    }
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Documents")
}

pub fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("peekaboo-notes.exe"))
}

/// Notes live in `Documents\Peekaboo Notes`, unless a file named `portable` sits next to the
/// exe, in which case everything stays beside it in `PeekabooData` (carry it on a USB
/// stick).
pub fn data_dir() -> PathBuf {
    // Worked out (and created) once; it's asked for on every save and some paints.
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = exe_path().parent().map(PathBuf::from).unwrap_or_default();
        let dir = if exe_dir.join("portable").exists() { exe_dir.join("PeekabooData") } else { documents_dir().join("Peekaboo Notes") };
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

/// A Unix timestamp as local wall-clock time.
pub fn local_time(ts: i64) -> SYSTEMTIME {
    unsafe {
        let t = ((ts + 11_644_473_600) as u64) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut loc: SYSTEMTIME = std::mem::zeroed();
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc);
        loc
    }
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
pub fn local_day(t: &SYSTEMTIME) -> i64 {
    days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64)
}

pub fn stamp() -> String {
    let t = local_time(now());
    format!("{:04}-{:02}-{:02} {:02}.{:02}.{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
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


/// Not exported by windows-sys' WindowsAndMessaging module.
pub const WM_MOUSELEAVE: u32 = 0x02A3;
