// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Forgiving search over notes. Each word of the query has to turn up somewhere in the
//! note, in any order, and may do so as:
//!   - the start of a word ("rel" finds "release"), the best kind of match;
//!   - anywhere inside the text ("lease" finds "release");
//!   - its letters in order, in a few runs, starting a word ("relchk" finds
//!     "release checklist");
//!   - a word with a typo or two ("relaese", "chekclist").
//!
//! Case and accents don't matter ("cafe" finds "Café"). Title matches count for more.

use std::cell::RefCell;

/// One prepared query.
pub struct Query {
    terms: Vec<Vec<char>>,
    /// Buffers kept from one note to the next: a search scores every note at each
    /// keystroke, and would otherwise allocate for each of them. They live as long as the
    /// query, and so are only ever as big as the longest note it was asked about.
    scratch: RefCell<Scratch>,
}

#[derive(Default)]
struct Scratch {
    title: Vec<char>,
    body: Vec<char>,
    work: Work,
}

/// What `find` works in: the letters of an in-order match, and the three rows of the
/// edit distance it needs.
#[derive(Default)]
struct Work {
    hits: Vec<usize>,
    rows: [Vec<usize>; 3],
}

/// A note's match: its score (higher is better) and the matched ranges, as char
/// indices into the strings given to `Query::score`.
#[derive(Debug, Default, Clone)]
pub struct Match {
    pub score: i32,
    pub title: Vec<(usize, usize)>,
    /// Where the first body-only match sits, for showing a snippet around it.
    pub body_at: Option<usize>,
}

/// Lowercase, without accents, one char for one char (so indices carry over).
fn fold(c: char) -> char {
    // Most text is ASCII, which only needs its capitals lowered.
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ř' => 'r',
        'ś' | 'š' | 'ş' => 's',
        'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        c => c,
    }
}

fn fold_all(s: &str) -> Vec<char> {
    s.chars().map(fold).collect()
}

