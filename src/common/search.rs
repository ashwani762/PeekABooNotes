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

/// One prepared query.
pub struct Query {
    terms: Vec<Vec<char>>,
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

impl Query {
    pub fn new(q: &str) -> Query {
        Query { terms: q.split_whitespace().map(fold_all).filter(|t| !t.is_empty()).collect() }
    }

    /// How well a note with this title and body matches; None when some word of the
    /// query isn't in it.
    pub fn score(&self, title: &str, body: &str) -> Option<Match> {
        let (t, b) = (fold_all(title), fold_all(body));
        let mut m = Match::default();
        for term in &self.terms {
            let in_title = find(term, &t);
            let in_body = find(term, &b);
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
        m.title.sort_unstable();
        Some(m)
    }

    /// Where the query's words are in `text` (a snippet of a body), as char ranges.
    pub fn marks(&self, text: &str) -> Vec<(usize, usize)> {
        let t = fold_all(text);
        let mut out: Vec<(usize, usize)> = self.terms.iter().filter_map(|term| find(term, &t)).flat_map(|(_, r)| r).collect();
        out.sort_unstable();
        out
    }
}

/// About `width` chars of the line of `body` holding char `at`, with an ellipsis where
/// it was cut.
pub fn snippet(body: &str, at: usize, width: usize) -> String {
    let chars: Vec<char> = body.chars().collect();
    let at = at.min(chars.len());
    let line_start = chars[..at].iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1);
    let line_end = chars[at..].iter().position(|&c| c == '\n').map_or(chars.len(), |i| at + i);
    let line = &chars[line_start..line_end];
    let at = at - line_start;
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
}
