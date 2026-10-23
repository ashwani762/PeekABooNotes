// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Interface text in every language: Android-style string resources.
//!
//! Each language is one `strings.<tag>.xml` file (`strings.en.xml`, `strings.de.xml`,
//! `strings.pt-BR.xml`, ...):
//!
//! ```xml
//! <resources>
//!     <string name="menu_new_note">New Note</string>
//!     <string name="window_all_notes">All Notes — %1$s</string>
//!     <plurals name="notes_count">
//!         <item quantity="one">%d note</item>
//!         <item quantity="other">%d notes</item>
//!     </plurals>
//! </resources>
//! ```
//!
//! Placeholders follow Android: `%s` / `%d` in order, or `%1$s` / `%2$d` by position;
//! `%%` is a percent sign. Plural items use the CLDR categories (zero, one, two, few,
//! many, other). English is the reference: any key missing from a translation falls back
//! to it.
//!
//! The built-in languages are compiled in. A `lang` folder next to the exe or in the
//! notes folder can add languages or override built-in strings without a rebuild. The
//! language follows Windows' preferred UI languages unless Settings picks one.

pub mod time;
mod xml;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;

/// Built-in languages: (tag, file).
const BUILTIN: &[(&str, &str)] = &[
    ("en", include_str!("strings/strings.en.xml")),
    ("de", include_str!("strings/strings.de.xml")),
    ("es", include_str!("strings/strings.es.xml")),
    ("fr", include_str!("strings/strings.fr.xml")),
    ("hi", include_str!("strings/strings.hi.xml")),
    ("ja", include_str!("strings/strings.ja.xml")),
];

#[derive(Default)]
struct Table {
    strings: HashMap<String, &'static str>,
    plurals: HashMap<String, HashMap<String, &'static str>>,
}

impl Table {
    fn merge(&mut self, res: xml::Resources) {
        // Strings live for the whole run (a language switch leaks a few KB, once).
        for (k, v) in res.strings {
            self.strings.insert(k, Box::leak(v.into_boxed_str()));
        }
        for (k, items) in res.plurals {
            let e = self.plurals.entry(k).or_default();
            for (q, v) in items {
                e.insert(q, Box::leak(v.into_boxed_str()));
            }
        }
    }
}

struct State {
    tag: String,
    current: Table,
    english: Table,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static LANGUAGES: RefCell<Option<Vec<Language>>> = const { RefCell::new(None) };
}

/// A language that can be chosen: its tag and its own name for itself.
#[derive(Clone)]
pub struct Language {
    pub tag: String,
    pub name: String,
}

/// Language files from the `lang` folders: (tag, contents).
fn external() -> Vec<(String, String)> {
    let mut dirs = Vec::new();
    if let Some(d) = crate::common::util::exe_path().parent() {
        dirs.push(d.join("lang"));
    }
    dirs.push(crate::common::util::data_dir().join("lang"));
    let mut out = Vec::new();
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for f in rd.flatten() {
            let name = f.file_name().to_string_lossy().to_string();
            if let Some(tag) = name.strip_prefix("strings.").and_then(|n| n.strip_suffix(".xml"))
                && let Ok(text) = std::fs::read_to_string(f.path()) {
                    out.push((tag.to_string(), text));
                }
        }
    }
    out
}

fn load(tag: &str) -> Table {
    let mut t = Table::default();
    for (b, text) in BUILTIN {
        if b.eq_ignore_ascii_case(tag) {
            t.merge(xml::parse(text));
        }
    }
    for (e, text) in external() {
        if e.eq_ignore_ascii_case(tag) {
            t.merge(xml::parse(&text));
        }
    }
    t
}

/// Every language available, built-in or from a `lang` folder (read once, at `init`).
pub fn languages() -> Vec<Language> {
    if let Some(l) = LANGUAGES.with(|c| c.borrow().clone()) {
        return l;
    }
    let l = scan_languages();
    LANGUAGES.with(|c| *c.borrow_mut() = Some(l.clone()));
    l
}

fn scan_languages() -> Vec<Language> {
    let mut tags: Vec<String> = BUILTIN.iter().map(|(t, _)| t.to_string()).collect();
    for (t, _) in external() {
        if !tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            tags.push(t);
        }
    }
    tags.into_iter()
        .map(|tag| {
            let name = load(&tag).strings.get("language_name").map(|s| s.to_string()).unwrap_or_else(|| tag.clone());
            Language { tag, name }
        })
        .collect()
}