/// `s` folded into `into`, which keeps its allocation.
fn fold_into(s: &str, into: &mut Vec<char>) {
    into.clear();
    into.extend(s.chars().map(fold));
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

fn word_start(t: &[char], i: usize) -> bool {
    i == 0 || !is_word(t[i - 1])
}

/// The best score `find` can give a term of `n` letters. Exact matches score up to 120;
/// letters in order gain 4 for each one on a word start, which needs a gap before it that
/// costs 2, so they stay under 60 + 4 a letter.
fn best_possible(n: usize) -> i32 {
    120.max(60 + 4 * n as i32)
}

/// The best way `term` turns up in `t`: (score, matched char ranges).
fn find(term: &[char], t: &[char], w: &mut Work) -> Option<(i32, Vec<(usize, usize)>)> {
    let n = term.len();
    if n == 0 || t.len() < n.min(t.len() + 1) {
        return None;
    }
    // Exact, at a word start (a whole word does better still), else anywhere.
    let mut inside = None;
    if t.len() >= n {
        for i in 0..=t.len() - n {
            if t[i] == term[0] && t[i..i + n] == *term {
                if word_start(t, i) {
                    let whole = i + n == t.len() || !is_word(t[i + n]);
                    return Some((if whole { 120 } else { 100 }, vec![(i, i + n)]));
                }
                inside.get_or_insert(i);
            }
        }
    }
    if let Some(i) = inside {
        return Some((70, vec![(i, i + n)]));
    }
    if n < 2 || !term.iter().all(|c| is_word(*c)) {
        return None;
    }
    // In order, starting a word, close together and in a few runs: at most one run per
    // two letters, so "test" doesn't turn up as t-e-st in "the best".
    if n >= 4 {
        let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
        for start in 0..t.len() {
            if t[start] != term[0] || !word_start(t, start) {
                continue;
            }
            let hits = &mut w.hits;
            hits.clear();
            hits.push(start);
            let (mut k, mut i) = (1, start + 1);
            while k < n && i < t.len() && i - start <= n * 3 {
                if t[i] == term[k] {
                    hits.push(i);
                    k += 1;
                }
                i += 1;
            }
            if k == n && runs(hits) <= n / 2 {
                let span = hits[n - 1] - start + 1;
                // Each letter landing on a word start counts for something.
                let starts = hits.iter().filter(|&&h| word_start(t, h)).count() as i32;
                let score = 50 - (span - n) as i32 * 2 + starts * 4;
                if best.as_ref().is_none_or(|b| score > b.0) {
                    best = Some((score, merge(hits)));
                }
            }
        }
        if let Some(b) = best.filter(|b| b.0 > 20) {
            return Some(b);
        }
    }
    // Three letters in order inside one word, from its start: "chk" for "checklist".
    if n == 3 {
        for start in 0..t.len() {
            if t[start] != term[0] || !word_start(t, start) {
                continue;
            }
            let end = (start..t.len()).find(|&j| !is_word(t[j])).unwrap_or(t.len());
            let hits = &mut w.hits;
            hits.clear();
            hits.push(start);
            let mut k = 1;
            for (i, &c) in t.iter().enumerate().take(end.min(start + 10)).skip(start + 1) {
                if k < n && c == term[k] {
                    hits.push(i);
                    k += 1;
                }
            }
            if k == n {
                return Some((30, merge(hits)));
            }
        }
    }
    // A word with a typo or two, compared with the same length of its start. Only for
    // longer words, and the first letter has to be right: "test" isn't "best".
    if n >= 5 {
        let allowed = if n >= 8 { 2 } else { 1 };
        let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
        let mut i = 0;
        while i < t.len() {
            if !is_word(t[i]) {
                i += 1;
                continue;
            }
            let end = (i..t.len()).find(|&j| !is_word(t[j])).unwrap_or(t.len());
            let word = &t[i..end];
            if word[0] != term[0] {
                i = end;
                continue;
            }
            // One table for the longest start compared gives the distance to the shorter
            // ones too: column j only depends on the word's first j letters.
            let dist = distances(term, &word[..word.len().min(n + 1)], &mut w.rows);
            for len in [n.saturating_sub(1), n, n + 1] {
                if len == 0 || len > word.len() {
                    continue;
                }
                let d = dist[len];
                if d <= allowed {
                    let score = 40 - d as i32 * 12;
                    if best.as_ref().is_none_or(|b| score > b.0) {
                        best = Some((score, vec![(i, i + len)]));
                    }
                }
            }
            i = end;
        }
        return best;
    }
    None
}

/// Ranges in order, those that touch or overlap made one: two words matching "Rel" and
/// "ease" mark "Release" once, not as two pills side by side.
fn joined(mut r: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    r.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (a, b) in r {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// How many ranges `merge` makes of `hits`, without making them.
fn runs(hits: &[usize]) -> usize {
    hits.windows(2).filter(|p| p[1] != p[0] + 1).count() + usize::from(!hits.is_empty())
}

/// Neighbouring indices as ranges: [3, 4, 5, 9] -> [(3, 6), (9, 10)].
fn merge(hits: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &h in hits {
        match out.last_mut() {
            Some(r) if r.1 == h => r.1 = h + 1,
            _ => out.push((h, h + 1)),
        }
    }
    out
}

/// Edit distances, counting a swap of neighbours as one edit, from `a` to each start of
/// `b`: element j is the distance to `b[..j]`. Row `i` of the table needs only rows
/// `i - 1` and `i - 2`, so it's worked out in three rows used in turn.
fn distances<'a>(a: &[char], b: &[char], rows: &'a mut [Vec<usize>; 3]) -> &'a [usize] {
    let (n, m) = (a.len(), b.len());
    for r in rows.iter_mut() {
        r.clear();
        r.resize(m + 1, 0);
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        let (cur, prev, before) = (i % 3, (i - 1) % 3, (i + 1) % 3);
        rows[cur][0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut d = (rows[prev][j] + 1).min(rows[cur][j - 1] + 1).min(rows[prev][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d = d.min(rows[before][j - 2] + 1);
            }
            rows[cur][j] = d;
        }
    }
    &rows[n % 3]
}

impl Query {
    pub fn new(q: &str) -> Query {
        Query { terms: q.split_whitespace().map(fold_all).filter(|t| !t.is_empty()).collect(), scratch: RefCell::default() }
    }

    /// How well a note with this title and body matches; None when some word of the
    /// query isn't in it.
    pub fn score(&self, title: &str, body: &str) -> Option<Match> {
        let mut scratch = self.scratch.borrow_mut();
        let Scratch { title: t, body: b, work } = &mut *scratch;
        fold_into(title, t);
        // The body is folded only when a term needs it.
        let mut body_folded = false;
        let mut m = Match::default();
        for term in &self.terms {
            let in_title = find(term, t, work);
            // A title match no body match could outdo settles the term without the body.
            let in_body = if in_title.as_ref().is_some_and(|(ts, _)| ts * 2 >= best_possible(term.len())) {
                None
            } else {
                if !body_folded {
                    fold_into(body, b);
                    body_folded = true;
                }
                find(term, b, work)
            };
            match (in_title, in_body) {
                (Some((ts, tr)), Some((bs, _))) if ts * 2 >= bs => {
                    m.score += ts * 2;
                    m.title.extend(tr);
                }
                (Some((ts, tr)), None) => {
                    m.score += ts * 2;
                    m.title.extend(tr);
                }
                (_, Some((bs, br))) => {
                    m.score += bs;
                    if m.body_at.is_none() {
                        m.body_at = br.first().map(|r| r.0);
                    }
                }
                (None, None) => return None,
            }
        }
        m.title = joined(m.title);
        Some(m)
    }

    /// Where the query's words are in `text` (a snippet of a body), as char ranges.
    pub fn marks(&self, text: &str) -> Vec<(usize, usize)> {
        let mut scratch = self.scratch.borrow_mut();
        let Scratch { body: t, work, .. } = &mut *scratch;
        fold_into(text, t);
        joined(self.terms.iter().filter_map(|term| find(term, t, work)).flat_map(|(_, r)| r).collect())
    }
}

/// About `width` chars of the line of `body` holding char `at`, with an ellipsis where
/// it was cut.
pub fn snippet(body: &str, at: usize, width: usize) -> String {
    // Only the line it's on is taken apart into chars, not the whole body.
    let byte = body.char_indices().nth(at).map_or(body.len(), |(b, _)| b);
    let line_start = body[..byte].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[byte..].find('\n').map_or(body.len(), |i| byte + i);
    let line: Vec<char> = body[line_start..line_end].chars().collect();
    let at = body[line_start..byte].chars().count();
    // Lead in with a little context, beginning at a word.
    let mut start = if at > width / 2 { at - width / 4 } else { 0 };
    while start > 0 && start < at && is_word(line[start - 1]) {
        start += 1;
    }
    let end = (start + width).min(line.len());
    let text: String = line[start..end].iter().collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 { format!("\u{2026}{text}") } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(q: &str, title: &str) -> bool {
        Query::new(q).score(title, "").is_some()
    }

    #[test]
    fn matches_forgivingly() {
        assert!(hit("rel", "Release 2.4 checklist"));
        assert!(hit("lease", "Release 2.4 checklist"));
        assert!(hit("check release", "Release 2.4 checklist"));
        assert!(hit("relchk", "Release checklist"));
        assert!(hit("relaese", "Release checklist"));
        assert!(hit("chekclist", "Release checklist"));
        assert!(hit("cafe", "Café notes"));
        assert!(hit("CAFÉ", "cafe notes"));
        assert!(hit("メモ", "買い物メモ"));
        assert!(!hit("xyz", "Release checklist"));
        assert!(!hit("release banana", "Release checklist"));
        // Short words aren't stretched into gaps or typos.
        assert!(!hit("and", "a new day"));
        assert!(!hit("test", "the best"));
        // Three-letter abbreviations, inside one word only.
        assert!(hit("chk", "Release checklist"));
        assert!(!hit("rcl", "Release checklist"));
        assert_eq!(Query::new("rel ease").score("Release", "").unwrap().title, vec![(0, 7)]);
    }

    #[test]
    fn ranks_and_marks() {
        let q = Query::new("rel");
        let title = q.score("Release plan", "").unwrap();
        let body = q.score("Plan", "Release it").unwrap();
        assert!(title.score > body.score);
        assert_eq!(title.title, vec![(0, 3)]);
        assert_eq!(body.body_at, Some(0));
        assert_eq!(Query::new("rel it").marks("Release it now"), vec![(0, 3), (8, 10)]);
        let whole = Query::new("plan").score("Plan", "").unwrap();
        let start = Query::new("plan").score("Planning", "").unwrap();
        assert!(whole.score > start.score);
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("Changelog\nSmoke test staging\nPost", 20, 40), "Smoke test staging");
        let s = snippet("one two three four five six seven eight nine ten", 30, 20);
        assert!(s.starts_with('\u{2026}') && s.contains("seven"), "{s}");
    }

    /// The search as it was before it reused its buffers, to check that it finds and
    /// ranks exactly as before.
    mod before {
        use super::super::Match;

        fn fold(c: char) -> char {
            let c = c.to_lowercase().next().unwrap_or(c);
            match c {
                'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
                'ç' | 'ć' | 'č' => 'c',
                'ď' => 'd',
                'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
                'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' => 'i',
                'ł' => 'l',
                'ñ' | 'ń' | 'ň' => 'n',
                'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
                'ř' => 'r',
                'ś' | 'š' | 'ş' => 's',
                'ť' => 't',
                'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' => 'u',
                'ý' | 'ÿ' => 'y',
                'ź' | 'ż' | 'ž' => 'z',
                c => c,
            }
        }

        fn fold_all(s: &str) -> Vec<char> {
            s.chars().map(fold).collect()
        }

        fn is_word(c: char) -> bool {
            c.is_alphanumeric()
        }

        fn word_start(t: &[char], i: usize) -> bool {
            i == 0 || !is_word(t[i - 1])
        }

        /// The best way `term` turns up in `t`: (score, matched char ranges).
        fn find(term: &[char], t: &[char]) -> Option<(i32, Vec<(usize, usize)>)> {
            let n = term.len();
            if n == 0 || t.len() < n.min(t.len() + 1) {
                return None;
            }
            // Exact, at a word start (a whole word does better still), else anywhere.
            let mut inside = None;
            if t.len() >= n {
                for i in 0..=t.len() - n {
                    if t[i..i + n] == *term {
                        if word_start(t, i) {
                            let whole = i + n == t.len() || !is_word(t[i + n]);
                            return Some((if whole { 120 } else { 100 }, vec![(i, i + n)]));
                        }
                        inside.get_or_insert(i);
                    }
                }
            }
            if let Some(i) = inside {
                return Some((70, vec![(i, i + n)]));
            }
            if n < 2 || !term.iter().all(|c| is_word(*c)) {
                return None;
            }
            // In order, starting a word, close together and in a few runs: at most one run per
            // two letters, so "test" doesn't turn up as t-e-st in "the best".
            if n >= 4 {
                let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
                for start in 0..t.len() {
                    if t[start] != term[0] || !word_start(t, start) {
                        continue;
                    }
                    let (mut k, mut i, mut hits) = (1, start + 1, vec![start]);
                    while k < n && i < t.len() && i - start <= n * 3 {
                        if t[i] == term[k] {
                            hits.push(i);
                            k += 1;
                        }
                        i += 1;
                    }
                    if k == n && merge(&hits).len() <= n / 2 {
                        let span = hits[n - 1] - start + 1;
                        // Each letter landing on a word start counts for something.
                        let starts = hits.iter().filter(|&&h| word_start(t, h)).count() as i32;
                        let score = 50 - (span - n) as i32 * 2 + starts * 4;
                        if best.as_ref().is_none_or(|b| score > b.0) {
                            best = Some((score, merge(&hits)));
                        }
                    }
                }
                if let Some(b) = best.filter(|b| b.0 > 20) {
                    return Some(b);
                }
            }
            // Three letters in order inside one word, from its start: "chk" for "checklist".
            if n == 3 {
                for start in 0..t.len() {
                    if t[start] != term[0] || !word_start(t, start) {
                        continue;
                    }
                    let end = (start..t.len()).find(|&j| !is_word(t[j])).unwrap_or(t.len());
                    let (mut k, mut hits) = (1, vec![start]);
                    for (i, &c) in t.iter().enumerate().take(end.min(start + 10)).skip(start + 1) {
                        if k < n && c == term[k] {
                            hits.push(i);
                            k += 1;
                        }
                    }
                    if k == n {
                        return Some((30, merge(&hits)));
                    }
                }
            }
            // A word with a typo or two, compared with the same length of its start. Only for
            // longer words, and the first letter has to be right: "test" isn't "best".
            if n >= 5 {
                let allowed = if n >= 8 { 2 } else { 1 };
                let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
                let mut i = 0;
                while i < t.len() {
                    if !is_word(t[i]) {
                        i += 1;
                        continue;
                    }
                    let end = (i..t.len()).find(|&j| !is_word(t[j])).unwrap_or(t.len());
                    let word = &t[i..end];
                    if word[0] != term[0] {
                        i = end;
                        continue;
                    }
                    for len in [n.saturating_sub(1), n, n + 1] {
                        if len == 0 || len > word.len() {
                            continue;
                        }
                        let d = distance(term, &word[..len]);
                        if d <= allowed {
                            let score = 40 - d as i32 * 12;
                            if best.as_ref().is_none_or(|b| score > b.0) {
                                best = Some((score, vec![(i, i + len)]));
                            }
                        }
                    }
                    i = end;
                }
                return best;
            }
            None
        }

        /// Ranges in order, those that touch or overlap made one: two words matching "Rel" and
        /// "ease" mark "Release" once, not as two pills side by side.
        fn joined(mut r: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
            r.sort_unstable();
            let mut out: Vec<(usize, usize)> = Vec::new();
            for (a, b) in r {
                match out.last_mut() {
                    Some(last) if a <= last.1 => last.1 = last.1.max(b),
                    _ => out.push((a, b)),
                }
            }
            out
        }

        /// Neighbouring indices as ranges: [3, 4, 5, 9] -> [(3, 6), (9, 10)].
        fn merge(hits: &[usize]) -> Vec<(usize, usize)> {
            let mut out: Vec<(usize, usize)> = Vec::new();
            for &h in hits {
                match out.last_mut() {
                    Some(r) if r.1 == h => r.1 = h + 1,
                    _ => out.push((h, h + 1)),
                }
            }
            out
        }

        /// Edit distance, counting a swap of neighbours as one edit.
        fn distance(a: &[char], b: &[char]) -> usize {
            let (n, m) = (a.len(), b.len());
            let mut d = vec![vec![0usize; m + 1]; n + 1];
            for (i, row) in d.iter_mut().enumerate() {
                row[0] = i;
            }
            for (j, cell) in d[0].iter_mut().enumerate() {
                *cell = j;
            }
            for i in 1..=n {
                for j in 1..=m {
                    let cost = usize::from(a[i - 1] != b[j - 1]);
                    d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
                    if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                        d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
                    }
                }
            }
            d[n][m]
        }


        pub fn score(q: &str, title: &str, body: &str) -> Option<Match> {
            let terms: Vec<Vec<char>> = q.split_whitespace().map(fold_all).filter(|t| !t.is_empty()).collect();
            let (t, b) = (fold_all(title), fold_all(body));
            let mut m = Match::default();
            for term in &terms {
                match (find(term, &t), find(term, &b)) {
                    (Some((ts, tr)), Some((bs, _))) if ts * 2 >= bs => {
                        m.score += ts * 2;
                        m.title.extend(tr);
                    }
                    (Some((ts, tr)), None) => {
                        m.score += ts * 2;
                        m.title.extend(tr);
                    }
                    (_, Some((bs, br))) => {
                        m.score += bs;
                        if m.body_at.is_none() {
                            m.body_at = br.first().map(|r| r.0);
                        }
                    }
                    (None, None) => return None,
                }
            }
            m.title = joined(m.title);
            Some(m)
        }

        pub fn snippet(body: &str, at: usize, width: usize) -> String {
            let chars: Vec<char> = body.chars().collect();
            let at = at.min(chars.len());
            let line_start = chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
            let line_end = chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| at + i);
            let line = &chars[line_start..line_end];
            let at = at - line_start;
            let mut start = if at > width / 2 { at - width / 4 } else { 0 };
            while start > 0 && start < at && is_word(line[start - 1]) {
                start += 1;
            }
            let end = (start + width).min(line.len());
            let text: String = line[start..end].iter().collect();
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if start > 0 { format!("\u{2026}{text}") } else { text }
        }

        pub fn marks(q: &str, text: &str) -> Vec<(usize, usize)> {
            let terms: Vec<Vec<char>> = q.split_whitespace().map(fold_all).filter(|t| !t.is_empty()).collect();
            let t = fold_all(text);
            joined(terms.iter().filter_map(|term| find(term, &t)).flat_map(|(_, r)| r).collect())
        }
    }

    /// Every query, over notes of mixed words, accents, scripts and punctuation, scores,
    /// marks and places its snippet exactly as the search did before it reused buffers.
    #[test]
    fn same_results_as_before() {
        const WORDS: [&str; 30] = [
            "release", "checklist", "Release", "passport", "hotel", "café", "Café", "résumé", "naïve", "ŁÓDŹ", "İstanbul", "straße", "买东西", "メモ", "relay", "relevant", "check-in", "re-lease", "chk", "rel",
            "a", "the", "best", "test", "2.4", "(draft)", "e-mail", "chekclist", "relaese", "Ǆemal",
        ];
        let seed = std::cell::Cell::new(11u32);
        let pick = |k: usize| {
            seed.set(seed.get().wrapping_mul(1_103_515_245).wrapping_add(12_345));
            (seed.get() >> 16) as usize % k
        };
        let texts: Vec<(String, String)> = (0..120)
            .map(|_| {
                let line = |n: usize| (0..n).map(|_| WORDS[pick(WORDS.len())]).collect::<Vec<_>>().join(if pick(4) == 0 { "," } else { " " });
                let title = line(1 + pick(4));
                let body = (0..pick(6)).map(|_| line(pick(12))).collect::<Vec<_>>().join("\n");
                (title, body)
            })
            .collect();
        let queries = [
            "rel", "release", "relaese", "chekclist", "relchk", "chk", "rcl", "cafe", "CAFÉ", "lodz", "istanbul", "strasse", "メモ", "東西", "test", "best", "and", "a", "e", "2.4", "draft", "check in", "rel ease",
            "relevantt", "passprot", "re lease chk", "x", "zzzz", "hotel passport", "ǆemal", "re-lease", "rcpth", "releasechecklistpassporthotel", "rrrrrrrrrrrrrrrrrrrrrr",
        ];
        for q in queries {
            let query = Query::new(q);
            for (title, body) in &texts {
                let (now, then) = (query.score(title, body), before::score(q, title, body));
                assert_eq!(now.as_ref().map(|m| (m.score, &m.title, m.body_at)), then.as_ref().map(|m| (m.score, &m.title, m.body_at)), "{q:?} in {title:?} / {body:?}");
                assert_eq!(query.marks(body), before::marks(q, body), "{q:?} marks in {body:?}");
            }
        }
        for (_, body) in &texts {
            for at in (0..body.chars().count() + 2).step_by(7) {
                for width in [20, 80] {
                    assert_eq!(snippet(body, at, width), before::snippet(body, at, width), "snippet at {at} of {body:?}");
                }
            }
        }
    }
}
