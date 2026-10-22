// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain-text Markdown in and out (titles, search, previews, export, import).

use super::*;

/// Convert the editor's content (already loaded into `h`) to Markdown.
/// With `inline` false only list/task markers are produced (cheap: one query per paragraph).
pub fn to_markdown(h: HWND, inline: bool) -> String {
    let t = text(h);
    let mut out = String::new();
    let mut start = 0usize;
    let mut numbers = [0usize; MAX_LEVEL as usize + 1];
    while start <= t.len() {
        let end = t[start..].iter().position(|&c| c == CR).map(|i| start + i).unwrap_or(t.len());
        set_sel(h, start as i32, start as i32);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        let num = pf.numbering;
        let task = start < end && is_task(t[start]);
        let level = if task || num != 0 { level_of(pf.start_indent, list_base(task)) as usize } else { 0 };
        let mut body_start = start;
        let mut prefix = "  ".repeat(level);
        match num {
            PFN_BULLET => prefix.push_str("- "),
            PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN => {
                numbers[level] += 1;
                for n in numbers.iter_mut().skip(level + 1) {
                    *n = 0;
                }
                prefix.push_str(&format!("{}. ", numbers[level]));
            }
            _ => numbers = [0; MAX_LEVEL as usize + 1],
        }
        if task {
            if prefix.is_empty() {
                prefix.push_str("- ");
            }
            if prefix.trim().is_empty() {
                prefix.push_str("- ");
            }
            prefix.push_str(if t[start] == CHECKED { "[x] " } else { "[ ] " });
            body_start += 1;
            if body_start < end && t[body_start] == b' ' as u16 {
                body_start += 1;
            }
        }
        out.push_str(&prefix);
        if inline && start == 0 && prefix.is_empty() && end > 0 {
            out.push_str("# ");
            out.push_str(&String::from_utf16_lossy(&t[..end]));
        } else if inline {
            let body = inline_md(h, &t, body_start, end);
            // A ticked item's strikethrough is how it looks, not part of its text.
            let done = task && t[start] == CHECKED;
            out.push_str(if done { body.strip_prefix("~~").and_then(|b| b.strip_suffix("~~")).unwrap_or(&body) } else { &body });
        } else {
            out.push_str(&String::from_utf16_lossy(&t[body_start..end]));
        }
        out.push('\n');
        if end >= t.len() {
            break;
        }
        start = end + 1;
    }
    // Inline mode is for export: plain `-` lists in the light mode are what we store.
    if !inline {
        out = out.replace("- [ ] ", "[ ] ").replace("- [x] ", "[x] ");
    }
    out.trim_end().to_string()
}

pub(super) fn inline_md(h: HWND, t: &[u16], a: usize, b: usize) -> String {
    let mut out = String::new();
    let mut cur = (false, false, false);
    for i in a..b {
        set_sel(h, i as i32, i as i32 + 1);
        let cf = char_format(h, SCF_SELECTION);
        let st = (cf.effects & CFM_BOLD != 0, cf.effects & CFM_ITALIC != 0, cf.effects & CFM_STRIKEOUT != 0);
        if st != cur {
            close_marks(&mut out, cur, st);
            if st.0 && !cur.0 {
                out.push_str("**");
            }
            if st.1 && !cur.1 {
                out.push('_');
            }
            if st.2 && !cur.2 {
                out.push_str("~~");
            }
            cur = st;
        }
        out.push_str(&String::from_utf16_lossy(&t[i..i + 1]));
    }
    close_marks(&mut out, cur, (false, false, false));
    out
}

pub(super) fn close_marks(out: &mut String, cur: (bool, bool, bool), next: (bool, bool, bool)) {
    if cur.2 && !next.2 {
        out.push_str("~~");
    }
    if cur.1 && !next.1 {
        out.push('_');
    }
    if cur.0 && !next.0 {
        out.push_str("**");
    }
}