/// The best available language for `choice` ("" = follow Windows): the exact tag, else
/// its base language ("pt-BR" -> "pt"), else English.
fn resolve(choice: &str) -> String {
    let have: Vec<String> = languages().into_iter().map(|l| l.tag).collect();
    let pick = |want: &str| -> Option<String> {
        have.iter().find(|h| h.eq_ignore_ascii_case(want)).cloned().or_else(|| {
            let base = want.split('-').next().unwrap_or(want);
            have.iter().find(|h| h.eq_ignore_ascii_case(base)).cloned()
        })
    };
    if !choice.is_empty()
        && let Some(t) = pick(choice) {
            return t;
        }
    crate::os::preferred_languages().iter().find_map(|w| pick(w)).unwrap_or_else(|| "en".into())
}

/// Load the interface language: a tag from Settings, or "" to follow Windows.
pub fn init(choice: &str) {
    LANGUAGES.with(|c| *c.borrow_mut() = None);
    let tag = resolve(choice);
    let current = load(&tag);
    let english = load("en");
    STATE.with(|s| *s.borrow_mut() = Some(State { tag, current, english }));
}

/// The language in use (e.g. "de").
pub fn tag() -> String {
    STATE.with(|s| s.borrow().as_ref().map(|s| s.tag.clone())).unwrap_or_else(|| "en".into())
}

/// The language tag as a NUL-terminated UTF-16 string (for Windows text APIs).
pub fn tag_wide() -> Vec<u16> {
    tag().encode_utf16().chain(Some(0)).collect()
}

/// A string by key, falling back to English, then to the key itself (remembered, so a
/// missing key costs one allocation, not one per frame).
pub fn get(key: &str) -> &'static str {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(s) = s.as_mut() else { return Box::leak(key.to_string().into_boxed_str()) as &'static str };
        if let Some(v) = s.current.strings.get(key).or_else(|| s.english.strings.get(key)) {
            return *v;
        }
        let v: &'static str = Box::leak(key.to_string().into_boxed_str());
        s.current.strings.insert(key.to_string(), v);
        v
    })
}

/// A string with its placeholders filled.
pub fn format(key: &str, args: &[&dyn Display]) -> String {
    fill(get(key), args)
}

/// A plural string for `n` (also the first argument), in the current language's rules.
pub fn plural(key: &str, n: i64, args: &[&dyn Display]) -> String {
    let lang = tag();
    let cat = plural_category(&lang, n);
    let text = STATE.with(|s| {
        let s = s.borrow();
        let s = s.as_ref()?;
        let from = |t: &Table, c: &str| t.plurals.get(key).and_then(|m| m.get(c).or_else(|| m.get("other"))).copied();
        from(&s.current, cat).or_else(|| from(&s.english, plural_category("en", n)))
    });
    fill(text.unwrap_or(key), args)
}

/// CLDR cardinal plural category for the languages we know; "other" elsewhere.
pub fn plural_category(lang: &str, n: i64) -> &'static str {
    let base = lang.split('-').next().unwrap_or(lang).to_ascii_lowercase();
    match base.as_str() {
        // No plural forms.
        "ja" | "zh" | "ko" | "vi" | "th" | "id" | "ms" => "other",
        // 0 and 1 are singular.
        "fr" | "hi" | "pt" | "bn" | "fa" => {
            if n == 0 || n == 1 { "one" } else { "other" }
        }
        "ru" | "uk" => {
            let (m10, m100) = (n % 10, n % 100);
            if m10 == 1 && m100 != 11 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "pl" => {
            let (m10, m100) = (n % 10, n % 100);
            if n == 1 {
                "one"
            } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
                "few"
            } else {
                "many"
            }
        }
        "ar" => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            _ if (3..=10).contains(&(n % 100)) => "few",
            _ if (11..=99).contains(&(n % 100)) => "many",
            _ => "other",
        },
        _ => {
            if n == 1 { "one" } else { "other" }
        }
    }
}

