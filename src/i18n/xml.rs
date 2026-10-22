// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Just enough XML for Android-style string resources: `<string name="...">` and
//! `<plurals name="..."><item quantity="...">`, with comments, CDATA, the standard
//! entities, numeric character references and Android's backslash escapes.

use std::collections::HashMap;

#[derive(Default)]
pub struct Resources {
    pub strings: HashMap<String, String>,
    pub plurals: HashMap<String, HashMap<String, String>>,
}

/// Parse a resources file. Malformed parts are skipped rather than fatal, so a broken
/// translation file can never stop the app from starting.
pub fn parse(text: &str) -> Resources {
    let mut r = Resources::default();
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |e| &after[e + 3..]);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        if rest.starts_with("<string ") || rest.starts_with("<string>") {
            let Some((attrs, body, tail)) = element(rest, "string") else { break };
            if let Some(name) = attr(attrs, "name") {
                r.strings.insert(name, value(body));
            }
            rest = tail;
            continue;
        }
        if rest.starts_with("<plurals") {
            let Some((attrs, body, tail)) = element(rest, "plurals") else { break };
            if let Some(name) = attr(attrs, "name") {
                let mut items = HashMap::new();
                let mut b = body;
                while let Some(i) = b.find("<item") {
                    let Some((a, v, t)) = element(&b[i..], "item") else { break };
                    if let Some(q) = attr(a, "quantity") {
                        items.insert(q, value(v));
                    }
                    b = t;
                }
                r.plurals.insert(name, items);
            }
            rest = tail;
            continue;
        }
        rest = &rest[1..];
    }
    r
}

/// `<tag attrs>body</tag>` at the start of `s`: (attrs, body, rest after the element).
fn element<'a>(s: &'a str, tag: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let open_end = s.find('>')?;
    let attrs = &s[1 + tag.len()..open_end];
    if attrs.trim_end().ends_with('/') {
        return Some((attrs.trim_end().trim_end_matches('/'), "", &s[open_end + 1..]));
    }
    let close = format!("</{tag}>");
    let body_start = open_end + 1;
    let end = s[body_start..].find(&close)? + body_start;
    Some((attrs, &s[body_start..end], &s[end + close.len()..]))
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let key = format!("{name}=");
    let i = attrs.find(&key)? + key.len();
    let q = attrs[i..].chars().next()?;
    if q != '"' && q != '\'' {
        return None;
    }
    let rest = &attrs[i + 1..];
    let end = rest.find(q)?;
    Some(entities(&rest[..end]))
}

/// Element text: CDATA kept verbatim, entities decoded, Android escapes applied, and
/// surrounding quotes (Android's way to keep leading/trailing spaces) removed.
fn value(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(i) = rest.find("<![CDATA[") {
        out.push_str(&escapes(&entities(&rest[..i])));
        let after = &rest[i + 9..];
        let e = after.find("]]>").unwrap_or(after.len());
        out.push_str(&after[..e]);
        rest = after.get(e + 3..).unwrap_or("");
    }
    out.push_str(&escapes(&entities(rest)));
    let t = out.trim();
    let t = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(t);
    t.to_string()
}

fn entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(e) = after.find(';') else {
            out.push('&');
            rest = after;
            continue;
        };
        let name = &after[..e];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if name.starts_with("#x") || name.starts_with("#X") => u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32),
            _ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
        }
        rest = &after[e + 1..];
    }
    out.push_str(rest);
    out
}

fn escapes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = it.by_ref().take(4).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_strings_and_plurals() {
        let r = parse(r#"<?xml version="1.0" encoding="utf-8"?>
            <!-- comment <string name="x">no</string> -->
            <resources>
              <string name="a">Hello &amp; welcome</string>
              <string name="b">It\'s %1$s</string>
              <string name="c">"  spaced  "</string>
              <string name="d"><![CDATA[<b>raw</b>]]></string>
              <plurals name="p">
                <item quantity="one">%d note</item>
                <item quantity="other">%d notes</item>
              </plurals>
            </resources>"#);
        assert_eq!(r.strings["a"], "Hello & welcome");
        assert_eq!(r.strings["b"], "It's %1$s");
        assert_eq!(r.strings["c"], "  spaced  ");
        assert_eq!(r.strings["d"], "<b>raw</b>");
        assert!(!r.strings.contains_key("x"));
        assert_eq!(r.plurals["p"]["other"], "%d notes");
    }
}