/// Load plain text / light Markdown into `h`, turning list markers into real formatting.
pub fn load_markdown(h: HWND, md: &str) {
    enum Kind {
        Plain,
        Bullet,
        Number,
        Task(bool),
    }
    let mut lines: Vec<(Kind, i32, String, Runs)> = Vec::new();
    // A file's final newline (and any blank lines after the text) is not an empty item.
    for raw in md.replace("\r\n", "\n").trim_end().split('\n') {
        let l = raw.trim_start();
        // Two spaces (or a tab) per nesting level, as `to_markdown` writes them.
        let indent: usize = raw[..raw.len() - l.len()].chars().map(|c| if c == '\t' { 2 } else { 1 }).sum();
        let hashes = l.chars().take_while(|&c| c == '#').count();
        let (k, rest) = if let Some(r) = l.strip_prefix("- [ ] ").or(l.strip_prefix("[ ] ")) {
            (Kind::Task(false), r)
        } else if let Some(r) =
            l.strip_prefix("- [x] ").or(l.strip_prefix("- [X] ")).or(l.strip_prefix("[x] ")).or(l.strip_prefix("[X] "))
        {
            (Kind::Task(true), r)
        } else if let Some(r) = l[hashes..].strip_prefix(' ').filter(|_| (1..=6).contains(&hashes)) {
            (Kind::Plain, r)
        } else if let Some(r) = l.strip_prefix("- ").or(l.strip_prefix("* ")).or(l.strip_prefix("• ")) {
            (Kind::Bullet, r)
        } else {
            let d = l.chars().take_while(|c| c.is_ascii_digit()).count();
            match l[d..].strip_prefix(". ") {
                Some(r) if d > 0 => (Kind::Number, r),
                _ => (Kind::Plain, raw),
            }
        };
        let (text, mut runs) = inline_marks(rest);
        let mark = match k {
            Kind::Task(true) => "\u{2611} ",
            Kind::Task(false) => "\u{2610} ",
            _ => "",
        };
        let shift = mark.encode_utf16().count() as i32;
        for r in &mut runs {
            r.0 += shift;
            r.1 += shift;
        }
        let level = if matches!(k, Kind::Plain) { 0 } else { (indent / 2).min(MAX_LEVEL as usize) as i32 };
        lines.push((k, level, format!("{mark}{text}"), runs));
    }
    let joined: Vec<&str> = lines.iter().map(|(_, _, s, _)| s.as_str()).collect();
    set_sel(h, 0, -1);
    insert_text(h, &joined.join("\r"));
    // Start clean: a reused control keeps the last document's list and character formats.
    set_sel(h, 0, -1);
    set_numbering(h, 0);
    set_effect(h, CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT, false);
    let mut pos = 0i32;
    for (k, level, s, runs) in &lines {
        let len = s.encode_utf16().count() as i32;
        set_sel(h, pos, pos);
        match k {
            Kind::Bullet => set_numbering(h, PFN_BULLET),
            Kind::Number => set_numbering(h, PFN_ARABIC),
            Kind::Task(true) => {
                set_sel(h, pos + 1, pos + len);
                set_effect(h, CFM_STRIKEOUT, true);
            }
            _ => {}
        }
        for _ in 0..*level {
            set_sel(h, pos, pos);
            nest(h, 1);
        }
        if matches!(k, Kind::Number) && *level == 0 {
            // After a nested sublist RichEdit would restart the count: carry it on.
            let before = siblings_before(h, &text(h), pos, 0);
            if before > 0 {
                set_sel(h, pos, pos);
                let mut pf = ParaFormat2::new();
                pf.mask = PFM_NUMBERINGSTART;
                pf.numbering_start = 1 + before as u16;
                send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
            }
        }
        for &(a, b, effect) in runs {
            set_sel(h, pos + a, pos + b);
            set_effect(h, effect, true);
        }
        pos += len + 1;
    }
    set_sel(h, 0, 0);
}

/// Character-format runs within a line: (start, end, effect) in UTF-16 units.
type Runs = Vec<(i32, i32, u32)>;

/// Strip `**bold**`, `_italic_` and `~~strike~~` marks from one line (as `to_markdown`
/// writes them), returning the text and the (start, end, effect) runs in UTF-16 units.
/// A mark without a partner stays literal text, and `_` inside a word (snake_case) is
/// never a mark.
pub(super) fn inline_marks(s: &str) -> (String, Runs) {
    let c: Vec<char> = s.chars().collect();
    let at = |k: usize| c.get(k).copied();
    let space = |k: Option<usize>| k.and_then(at).is_none_or(char::is_whitespace);
    let word = |k: Option<usize>| k.and_then(at).is_some_and(char::is_alphanumeric);
    let mark = |k: usize| match (at(k), at(k + 1)) {
        (Some('*'), Some('*')) => Some((0, 2, CFM_BOLD)),
        (Some('~'), Some('~')) => Some((2, 2, CFM_STRIKEOUT)),
        (Some('_'), _) => Some((1, 1, CFM_ITALIC)),
        _ => None,
    };
    // Can a mark of `slot` at `k` close a run / open one?
    let closes = |k: usize, slot: usize, len: usize| !space(k.checked_sub(1)) && (slot != 1 || !word(Some(k + len)));
    let opens = |k: usize, slot: usize, len: usize| !space(Some(k + len)) && (slot != 1 || !word(k.checked_sub(1)));
    let mut out = String::new();
    let mut n = 0i32;
    let mut open: [Option<i32>; 3] = [None; 3];
    let mut runs = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if let Some((slot, len, effect)) = mark(i) {
            match open[slot] {
                Some(a) if closes(i, slot, len) => {
                    runs.push((a, n, effect));
                    open[slot] = None;
                    i += len;
                    continue;
                }
                None if opens(i, slot, len)
                    && (i + len..c.len()).any(|j| mark(j).is_some_and(|m| m.0 == slot) && j > i + len && closes(j, slot, len)) =>
                {
                    open[slot] = Some(n);
                    i += len;
                    continue;
                }
                _ => {}
            }
        }
        out.push(c[i]);
        n += c[i].len_utf16() as i32;
        i += 1;
    }
    (out, runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_marks_round_trip() {
        let (t, r) = inline_marks("Ship **v2.4** by _Friday_, not ~~Thursday~~");
        assert_eq!(t, "Ship v2.4 by Friday, not Thursday");
        assert_eq!(r, vec![(5, 9, CFM_BOLD), (13, 19, CFM_ITALIC), (25, 33, CFM_STRIKEOUT)]);
        // Nested, as `inline_md` writes them.
        let (t, r) = inline_marks("**_both_**");
        assert_eq!(t, "both");
        assert_eq!(r, vec![(0, 4, CFM_ITALIC), (0, 4, CFM_BOLD)]);
        // Not marks: snake_case, a lone star pair, spaced underscores.
        for s in ["user_id and api_key", "2 ** 3", "a _ b _ c", "**open only"] {
            assert_eq!(inline_marks(s), (s.to_string(), vec![]), "{s}");
        }
    }
}
