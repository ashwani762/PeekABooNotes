// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The YAML front matter at the top of an exported note: what the Markdown itself can't
//! say (colour, pinned, archived, off the deck, when it was made and last changed), so
//! Export All and then Import brings the notes back as they were. Markdown viewers that
//! know front matter show it as the file's properties; the rest show a few short lines.

use crate::common::mcp::{color_name, color_of};
use crate::common::store::Note;
use crate::common::util::days_from_civil;

/// A note's state as front matter carries it. Dates are Unix seconds, 0 when not known.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub color: Option<u8>,
    pub pinned: bool,
    pub archived: bool,
    pub in_deck: bool,
    pub created: i64,
    pub updated: i64,
}

impl Default for Meta {
    fn default() -> Meta {
        Meta { color: None, pinned: false, archived: false, in_deck: true, created: 0, updated: 0 }
    }
}

impl Meta {
    pub fn of(n: &Note) -> Meta {
        Meta { color: Some(n.color), pinned: n.pinned, archived: n.archived, in_deck: !n.off_deck, created: n.created, updated: n.updated }
    }

    /// Give note `n` what this carries; what it doesn't say stays as the note has it.
    pub fn apply(&self, n: &mut Note) {
        if let Some(c) = self.color {
            n.color = c;
        }
        n.pinned = self.pinned;
        n.archived = self.archived;
        n.off_deck = !self.in_deck;
        if self.updated > 0 {
            n.updated = self.updated;
        }
        if self.created > 0 {
            n.created = self.created;
        }
        // Only one date given: a note isn't made after its last change.
        n.created = n.created.min(n.updated);
    }
}

/// The block for `m`, ending in a line break, with only what differs from a fresh note's
/// defaults; empty when nothing does.
pub fn write(m: &Meta) -> String {
    let mut lines = Vec::new();
    if let Some(c) = m.color {
        lines.push(format!("color: {}", color_name(c)));
    }
    if m.pinned {
        lines.push("pinned: true".into());
    }
    if m.archived {
        lines.push("archived: true".into());
    }
    if !m.in_deck {
        lines.push("in_deck: false".into());
    }
    if m.created > 0 {
        lines.push(format!("created: {}", iso(m.created)));
    }
    if m.updated > 0 {
        lines.push(format!("updated: {}", iso(m.updated)));
    }
    if lines.is_empty() { String::new() } else { format!("---\n{}\n---\n", lines.join("\n")) }
}

/// The front matter at the top of `text`, and the text after it. Only a block of this
/// app's own keys with values it understands counts: another program's front matter (tags,
/// aliases) or a note that just starts with a `---` line comes back whole, as plain text.
pub fn split(text: &str) -> (Option<Meta>, &str) {
    let plain = (None, text);
    let mut rest = text;
    if next_line(&mut rest).map(str::trim_end) != Some("---") {
        return plain;
    }
    let mut meta = Meta::default();
    let mut any = false;
    loop {
        // Never closed: not front matter.
        let Some(line) = next_line(&mut rest) else { return plain };
        let line = line.trim_end();
        if line == "---" || line == "..." {
            break;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { return plain };
        // An indented key is nested YAML, which this never writes.
        if key.trim() != key {
            return plain;
        }
        let value = value_of(value);
        let ok = match key {
            "color" | "colour" => color_of(value).map(|c| meta.color = Some(c)),
            "pinned" => flag(value).map(|b| meta.pinned = b),
            "archived" => flag(value).map(|b| meta.archived = b),
            "in_deck" => flag(value).map(|b| meta.in_deck = b),
            "created" => parse_iso(value).map(|t| meta.created = t),
            "updated" => parse_iso(value).map(|t| meta.updated = t),
            _ => None,
        };
        if ok.is_none() {
            return plain;
        }
        any = true;
    }
    if !any {
        return plain;
    }
    // A blank line after the block is the gap a hand-written file leaves, not the note's.
    let mut after = rest;
    if next_line(&mut after).is_some_and(|l| l.trim().is_empty()) {
        rest = after;
    }
    (Some(meta), rest)
}

/// The next line of `rest` without its line break, moving `rest` past it.
fn next_line<'a>(rest: &mut &'a str) -> Option<&'a str> {
    if rest.is_empty() {
        return None;
    }
    let (line, tail) = match rest.find('\n') {
        Some(i) => (&rest[..i], &rest[i + 1..]),
        None => (*rest, ""),
    };
    *rest = tail;
    Some(line.strip_suffix('\r').unwrap_or(line))
}

/// A scalar's text: quotes taken off, or a trailing ` # comment` when it isn't quoted.
fn value_of(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v.find(" #").map_or(v, |i| v[..i].trim_end())
}

