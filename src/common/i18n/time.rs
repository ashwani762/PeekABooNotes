// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dates and times in the interface language, formatted by the OS for that locale
//! ("3:42 PM" / "15:42", "October 18" / "18. Oktober").

use crate::common::util::{LocalTime, local_day, now};
use crate::os;

fn clock(t: &LocalTime) -> String {
    os::format_clock(t, &super::tag()).unwrap_or_else(|| format!("{:02}:{:02}", t.hour, t.minute))
}

/// "October 18" this year, the short date ("18/10/2025") for other years.
fn date(t: &LocalTime, this_year: bool) -> String {
    os::format_date(t, &super::tag(), this_year).unwrap_or_else(|| format!("{}-{:02}-{:02}", t.year, t.month, t.day))
}

fn weekday(t: &LocalTime) -> String {
    os::format_weekday(t, &super::tag()).unwrap_or_else(|| date(t, true))
}

/// Compact list time: "3:42 PM", "yesterday", "Tuesday", "October 18". Always used
/// within a sentence ("Edited yesterday"), so today and yesterday are lowercase.
pub fn relative_time(ts: i64) -> String {
    let (t, today) = (os::local_time(ts), os::local_time(now()));
    match local_day(&today) - local_day(&t) {
        i64::MIN..=0 => clock(&t),
        1 => crate::t!("time_yesterday").to_string(),
        2..=6 => weekday(&t),
        _ => date(&t, t.year == today.year),
    }
}

/// Sentence form for details: "today at 3:42 PM", "yesterday at 9:10 AM",
/// "October 18 at 11:02 AM".
pub fn long_time(ts: i64) -> String {
    let (t, today) = (os::local_time(ts), os::local_time(now()));
    match local_day(&today) - local_day(&t) {
        i64::MIN..=0 => crate::t!("time_today_at", clock(&t)),
        1 => crate::t!("time_yesterday_at", clock(&t)),
        _ => crate::t!("time_date_at", date(&t, t.year == today.year), clock(&t)),
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
