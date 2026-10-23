// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the portable code (`common`) asks of the operating system, answered by Windows:
//! the local clock, the Documents folder, the preferred languages, locale-formatted times
//! and dates, and a description of the system for the log. Another platform provides the
//! same functions under the same names.

use std::path::PathBuf;

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::Globalization::{
    DATE_SHORTDATE, GetDateFormatEx, GetLocaleInfoEx, GetTimeFormatEx, GetUserDefaultLocaleName, GetUserPreferredUILanguages, LOCALE_SMONTHDAY,
    MUI_LANGUAGE_NAME, TIME_NOSECONDS,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};

use crate::common::util::LocalTime;
use crate::win::util::{from_wide, wide};

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

fn from_system(t: &SYSTEMTIME) -> LocalTime {
    LocalTime {
        year: t.wYear as i32,
        month: t.wMonth as u32,
        day: t.wDay as u32,
        hour: t.wHour as u32,
        minute: t.wMinute as u32,
        second: t.wSecond as u32,
        millis: t.wMilliseconds as u32,
    }
}

fn to_system(t: &LocalTime) -> SYSTEMTIME {
    SYSTEMTIME {
        wYear: t.year as u16,
        wMonth: t.month as u16,
        wDayOfWeek: 0,
        wDay: t.day as u16,
        wHour: t.hour as u16,
        wMinute: t.minute as u16,
        wSecond: t.second as u16,
        wMilliseconds: t.millis as u16,
    }
}

/// A Unix timestamp as local wall-clock time.
pub fn local_time(ts: i64) -> LocalTime {
    unsafe {
        let t = ((ts + 11_644_473_600) as u64) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        let mut loc: SYSTEMTIME = std::mem::zeroed();
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc);
        from_system(&loc)
    }
}

/// Now, to the millisecond (for the log).
pub fn local_now() -> LocalTime {
    unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        from_system(&t)
    }
}

/// Windows' preferred UI languages, most preferred first (e.g. ["de-DE", "en-US"]).
pub fn preferred_languages() -> Vec<String> {
    let (mut count, mut len) = (0u32, 0u32);
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, std::ptr::null_mut(), &mut len) == 0 || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, buf.as_mut_ptr(), &mut len) == 0 {
            return Vec::new();
        }
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// The locale to format with for interface language `lang`: the user's own (with their
/// regional choices) when it is in that language, else the language's default locale.
fn locale(lang: &str) -> Vec<u16> {
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 {
        let user = String::from_utf16_lossy(&buf[..n as usize - 1]);
        let base = |s: &str| s.split('-').next().unwrap_or("").to_ascii_lowercase();
        if base(&user) == base(lang) {
            return buf[..n as usize].to_vec();
        }
    }
    wide(lang)
}

/// "3:42 PM" / "15:42" in `lang`'s locale.
pub fn format_clock(t: &LocalTime, lang: &str) -> Option<String> {
    let (loc, st) = (locale(lang), to_system(t));
    let mut buf = [0u16; 64];
    let n = unsafe { GetTimeFormatEx(loc.as_ptr(), TIME_NOSECONDS, &st, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// "October 18" (this year) or the short date ("18/10/2025") in `lang`'s locale.
pub fn format_date(t: &LocalTime, lang: &str, this_year: bool) -> Option<String> {
    let (loc, st) = (locale(lang), to_system(t));
    // The locale's own month-day pattern ("MMMM d", "d. MMMM"): GetDateFormatEx refuses
    // DATE_MONTHDAY as invalid flags.
    let month_day = this_year
        .then(|| {
            let mut pic = [0u16; 80];
            let n = unsafe { GetLocaleInfoEx(loc.as_ptr(), LOCALE_SMONTHDAY, pic.as_mut_ptr(), pic.len() as i32) };
            (n > 1).then(|| pic[..n as usize].to_vec())
        })
        .flatten();
    let (flags, pic) = match &month_day {
        Some(p) => (0, p.as_ptr()),
        None => (DATE_SHORTDATE, std::ptr::null()),
    };
    let mut buf = [0u16; 96];
    let n = unsafe { GetDateFormatEx(loc.as_ptr(), flags, &st, pic, buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// "Tuesday" in `lang`'s locale.
pub fn format_weekday(t: &LocalTime, lang: &str) -> Option<String> {
    let (loc, st) = (locale(lang), to_system(t));
    let mut buf = [0u16; 64];
    let pic = wide("dddd");
    let n = unsafe { GetDateFormatEx(loc.as_ptr(), 0, &st, pic.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// The system's display scale, in percent.
pub fn display_scale() -> u32 {
    unsafe { GetDpiForSystem() * 100 / 96 }
}

/// "Windows 11 25H2 (build 26200.6899)". From the registry, which (unlike GetVersionEx)
/// doesn't depend on what the manifest claims to support.
pub fn system_description() -> String {
    let key = wide("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let text = |name: &str| unsafe {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        let ok = RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), wide(name).as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as _, &mut size) == 0;
        if ok { from_wide(&buf) } else { String::new() }
    };
    let ubr = unsafe {
        let (mut v, mut size) = (0u32, 4u32);
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), wide("UBR").as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), &mut v as *mut u32 as _, &mut size);
        v
    };
    let build = text("CurrentBuild");
    // ProductName still says "Windows 10" on 11; the build number tells them apart.
    let major = if build.parse::<u32>().unwrap_or(0) >= 22000 { 11 } else { 10 };
    let release = text("DisplayVersion");
    let release = if release.is_empty() { text("ReleaseId") } else { release };
    format!("Windows {major} {release} (build {build}.{ubr})").replace("  ", " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        let t = local_time(1_792_570_440); // 2026-10-21, mid-afternoon
        for lang in ["en", "de", "ja"] {
            assert!(format_clock(&t, lang).is_some(), "clock {lang}");
            assert!(format_weekday(&t, lang).is_some(), "weekday {lang}");
            assert!(format_date(&t, lang, true).is_some(), "date this year {lang}");
            assert!(format_date(&t, lang, false).is_some(), "short date {lang}");
        }
        // English follows the user's own regional order ("October 21" or "21 October").
        let en = format_date(&t, "en", true).unwrap();
        assert!(en.contains("October") && en.contains("21") && !en.contains("2026"), "{en}");
        assert_eq!(format_date(&t, "de", true).as_deref(), Some("21. Oktober"));
    }

    #[test]
    fn describes_windows() {
        let v = system_description();
        assert!(v.starts_with("Windows 1") && v.contains("(build "), "{v}");
    }
}