fn flag(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(true),
        "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Unix seconds as ISO 8601 in UTC: "2026-10-25T14:03:07Z".
pub fn iso(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// The date for a day number since 1970-01-01 (Howard Hinnant's algorithm, the inverse of
/// `days_from_civil`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// An ISO 8601 date or date and time as Unix seconds: "2026-10-25", "2026-10-25T14:03Z",
/// "2026-10-25 16:03:07.5+02:00". Without a zone it is UTC, as YAML reads it.
pub fn parse_iso(s: &str) -> Option<i64> {
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        if part.bytes().all(|b| b.is_ascii_digit()) { part.parse().ok() } else { None }
    };
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let mut t = days_from_civil(y, mo, d) * 86_400;
    let mut rest = &s[10..];
    if rest.is_empty() {
        return Some(t);
    }
    rest = rest.strip_prefix(['T', 't', ' '])?;
    let at = |r: &str, i: usize| r.as_bytes().get(i).copied();
    let two = |r: &str| -> Option<i64> {
        let part = r.get(..2)?;
        if part.bytes().all(|b| b.is_ascii_digit()) { part.parse().ok() } else { None }
    };
    let (h, mi) = (two(rest)?, two(rest.get(3..)?)?);
    if at(rest, 2) != Some(b':') || h > 23 || mi > 59 {
        return None;
    }
    t += h * 3600 + mi * 60;
    rest = &rest[5..];
    if at(rest, 0) == Some(b':') {
        let sec = two(rest.get(1..)?)?;
        if sec > 60 {
            return None;
        }
        t += sec;
        rest = &rest[3..];
        if at(rest, 0) == Some(b'.') {
            rest = rest[1..].trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    let rest = rest.trim_start();
    match rest {
        "" | "Z" | "z" => Some(t),
        _ => {
            let sign = match at(rest, 0)? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let zone = &rest[1..];
            let (oh, om) = match zone.len() {
                1 | 2 => (zone.parse::<i64>().ok().filter(|_| zone.bytes().all(|b| b.is_ascii_digit()))?, 0),
                4 => (two(zone)?, two(&zone[2..])?),
                5 if at(zone, 2) == Some(b':') => (two(zone)?, two(&zone[3..])?),
                _ => return None,
            };
            if oh > 23 || om > 59 {
                return None;
            }
            Some(t - sign * (oh * 3600 + om * 60))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        for t in [1, 86_399, 951_782_400, 1_761_400_987, 4_102_444_800] {
            assert_eq!(parse_iso(&iso(t)), Some(t), "{}", iso(t));
        }
        assert_eq!(iso(1_761_400_987), "2025-10-25T14:03:07Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(parse_iso("2025-10-25"), Some(1_761_350_400));
        assert_eq!(parse_iso("2025-10-25T16:03:07+02:00"), Some(1_761_400_987));
        assert_eq!(parse_iso("2025-10-25 19:33:07.25 +0530"), Some(1_761_400_987));
        assert_eq!(parse_iso("2025-10-25T09:03:07-5"), Some(1_761_400_987));
        assert_eq!(parse_iso("2025-10-25t14:03z"), Some(1_761_400_980));
        for bad in ["", "2025-13-01", "2025-10-25T25:00", "2025-10-25T14", "25-10-2025", "2025-10-25T14:03:07+2:00", "2025-1o-25", "soon"] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }

    #[test]
    fn front_matter_round_trips() {
        let m = Meta { color: Some(4), pinned: true, archived: true, in_deck: false, created: 1_700_000_000, updated: 1_761_400_987 };
        let block = write(&m);
        assert_eq!(
            block,
            "---\ncolor: Blue\npinned: true\narchived: true\nin_deck: false\ncreated: 2023-11-14T22:13:20Z\nupdated: 2025-10-25T14:03:07Z\n---\n"
        );
        let file = format!("{block}Groceries\n- milk\n");
        assert_eq!(split(&file), (Some(m), "Groceries\n- milk\n"));
        // Only what differs from a fresh note is written.
        let plain = Meta { color: Some(0), ..Meta::default() };
        assert_eq!(write(&plain), "---\ncolor: Yellow\n---\n");
        assert_eq!(write(&Meta::default()), "");
        assert_eq!(split("---\ncolor: Yellow\n---\nT"), (Some(plain), "T"));
    }

    #[test]
    fn hand_written_front_matter_is_read() {
        let file = "---\r\n# mine\r\ncolour: 'mint'\r\npinned: yes  # keep\r\nupdated: \"2025-10-25\"\r\n...\r\n\r\nTitle\r\n";
        let (m, rest) = split(file);
        let m = m.expect("front matter");
        assert_eq!((m.color, m.pinned, m.archived, m.in_deck, m.created, m.updated), (Some(5), true, false, true, 0, 1_761_350_400));
        assert_eq!(rest, "Title\r\n");
    }

    #[test]
    fn anything_else_stays_text() {
        for text in [
            "Title\n---\ncolor: Blue\n---\n",
            "---\n",
            "---\ncolor: Blue\n",
            "---\n---\nT",
            "---\ntags: [a, b]\n---\nT",
            "---\ncolor: Blue\ntags: work\n---\nT",
            "---\ncolor: Purple\n---\nT",
            "---\npinned: maybe\n---\nT",
            "---\n  color: Blue\n---\nT",
            "---\nA rule, then a line\n---\nT",
            "----\ncolor: Blue\n---\nT",
            "",
        ] {
            assert_eq!(split(text), (None, text), "{text:?}");
        }
    }

    #[test]
    fn apply_sets_the_note_state() {
        let mut n = Note { color: 0, created: 2_000_000_000, updated: 2_000_000_000, ..Default::default() };
        Meta { color: Some(3), pinned: true, archived: false, in_deck: false, created: 0, updated: 1_761_400_987 }.apply(&mut n);
        assert_eq!((n.color, n.pinned, n.archived, n.off_deck, n.created, n.updated), (3, true, false, true, 1_761_400_987, 1_761_400_987));
    }
}