/// Fill Android-style placeholders: `%s`, `%d` in order; `%1$s`, `%2$d` by position.
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut next = 0usize;
    let b = template.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 1 < b.len() {
            if b[i + 1] == b'%' {
                out.push('%');
                i += 2;
                continue;
            }
            // %N$s
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (idx, after) = if j > i + 1 && j < b.len() && b[j] == b'$' {
                (template[i + 1..j].parse::<usize>().ok().map(|n| n.saturating_sub(1)), j + 1)
            } else {
                (None, i + 1)
            };
            if after < b.len() && matches!(b[after], b's' | b'd') {
                let k = idx.unwrap_or_else(|| {
                    next += 1;
                    next - 1
                });
                if let Some(a) = args.get(k) {
                    out.push_str(&a.to_string());
                }
                i = after + 1;
                continue;
            }
        }
        let ch = template[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A key combination in this language's key names, e.g. ["ctrl", "alt", "N"] ->
/// "Strg+Alt+N" in German. Lowercase names are looked up as `key_<name>`.
pub fn keys(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|p| if p.chars().all(|c| c.is_ascii_lowercase()) { get(&format!("key_{p}")).to_string() } else { p.to_string() })
        .collect::<Vec<_>>()
        .join("+")
}

/// `t!("key")` -> &'static str; `t!("key", a, b)` -> String with %1$s, %2$s filled.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::common::i18n::get($key)
    };
    ($key:literal, $($arg:expr),+ $(,)?) => {
        $crate::common::i18n::format($key, &[$(&$arg as &dyn ::std::fmt::Display),+])
    };
}

/// `tn!("key", n)` -> the plural form for n with %d = n; extra arguments follow it.
#[macro_export]
macro_rules! tn {
    ($key:literal, $n:expr $(, $arg:expr)* $(,)?) => {{
        let n = $n;
        $crate::common::i18n::plural($key, n as i64, &[&n as &dyn ::std::fmt::Display $(, &$arg as &dyn ::std::fmt::Display)*])
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(text: &str) -> (Vec<String>, Vec<String>) {
        let r = xml::parse(text);
        let mut s: Vec<String> = r.strings.into_keys().collect();
        let mut p: Vec<String> = r.plurals.into_keys().collect();
        s.sort();
        p.sort();
        (s, p)
    }

    /// Placeholders a string uses, normalised to positions: "%1$s %d" -> [1, 2].
    fn placeholders(s: &str) -> Vec<usize> {
        let mut v = Vec::new();
        let mut next = 0;
        let b = s.as_bytes();
        let mut i = 0;
        while i + 1 < b.len() {
            if b[i] == b'%' {
                if b[i + 1] == b'%' {
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j < b.len() && b[j] == b'$' && j + 1 < b.len() && matches!(b[j + 1], b's' | b'd') {
                    v.push(s[i + 1..j].parse().unwrap());
                } else if matches!(b[i + 1], b's' | b'd') {
                    next += 1;
                    v.push(next);
                }
            }
            i += 1;
        }
        v.sort();
        v.dedup();
        v
    }

    #[test]
    fn every_language_matches_english() {
        let en = xml::parse(BUILTIN[0].1);
        let (es, ep) = keys_of(BUILTIN[0].1);
        for (tag, text) in &BUILTIN[1..] {
            let (s, p) = keys_of(text);
            assert_eq!(s, es, "{tag}: string keys differ from English");
            assert_eq!(p, ep, "{tag}: plural keys differ from English");
            let r = xml::parse(text);
            for (k, v) in &r.strings {
                assert_eq!(placeholders(v), placeholders(&en.strings[k]), "{tag}: placeholders of {k}");
            }
        }
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(fill("%1$s of %2$s", &[&"a", &"b"]), "a of b");
        assert_eq!(fill("%2$s, %1$s", &[&"a", &"b"]), "b, a");
        assert_eq!(fill("%d notes, 100%%", &[&3]), "3 notes, 100%");
        assert_eq!(fill("%s and %s", &[&"x", &"y"]), "x and y");
    }

    #[test]
    fn plural_rules() {
        assert_eq!(plural_category("en", 1), "one");
        assert_eq!(plural_category("en", 0), "other");
        assert_eq!(plural_category("fr", 0), "one");
        assert_eq!(plural_category("ja", 1), "other");
        assert_eq!(plural_category("ru", 22), "few");
    }
}
