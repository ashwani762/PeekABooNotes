// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding emoji in text: each one as the run of UTF-16 units it takes, with its skin
//! tone, variation selector, joined parts (👨‍👩‍👧) or flag pair, so it is drawn whole.

/// Pictographs shown as emoji by default (outside the big emoji blocks, most symbols
/// only become emoji with U+FE0F after them).
fn emoji_by_default(c: u32) -> bool {
    matches!(c,
        0x1F000..=0x1FAFF
        | 0x231A | 0x231B | 0x23E9..=0x23EC | 0x23F0 | 0x23F3 | 0x25FD | 0x25FE
        | 0x2614 | 0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB
        | 0x26BD | 0x26BE | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3
        | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E
        | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C
        | 0x2B50 | 0x2B55)
}

/// Symbols that are emoji when followed by U+FE0F (❤️, ✔️, ☀️).
fn emoji_with_selector(c: u32) -> bool {
    matches!(c, 0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x2194..=0x21AA | 0x2300..=0x23FF | 0x24C2 | 0x25AA..=0x25FE | 0x2600..=0x27BF | 0x2934 | 0x2935 | 0x2B05..=0x2B55 | 0x3030 | 0x303D | 0x3297 | 0x3299)
}

fn is_modifier(c: u32) -> bool {
    matches!(c, 0x1F3FB..=0x1F3FF | 0xFE0F | 0x20E3 | 0xE0020..=0xE007F)
}

/// (start, end) in UTF-16 units of each emoji in `t`. `skip` are characters never to
/// count (the checklist's ☐ and ☑, which are drawn as circles).
pub fn find(t: &[u16], skip: &[u16]) -> Vec<(usize, usize)> {
    // Code points with their UTF-16 offsets.
    let cps: Vec<(usize, u32)> = {
        let mut v = Vec::new();
        let mut i = 0;
        for c in char::decode_utf16(t.iter().copied()) {
            let (cp, len) = match c {
                Ok(ch) => (ch as u32, ch.len_utf16()),
                Err(_) => (0xFFFD, 1),
            };
            v.push((i, cp));
            i += len;
        }
        v
    };
    let end_of = |k: usize| cps.get(k + 1).map_or(t.len(), |n| n.0);
    let mut out = Vec::new();
    let mut k = 0;
    while k < cps.len() {
        let (start, c) = cps[k];
        let next = cps.get(k + 1).map(|n| n.1);
        let keycap = (c == u32::from(b'#') || c == u32::from(b'*') || (0x30..=0x39).contains(&c)) && next == Some(0xFE0F) && cps.get(k + 2).map(|n| n.1) == Some(0x20E3);
        let flag = (0x1F1E6..=0x1F1FF).contains(&c) && next.is_some_and(|n| (0x1F1E6..=0x1F1FF).contains(&n));
        let starts = (emoji_by_default(c) || (emoji_with_selector(c) && next == Some(0xFE0F)) || keycap) && !skip.contains(&(c as u16)) && !(0x1F3FB..=0x1F3FF).contains(&c);
        if !starts && !flag {
            k += 1;
            continue;
        }
        let mut j = k;
        if flag {
            j += 1;
        } else {
            loop {
                // Modifiers, then a joiner and the next part.
                while cps.get(j + 1).is_some_and(|n| is_modifier(n.1)) {
                    j += 1;
                }
                if cps.get(j + 1).map(|n| n.1) == Some(0x200D) && cps.get(j + 2).is_some() {
                    j += 2;
                    continue;
                }
                break;
            }
        }
        out.push((start, end_of(j)));
        k = j + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(s: &str) -> Vec<String> {
        let t: Vec<u16> = s.encode_utf16().collect();
        find(&t, &[0x2610, 0x2611]).into_iter().map(|(a, b)| String::from_utf16_lossy(&t[a..b])).collect()
    }

    #[test]
    fn finds_whole_emoji() {
        assert_eq!(runs("Beach 🏖️ trip 😎"), vec!["🏖️", "😎"]);
        assert_eq!(runs("thumbs 👍🏽 ok"), vec!["👍🏽"]);
        assert_eq!(runs("family 👨‍👩‍👧 here"), vec!["👨‍👩‍👧"]);
        assert_eq!(runs("flag 🇮🇳 ok"), vec!["🇮🇳"]);
        assert_eq!(runs("coffee ☕ and love ❤️"), vec!["☕", "❤️"]);
        assert_eq!(runs("key 1️⃣"), vec!["1️⃣"]);
        // Not emoji: plain symbols, digits, checklist boxes, the arrows people type.
        assert!(runs("☐ todo ☑ done ✓ → 1 2 © ♥").is_empty());
    }
}
