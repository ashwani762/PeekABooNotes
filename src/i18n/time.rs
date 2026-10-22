//! Dates and times in the interface language, formatted by Windows for that locale
//! ("3:42 PM" / "15:42", "October 18" / "18. Oktober").

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{DATE_MONTHDAY, DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, GetUserDefaultLocaleName, TIME_NOSECONDS};

use crate::platform::util::{local_day, local_time, now};

/// The locale to format with: the user's own (with their regional choices) when it is
/// in the interface language, else that language's default locale.
fn locale() -> Vec<u16> {
    let lang = super::tag();
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 {
        let user = String::from_utf16_lossy(&buf[..n as usize - 1]);
        let base = |s: &str| s.split('-').next().unwrap_or("").to_ascii_lowercase();
        if base(&user) == base(&lang) {
            return buf[..n as usize].to_vec();
        }
    }
    lang.encode_utf16().chain(Some(0)).collect()
}

fn clock(t: &SYSTEMTIME) -> String {
    let loc = locale();
    let mut buf = [0u16; 64];
    let n = unsafe { GetTimeFormatEx(loc.as_ptr(), TIME_NOSECONDS, t, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { format!("{:02}:{:02}", t.wHour, t.wMinute) }
}

/// "October 18" this year, the short date ("18/10/2025") for other years.
fn date(t: &SYSTEMTIME, this_year: bool) -> String {
    let loc = locale();
    let mut buf = [0u16; 96];
    let flags = if this_year { DATE_MONTHDAY } else { DATE_SHORTDATE };
    let n = unsafe { GetDateFormatEx(loc.as_ptr(), flags, t, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { format!("{}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay) }
}

fn weekday(t: &SYSTEMTIME) -> String {
    let loc = locale();
    let mut buf = [0u16; 64];
    let pic: Vec<u16> = "dddd\0".encode_utf16().collect();
    let n = unsafe { GetDateFormatEx(loc.as_ptr(), 0, t, pic.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null()) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { date(t, true) }
}

/// Compact list time: "3:42 PM", "Yesterday", "Tuesday", "October 18".
pub fn relative_time(ts: i64) -> String {
    let (t, today) = (local_time(ts), local_time(now()));
    match local_day(&today) - local_day(&t) {
        i64::MIN..=0 => clock(&t),
        1 => crate::t!("time_yesterday").to_string(),
        2..=6 => weekday(&t),
        _ => date(&t, t.wYear == today.wYear),
    }
}

/// Sentence form for details: "Today at 3:42 PM", "Yesterday at 9:10 AM",
/// "October 18 at 11:02 AM".
pub fn long_time(ts: i64) -> String {
    let (t, today) = (local_time(ts), local_time(now()));
    match local_day(&today) - local_day(&t) {
        i64::MIN..=0 => crate::t!("time_today_at", clock(&t)),
        1 => crate::t!("time_yesterday_at", clock(&t)),
        _ => crate::t!("time_date_at", date(&t, t.wYear == today.wYear), clock(&t)),
    }
}

/// "just now", "5 mins ago", then `long_time`.
pub fn edited_time(ts: i64) -> String {
    match now() - ts {
        i64::MIN..60 => crate::t!("time_just_now").to_string(),
        d @ 60..3600 => crate::tn!("time_minutes_ago", d / 60),
        _ => long_time(ts),
    }
}
