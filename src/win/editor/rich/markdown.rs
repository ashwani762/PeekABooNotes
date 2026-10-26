// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain-text Markdown in and out (titles, search, previews, export, import).

use super::*;

/// Convert the editor's content (already loaded into `h`) to Markdown.
/// With `inline` false only list/task markers are produced (cheap: one query per paragraph).
pub fn to_markdown(h: HWND, inline: bool) -> String {
    let t = text(h);
    let mut out = body_md(h, &t, 0, t.len(), inline);
    // Inline mode is for export: plain `-` lists in the light mode are what we store.
    if !inline {
        out = out.lines().map(light_task).collect::<Vec<_>>().join("\n");
    }
    // A line break within a paragraph (Shift+Enter) goes out as a line: never as the
    // control character RichEdit keeps it as.
    out.replace('\u{B}', "\n").trim_end().to_string()
}

/// What `body_md` wrote last.
#[derive(Clone, Copy, PartialEq)]
enum Last {
    Text,
    Empty,
    Block(Block),
}

/// The paragraphs and blocks from `a` to `b` (the whole note, or a quote's cell) as
/// Markdown, a line each and each ending in a newline (`inline`: with marks, links and
/// alignment).
///
/// Import takes one blank line on either side of a block as Markdown's way to set it
/// apart, not as an empty paragraph, so an empty paragraph next to a block goes out with
/// one more: a note goes out and comes back in with the empty lines it had.
fn body_md(h: HWND, t: &[u16], a: usize, b: usize, inline: bool) -> String {
    let mut out = String::new();
    let mut start = a;
    let mut numbers = [0usize; MAX_LEVEL as usize + 1];
    let mut last = None;
    // Where the last list item's line ends in `out`, while nothing but empty lines came
    // after it, and whether any did. Import takes one blank line between an item and plain
    // text as the gap Markdown needs there (`store::plain_line`), so empty lines there go
    // out with one more, as next to a block.
    let mut item_end: Option<usize> = None;
    let mut empty_since = false;
    while start <= b {
        // Two blocks always have an empty paragraph between them (RichEdit won't let two
        // tables touch): that's their gap, not a blank line.
        if t.get(start) == Some(&CR) && start >= 2 && t[start - 2] == ROW_END && t.get(start + 1) == Some(&ROW_START) {
            start += 1;
            continue;
        }
        // Quotes, code blocks and tables go out whole.
        if t.get(start) == Some(&ROW_START)
            && let Some((kind, rows)) = block_at(h, t, start as i32)
        {
            if inline && last == Some(Last::Empty) {
                out.push('\n');
            }
            // Two quotes (or tables) in a row would read back as one: a blank line keeps
            // them two (and import drops it again).
            if last == Some(Last::Block(kind)) && kind != Block::Code {
                out.push('\n');
            }
            last = Some(Last::Block(kind));
            item_end = None;
            out.push_str(&block_md(h, t, kind, &rows, inline));
            out.push('\n');
            // A quote under a list item is part of it: the list's count goes on after it.
            if kind != Block::Quote || quote_level(h, rows[0]) == 0 {
                numbers = [0; MAX_LEVEL as usize + 1];
            }
            start = rows.last().map_or(b, |r| r.end);
            // (At the end of a quote, the empty paragraph after a block is RichEdit's.)
            if start >= b {
                break;
            }
            continue;
        }
        let end = t[start..b].iter().position(|&c| c == CR).map_or(b, |i| start + i);
        if start == end {
            if inline && matches!(last, Some(Last::Block(_))) {
                out.push('\n');
            }
            last = Some(Last::Empty);
        } else {
            last = Some(Last::Text);
        }
        let line = para_md(h, t, start, end, inline, &mut numbers);
        if inline {
            use crate::common::store::{list_line, plain_line};
            if line.trim().is_empty() {
                empty_since = true;
            } else {
                if let Some(at) = item_end.take().filter(|_| empty_since && plain_line(&line)) {
                    out.insert(at, '\n');
                }
                empty_since = false;
            }
            out.push_str(&line);
            out.push('\n');
            if list_line(&line) {
                item_end = Some(out.len());
            }
        } else {
            out.push_str(&line);
            out.push('\n');
        }
        if end >= b {
            break;
        }
        start = end + 1;
    }
    out
}

/// A task line without the `- `, past any indent and quote marks.
fn light_task(l: &str) -> String {
    let i = l.find(|c: char| c != ' ' && c != '>').unwrap_or(l.len());
    let (lead, rest) = l.split_at(i);
    for (from, to) in [("- [ ]", "[ ]"), ("- [x]", "[x]")] {
        if let Some(r) = rest.strip_prefix(from)
            && (r.is_empty() || r.starts_with(' '))
        {
            return format!("{lead}{to}{r}");
        }
    }
    l.to_string()
}

/// The paragraph `t[start..end]` as one line of Markdown (`inline`: with marks and links;
/// otherwise only its list marker), counting numbered items in `numbers`.
fn para_md(h: HWND, t: &[u16], start: usize, end: usize, inline: bool, numbers: &mut [usize; MAX_LEVEL as usize + 1]) -> String {
    set_sel(h, start as i32, start as i32);
    let mut pf = ParaFormat2::new();
    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
    let num = pf.numbering;
    let task = start < end && is_task(t[start]);
    let level = if task || num != 0 { level_of(pf.start_indent, list_base(task)) as usize } else { 0 };
    let mut body_start = start;
    let mut prefix = "  ".repeat(level);
    match num {
        n if is_bullet(n) => prefix.push_str("- "),
        PFN_ARABIC | PFN_LCLETTER | PFN_LCROMAN => {
            // A list's first item has its own start (a list can start at 5).
            if numbers[level] == 0 {
                numbers[level] = pf.numbering_start.max(1) as usize - 1;
            }
            numbers[level] += 1;
            for n in numbers.iter_mut().skip(level + 1) {
                *n = 0;
            }
            prefix.push_str(&format!("{}. ", numbers[level]));
        }
        _ => *numbers = [0; MAX_LEVEL as usize + 1],
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
    // A centred or right-aligned line keeps its alignment on the way out (the title and
    // list items too).
    let align = if inline {
        match pf.alignment {
            PFA_CENTER => Some("center"),
            PFA_RIGHT => Some("right"),
            _ => None,
        }
    } else {
        None
    };
    let title = inline && start == 0 && prefix.is_empty() && end > 0;
    let mut body = if title {
        with_links(h, t, 0, end)
    } else if let Some(md) = inline.then(|| item_link(h, t, body_start)).flatten().and_then(|l| attachment_md(&l.2)) {
        md
    } else if inline {
        // A ticked item's strikethrough is how it looks, not part of its text (also
        // inside other marks: **~~bread~~**); tildes typed in it are.
        let done = task && t[start] == CHECKED;
        inline_md_without(h, t, body_start, end, if done { CFM_STRIKEOUT } else { 0 })
    } else if let Some(md) = light_attachment(h, t, body_start, end) {
        md
    } else {
        visible(t, body_start, end)
    };
    // Plain text that only looks like a list, heading or quote ("- not a list", typed and
    // reverted with Backspace, "  - indented", ">>> print") is escaped, so it comes back as
    // text, past any spaces it starts with; so is a table's first row that only looks like
    // one (`\| a | b |` over `| --- |`), a list item's text that would read as a quote under
    // it (`- \> text`), and a bullet's that would read as a checkbox (`- \[ ] text`).
    let looks_plain = prefix.is_empty() && !title && {
        let v = visible(t, body_start, end);
        crate::common::store::needs_escape(&v) || (v.trim_start().starts_with('|') && t.get(end) == Some(&CR) && separator(&visible(t, end + 1, para_end(t, end as i32 + 1) as usize)).is_some())
    };
    let looks_item = inline && num != 0 && !task && (body.starts_with('>') || (is_bullet(num) && reads_as_box(&body)));
    if looks_plain {
        body.insert(body.len() - body.trim_start().len(), '\\');
    } else if looks_item {
        body.insert(0, '\\');
    }
    match align {
        Some(a) if title => format!("<h1 align=\"{a}\">{body}</h1>"),
        _ if title => format!("# {body}"),
        Some(a) if !body.is_empty() => format!("{prefix}<p align=\"{a}\">{body}</p>"),
        _ => prefix + &body,
    }
}

/// Whether `s` starts as a checklist item's box is written (`[ ] `, `[x]`).
fn reads_as_box(s: &str) -> bool {
    ["[ ]", "[x]", "[X]"].iter().any(|m| s.strip_prefix(m).is_some_and(|r| r.is_empty() || r.starts_with(' ')))
}

/// The block starting at `cp` as the light text has it: (kind, how many lines, where it
/// ends in `t`).
pub fn light_block(h: HWND, t: &[u16], cp: usize) -> Option<(Block, usize, usize)> {
    let (kind, rows) = block_at(h, t, cp as i32)?;
    let lines = block_md(h, t, kind, &rows, false).lines().count();
    Some((kind, lines, rows.last().map_or(t.len(), |r| r.end)))
}

/// A quote, code block or table as Markdown (`inline`: with marks and links).
fn block_md(h: HWND, t: &[u16], kind: Block, rows: &[Row], inline: bool) -> String {
    // A cell's paragraphs: (start, end) in `t`.
    let paras = |a: usize, b: usize| {
        let mut out = Vec::new();
        let mut s = a;
        for i in a..=b {
            if i == b || t.get(i) == Some(&CR) {
                out.push((s, i));
                s = i + 1;
            }
        }
        out
    };
    let text_of = |a: usize, b: usize| if inline { inline_md(h, t, a, b) } else { visible(t, a, b) };
    let mut out = String::new();
    match kind {
        Block::Quote => {
            let (a, b) = cells(t, rows[0]).first().copied().unwrap_or((rows[0].start, rows[0].start));
            // What it holds as it would be outside (`> - item`, `> > nested`, a fenced code
            // block a line at a time), indented as the quote is.
            let indent = "  ".repeat(quote_level(h, rows[0]));
            let body = body_md(h, t, a, b, inline);
            let body = body.strip_suffix('\n').unwrap_or(&body);
            let lines: Vec<String> = body.split('\n').map(|l| format!("{indent}> {l}").trim_end().to_string()).collect();
            out.push_str(&lines.join("\n"));
        }
        Block::Code => {
            let (a, b) = cells(t, rows[0]).first().copied().unwrap_or((rows[0].start, rows[0].start));
            // Its language goes on the opening fence (```rust); its text is the rest.
            let tag = code_tag(h, t, a, b);
            let lang = tag.filter(|_| inline).map(|(s, e)| unmark_str(&t[s..e])).unwrap_or_default();
            // (Links in code, as opening a note in 0.6.0 made of addresses there, are their
            // text: their hidden instructions stay out.)
            let mut code: Vec<u16> = Vec::with_capacity(b - a);
            let mut i = tag.map_or(a, |(_, e)| e);
            for (at, name, _) in instructions(t, i, b) {
                code.extend_from_slice(&t[i..at]);
                i = name;
            }
            code.extend_from_slice(&t[i..b]);
            let lines: Vec<String> = code.split(|&c| c == CR).map(|l| unmark_str(l).replace('\u{B}', "\n")).collect();
            // A fence longer than any run of backticks starting a line of the code, so none
            // of them ends it early. (The light text keeps to three, as previews read it.)
            let most = lines.iter().flat_map(|l| l.split('\n')).map(|l| l.trim_start().bytes().take_while(|&c| c == b'`').count()).max().unwrap_or(0);
            let fence = "`".repeat(if inline && most >= 3 { most + 1 } else { 3 });
            out.push_str(&format!("{fence}{lang}\n"));
            for l in lines {
                out.push_str(&l);
                out.push('\n');
            }
            out.push_str(&fence);
        }
        Block::Table => {
            for (n, row) in rows.iter().enumerate() {
                let cs = cells(t, *row);
                let line: Vec<String> = cs
                    .iter()
                    .map(|&(a, b)| paras(a, b).into_iter().map(|(x, y)| text_of(x, y).replace('|', "\\|").replace('\u{B}', "<br>")).collect::<Vec<_>>().join("<br>"))
                    .collect();
                out.push_str(&format!("| {} |\n", line.join(" | ")));
                if n == 0 {
                    // Each column's alignment, from its header cell.
                    let seps: Vec<&str> = cs
                        .iter()
                        .map(|&(a, _)| {
                            set_sel(h, a as i32, a as i32);
                            let mut pf = ParaFormat2::new();
                            send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                            match pf.alignment {
                                PFA_CENTER => ":---:",
                                PFA_RIGHT => "---:",
                                _ => "---",
                            }
                        })
                        .collect();
                    out.push_str(&format!("| {} |\n", seps.join(" | ")));
                }
            }
            out.pop();
        }
    }
    out
}

/// A picture or file on its own line, as the light text keeps it, so previews can show
/// it: `![name](attachment:name)` or `[name](attachment:name)`. None for other lines.
fn light_attachment(h: HWND, t: &[u16], a: usize, b: usize) -> Option<String> {
    use crate::common::attach;
    // Only lines with a link at their end can be one: most lines are told apart cheaply.
    if !t[a..b].contains(&(ITEM_LINK_NAME as u16)) {
        return None;
    }
    let (_, _, url) = item_link(h, t, a)?;
    let name = attach::name_of(&url)?;
    Some(if attach::is_image(&name) { format!("![{name}]({url})") } else { format!("[{name}]({url})") })
}

/// Where a link that starts at a hidden instruction ends: (where its name starts, where
/// it ends, url), or None when `i` doesn't start one.
fn link_from(h: HWND, links: &[(usize, usize, String)], i: usize, b: usize) -> Option<(usize, usize, String)> {
    let (_, name, url) = links.iter().find(|l| l.0 == i)?;
    let end = link_at(h, *name as i32).map_or(b, |l| (l.1 as usize).min(b)).max(*name);
    Some((*name, end, url.clone()))
}

/// An attachment link as Markdown, pointing where export copies the file:
/// `![name](attachments/name)` for a picture, `[name](attachments/name)` otherwise.
/// None for other links.
fn attachment_md(url: &str) -> Option<String> {
    use crate::common::attach;
    let name = attach::name_of(url)?;
    let link = attach::export_link(&name);
    Some(if attach::is_image(&name) { format!("![{name}]({link})") } else { format!("[{name}]({link})") })
}

/// `t[a..b]` with links as `[name](url)` and no other marks (the title line).
fn with_links(h: HWND, t: &[u16], a: usize, b: usize) -> String {
    let links = instructions(t, a, b);
    let mut out = String::new();
    let mut i = a;
    while i < b {
        if let Some((name, end, url)) = link_from(h, &links, i, b) {
            let shown = unmark_str(&t[name..end]);
            // An address that links to itself is written as just the address.
            let bare = looks_like_url(&shown) && normalize_url(&shown) == url;
            let md = attachment_md(&url).unwrap_or_else(|| if bare { shown.clone() } else { format!("[{shown}]({url})") });
            out.push_str(&md);
            i = end;
            continue;
        }
        let n = char_len(t, i, b);
        if literal(t, i) {
            out.push('\\');
        }
        out.push_str(&unmark_str(&t[i..i + n]));
        i += n;
    }
    out
}

/// How many UTF-16 units the character at `i` takes (2 for a surrogate pair: most
/// emoji), so it's never cut in half.
fn char_len(t: &[u16], i: usize, b: usize) -> usize {
    let pair = (0xD800..0xDC00).contains(&t[i]) && i + 1 < b && (0xDC00..0xE000).contains(&t[i + 1]);
    if pair { 2 } else { 1 }
}

pub(super) fn inline_md(h: HWND, t: &[u16], a: usize, b: usize) -> String {
    inline_md_without(h, t, a, b, 0)
}

/// `inline_md` without the marks in `drop`.
fn inline_md_without(h: HWND, t: &[u16], a: usize, b: usize, drop: u32) -> String {
    let mut out = String::new();
    let mut fx = Effects { from: a, of: effects(h, a, b) };
    fx.of.iter_mut().for_each(|e| *e &= !drop);
    let cur = marks(&fx, h, t, a, b, NO_MARKS, &mut out, false);
    close_marks(&mut out, cur, NO_MARKS);
    out
}

/// The marks of each character in a stretch of text, read once.
struct Effects {
    from: usize,
    of: Vec<u32>,
}

/// Bold, italic, strikethrough and underline of each character from `a` to `b`, read a
/// run at a time: TOM steps from one change of formatting to the next, and each run is
/// one question to RichEdit. Character by character, a long note took seconds to write
/// out (export, an assistant reading it).
/// Hidden too: what's hidden isn't the note's text (see `marks`).
fn effects(h: HWND, a: usize, b: usize) -> Vec<u32> {
    use windows::Win32::UI::Controls::RichEdit::tomCharFormat;
    const ALL: u32 = CFM_BOLD | CFM_ITALIC | CFM_STRIKEOUT | CFM_UNDERLINE | CFM_HIDDEN;
    let mut out = vec![0; b.saturating_sub(a)];
    let r = super::links::range(h, a as i32, a as i32);
    let mut x = a;
    while x < b {
        // The end of the run that starts at `x` (one character, if TOM can't say).
        let end = r
            .as_ref()
            .and_then(|r| unsafe {
                r.SetRange(x as i32, x as i32).ok()?;
                r.MoveEnd(tomCharFormat.0, 1).ok()?;
                r.GetEnd().ok()
            })
            .map_or(x + 1, |e| (e.max(0) as usize).clamp(x + 1, b));
        set_sel(h, x as i32, end as i32);
        let cf = char_format(h, SCF_SELECTION);
        if cf.mask & ALL == ALL {
            out[x - a..end - a].fill(cf.effects & ALL);
            x = end;
        } else {
            // Not one run after all: this character alone, then on.
            set_sel(h, x as i32, x as i32 + 1);
            out[x - a] = char_format(h, SCF_SELECTION).effects & ALL;
            x += 1;
        }
    }
    out
}

/// Append `t[a..b]` with its marks to `out`, starting from the marks in effect `cur`;
/// returns those in effect at the end. A mark running across a link stays open around
/// it: **the [doc](url)**.
/// Bold, italic, strikethrough, underline.
type Marks = (bool, bool, bool, bool);
const NO_MARKS: Marks = (false, false, false, false);

fn blank(c: u16) -> bool {
    c == b' ' as u16 || c == b'\t' as u16
}

/// The marks on the character at `i`.
/// The marks on the character at `i`. In a link's text (`link`) underline isn't one: it's
/// how links look (RichEdit writes it into the RTF for them).
fn marks_at(fx: &Effects, i: usize, n: usize, link: bool) -> Marks {
    // A character's marks are all of its units' (a surrogate pair is read as one).
    let e = fx.of[i - fx.from..i - fx.from + n].iter().fold(u32::MAX, |acc, &x| acc & x);
    (e & CFM_BOLD != 0, e & CFM_ITALIC != 0, e & CFM_STRIKEOUT != 0, e & CFM_UNDERLINE != 0 && !link)
}

/// The marks both `a` and `b` have.
fn both(a: Marks, b: Marks) -> Marks {
    (a.0 && b.0, a.1 && b.1, a.2 && b.2, a.3 && b.3)
}

#[allow(clippy::too_many_arguments)]
fn marks(fx: &Effects, h: HWND, t: &[u16], a: usize, b: usize, mut cur: Marks, out: &mut String, link: bool) -> Marks {
    let links = instructions(t, a, b);
    let mut i = a;
    while i < b {
        if let Some((name, end, url)) = link_from(h, &links, i, b) {
            if let Some(md) = attachment_md(&url) {
                out.push_str(&md);
                i = end;
                continue;
            }
            // An address that links to itself goes out as just the address.
            let shown = unmark_str(&t[name..end]);
            if looks_like_url(&shown) && normalize_url(&shown) == url {
                cur = marks(fx, h, t, name, end, cur, out, true);
                i = end;
                continue;
            }
            // Spaces at the end of a link's text go after its brackets.
            let mut shown_end = end;
            while shown_end > name && blank(t[shown_end - 1]) {
                shown_end -= 1;
            }
            // A mark stays open across the link only when all of its text has it: the
            // others close before its `[` and open again inside, and those opened inside
            // close before its `]` (`[**bold link**](url)`, not `[**bold link](url)**`).
            let mut across = cur;
            for k in name..shown_end {
                if !blank(t[k]) && fx.of[k - fx.from] & CFM_HIDDEN == 0 {
                    across = both(across, marks_at(fx, k, 1, true));
                }
            }
            close_marks(out, cur, across);
            out.push('[');
            let inside = marks(fx, h, t, name, shown_end, across, out, true);
            cur = both(inside, across);
            close_marks(out, inside, cur);
            out.push_str(&format!("]({url})"));
            out.push_str(&unmark_str(&t[shown_end..end]));
            i = end;
            continue;
        }
        let n = char_len(t, i, b);
        // Hidden text, but for an emoji selector's stand-in, is nothing the note shows (a
        // code block's language, come along with its text).
        if fx.of[i - fx.from] & CFM_HIDDEN != 0 && unmark(t[i]) == t[i] {
            i += n;
            continue;
        }
        // A space carries only the marks on both sides of it: `**bold** _italic_`, never
        // `**bold **_italic_` (a mark that closes after a space isn't one), while a bold
        // phrase stays one run, `**two words**`.
        let st = if blank(t[i]) {
            let next = (i + n..b).find(|&j| !blank(t[j])).map(|j| marks_at(fx, j, char_len(t, j, b), link));
            match next {
                Some(nx) => {
                    let here = marks_at(fx, i, n, link);
                    (here.0 && cur.0 && nx.0, here.1 && cur.1 && nx.1, here.2 && cur.2 && nx.2, here.3 && cur.3 && nx.3)
                }
                None => NO_MARKS,
            }
        } else {
            marks_at(fx, i, n, link)
        };
        if st != cur {
            close_marks(out, cur, st);
            if st.0 && !cur.0 {
                out.push_str("**");
            }
            if st.1 && !cur.1 {
                out.push('_');
            }
            if st.2 && !cur.2 {
                out.push_str("~~");
            }
            // Markdown has no underline: the HTML tag, which viewers show and import reads.
            if st.3 && !cur.3 {
                out.push_str("<u>");
            }
            cur = st;
        }
        // (Not in an address, which is read back as it is.)
        if !link && literal(t, i) {
            out.push('\\');
        }
        out.push_str(&unmark_str(&t[i..i + n]));
        i += n;
    }
    cur
}

/// What a backslash before it makes text on the way back in (`inline_marks`, `md_links`).
const ESCAPED: [char; 5] = ['*', '_', '~', '<', '['];

/// Whether the character at `i` in `t` would read back as part of a mark (`*italic*`,
/// `__bold__`, `~~struck~~`), a tag (`<u>`, `<br>`, `<p align=...>`) or a link
/// (`[a](url)`), so it goes out escaped (`\_`): typed, it's text. Not when it can't be read
/// so: a run of stars or underscores between spaces (2 ** 3), one star or any underscores
/// inside a word (2*3*4, snake_case), one tilde. Always after a backslash that's text, which
/// would otherwise read as its escape (C:\_temp).
fn literal(t: &[u16], i: usize) -> bool {
    let c = t[i];
    if !ESCAPED.iter().any(|&e| e as u16 == c) {
        return false;
    }
    if i > 0 && t[i - 1] == b'\\' as u16 {
        return true;
    }
    let (mut a, mut b) = (i, i + 1);
    while a > 0 && t[a - 1] == c {
        a -= 1;
    }
    while b < t.len() && t[b] == c {
        b += 1;
    }
    let gap = |k: Option<&u16>| k.is_none_or(|&c| c == CR || c == CELL || char::from_u32(c as u32).is_some_and(char::is_whitespace));
    let word = |k: Option<&u16>| k.is_some_and(|&c| char::from_u32(c as u32).is_some_and(char::is_alphanumeric));
    let (before, after) = (a.checked_sub(1).and_then(|k| t.get(k)), t.get(b));
    let follows = |s: &str| s.encode_utf16().enumerate().all(|(k, u)| t.get(i + 1 + k) == Some(&u));
    match c as u8 {
        b'*' => !(gap(before) && gap(after)) && !(b - a == 1 && word(before) && word(after)),
        b'_' => !(gap(before) && gap(after)) && !(word(before) && word(after)),
        b'~' => b - a >= 2 && !(gap(before) && gap(after)),
        b'<' => ["u>", "/u>", "br>", "p align=", "h1 align=", "h2 align=", "h3 align=", "h4 align=", "h5 align=", "h6 align="].iter().any(|s| follows(s)),
        // A link: `](` further on in the paragraph.
        _ => {
            let end = t[i..].iter().position(|&c| c == CR || c == CELL).map_or(t.len(), |k| i + k);
            t[i..end].windows(2).any(|w| w == [b']' as u16, b'(' as u16])
        }
    }
}

pub(super) fn close_marks(out: &mut String, cur: Marks, next: Marks) {
    if cur.3 && !next.3 {
        out.push_str("</u>");
    }
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
    bulk(h, || load_markdown_now(h, md));
}

fn load_markdown_now(h: HWND, md: &str) {
    let norm = md.replace("\r\n", "\n");
    // A file's final newline (and any blank lines after the text) is not an empty item.
    let src: Vec<&str> = norm.trim_end().split('\n').collect();
    let body = parse_body(&src);
    set_sel(h, 0, -1);
    insert_text(h, &body.joined());
    // Start clean: a reused control keeps the last document's list and character formats.
    set_sel(h, 0, -1);
    set_numbering(h, 0);
    set_alignment(h, PFA_LEFT);
    set_effect(h, CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT, false);
    put_body(h, 0, &body);
    set_sel(h, 0, 0);
}

/// Markdown lines as paragraphs (the whole note, or what a quote holds): quotes, code and
/// tables each have an empty paragraph holding their place until the text is in.
struct Body {
    lines: Vec<Line>,
    /// Which of `lines` holds which block.
    fills: Vec<(usize, Fill)>,
}

impl Body {
    /// The paragraphs' text, as it goes into the editor.
    fn joined(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\r")
    }
}

fn parse_body(src: &[&str]) -> Body {
    let mut lines: Vec<Line> = Vec::new();
    let mut fills: Vec<(usize, Fill)> = Vec::new();
    let mut i = 0;
    while i < src.len() {
        // Trailing spaces only ever mean a line break (`hard_breaks`), which a line is anyway.
        let raw = src[i].trim_end();
        i += 1;
        // A blank line between a list item and plain text is Markdown's way to end the list
        // (`store::hard_breaks` writes one), not an empty line: export writes one more
        // where the note had one.
        if raw.is_empty() && i >= 2 && crate::common::store::list_line(src[i - 2]) && src[i..].iter().find(|l| !l.trim().is_empty()).is_some_and(|l| crate::common::store::plain_line(l)) {
            continue;
        }
        if let Some((fill, used)) = block_from(raw, &src[i..]) {
            // A blank line on either side of a block is Markdown's way to set it apart (a
            // table needs one), not an empty paragraph: blocks have their own space.
            if lines.last().is_some_and(|l| l.text.is_empty()) && fills.last().is_none_or(|(at, _)| *at != lines.len() - 1) {
                lines.pop();
            }
            fills.push((lines.len(), fill));
            lines.push(Line::empty());
            i += used;
            if src.get(i).is_some_and(|n| n.trim().is_empty()) {
                i += 1;
            }
            continue;
        }
        lines.push(parse_line(raw));
    }
    Body { lines, fills }
}

/// Give the paragraphs of `body`, their text already in from `at`, their formats, links
/// and blocks.
fn put_body(h: HWND, at: i32, body: &Body) {
    let mut pos = at;
    // Everything that inserts text (links' hidden instructions, blocks), by position.
    let mut later: Vec<(i32, Later)> = Vec::new();
    let mut fills = body.fills.iter().peekable();
    for (n, line) in body.lines.iter().enumerate() {
        later.extend(line.links.iter().map(|(a, b, url)| (pos + a, Later::Link(pos + b, url))));
        if let Some((_, fill)) = fills.next_if(|(k, _)| *k == n) {
            later.push((pos, Later::Block(fill)));
        }
        format_line(h, pos, line);
        pos += line.text.encode_utf16().count() as i32 + 1;
    }
    // Each adds text: from the end, so the rest stay put.
    later.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, l) in later {
        match l {
            Later::Link(b, url) => {
                set_link(h, at, b, url);
            }
            Later::Block(fill) => put_block(h, at, fill),
        }
    }
}

/// What a Markdown line makes of its paragraph.
enum Kind {
    Plain,
    Bullet,
    /// With the number it was written with.
    Number(u16),
    Task(bool),
}

/// A paragraph read from a Markdown line: its kind and nesting level, its text (with a
/// checklist item's box), marks, links and alignment.
struct Line {
    kind: Kind,
    level: i32,
    text: String,
    runs: Runs,
    links: Links,
    align: u16,
}

impl Line {
    fn empty() -> Line {
        Line { kind: Kind::Plain, level: 0, text: String::new(), runs: Vec::new(), links: Vec::new(), align: PFA_LEFT }
    }
}

/// The text after a task marker (`- [ ] `, `[x] `), or "" for an item with none.
fn task_rest(l: &str, done: bool) -> Option<&str> {
    let marks: &[&str] = if done { &["- [x]", "- [X]", "[x]", "[X]"] } else { &["- [ ]", "[ ]"] };
    marks.iter().find_map(|m| l.strip_prefix(m).and_then(|r| r.strip_prefix(' ').or(r.is_empty().then_some(""))))
}

/// One Markdown line, outside any block, as a paragraph.
fn parse_line(raw: &str) -> Line {
    let l = raw.trim_start();
    // A centred or right-aligned line, as `to_markdown` writes one.
    let (raw, align) = match aligned(l) {
        Some((inner, a)) => (inner, a),
        None => (raw, PFA_LEFT),
    };
    let l = raw.trim_start();
    // Two spaces (or a tab) per nesting level, as `to_markdown` writes them.
    let indent: usize = raw[..raw.len() - l.len()].chars().map(|c| if c == '\t' { 2 } else { 1 }).sum();
    let hashes = l.chars().take_while(|&c| c == '#').count();
    // Text written escaped, with the spaces before it (`  \- not a list`).
    let escaped = crate::common::store::unescaped(l).map(|r| format!("{}{r}", &raw[..raw.len() - l.len()]));
    let (k, rest) = if let Some(r) = &escaped {
        (Kind::Plain, r.as_str())
    } else if ["-", "*", "•"].contains(&l) {
        // An empty item (its marker's space taken off with the line's end).
        (Kind::Bullet, "")
    } else if let Some(r) = task_rest(l, false) {
        (Kind::Task(false), r)
    } else if let Some(r) = task_rest(l, true) {
        (Kind::Task(true), r)
    } else if let Some(r) = l[hashes..].strip_prefix(' ').filter(|_| (1..=6).contains(&hashes)) {
        (Kind::Plain, r)
    } else if let Some(r) = l.strip_prefix("- ").or(l.strip_prefix("* ")).or(l.strip_prefix("• ")) {
        (Kind::Bullet, r)
    } else {
        let d = l.chars().take_while(|c| c.is_ascii_digit()).count();
        match l[d..].strip_prefix(". ").or((&l[d..] == ".").then_some("")) {
            Some(r) if d > 0 => (Kind::Number(l[..d].parse().unwrap_or(1)), r),
            _ => (Kind::Plain, raw),
        }
    };
    // A list item's alignment is inside its marker (`- <p align="center">item</p>`).
    let (rest, align) = match aligned(rest) {
        Some((inner, a)) if !matches!(k, Kind::Plain) => (inner, a),
        _ => (rest, align),
    };
    // A list item's text that would read as a quote or a checkbox is written escaped.
    let rest = match rest.strip_prefix('\\') {
        Some(r) if matches!(k, Kind::Bullet | Kind::Number(_)) && (r.starts_with('>') || reads_as_box(r)) => r,
        _ => rest,
    };
    let (text, mut runs, mut links) = parse_inline(rest);
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
    for l in &mut links {
        l.0 += shift;
        l.1 += shift;
    }
    let level = if matches!(k, Kind::Plain) { 0 } else { (indent / 2).min(MAX_LEVEL as usize) as i32 };
    Line { kind: k, level, text: format!("{mark}{text}"), runs, links, align }
}

/// Give the paragraph at `pos`, its text already in, the format `line` describes. Its
/// links are the caller's to make: they insert text.
fn format_line(h: HWND, pos: i32, line: &Line) {
    let (k, level, align, runs) = (&line.kind, &line.level, &line.align, &line.runs);
    let len = line.text.encode_utf16().count() as i32;
    set_sel(h, pos, pos);
    match k {
        Kind::Bullet => set_numbering(h, PFN_BULLET),
        Kind::Number(_) => set_numbering(h, PFN_ARABIC),
        Kind::Task(true) => {
            set_sel(h, pos + 1, pos + len);
            set_effect(h, CFM_STRIKEOUT, true);
        }
        _ => {}
    }
    if *align != PFA_LEFT {
        set_sel(h, pos, pos);
        set_alignment(h, *align);
    }
    for _ in 0..*level {
        set_sel(h, pos, pos);
        nest(h, 1);
    }
    if let Kind::Number(n) = *k {
        // After a nested sublist RichEdit would restart the count: carry it on. A list's
        // first item has its own number (`5. item`), as in Markdown, where only the
        // first one counts.
        let (before, first) = siblings_before(h, &text(h), pos, *level);
        let start = if before > 0 { first + before as u16 } else { n.max(1) };
        if start != 1 {
            set_sel(h, pos, pos);
            let mut pf = ParaFormat2::new();
            pf.mask = PFM_NUMBERINGSTART;
            pf.numbering_start = start;
            send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
        }
    }
    for &(a, b, effect) in runs {
        set_sel(h, pos + a, pos + b);
        set_effect(h, effect, true);
    }
}

/// A paragraph's text with its inline marks and links taken out: (text, runs, links).
type Inline = (String, Runs, Links);

/// A block's content, read from Markdown.
enum Fill {
    /// How far it's indented under a list item, and what it holds (blocks of its own too).
    Quote(usize, Body),
    /// Its language (```rust), and its lines.
    Code(String, Vec<String>),
    /// Rows of cells (each cell's paragraphs), and each column's alignment.
    Table(Vec<Vec<Vec<Inline>>>, Vec<u16>),
}

enum Later<'a> {
    Link(i32, &'a str),
    Block(&'a Fill),
}

/// Strip a line's marks and links: its text, (start, end, effect) runs and links.
fn parse_inline(rest: &str) -> Inline {
    let (text, mut runs) = inline_marks(rest);
    // Links last: marks may sit inside their names. Removing the link syntax moves
    // what follows it, so the runs move with it.
    let (plain, links) = md_links(&text);
    let units = |s: &str| s.encode_utf16().count() as i32;
    let (old, new) = (units(&text), units(&plain));
    if old != new {
        // Map each UTF-16 position of `text` to its place in `plain`.
        let mut map = vec![0i32; old as usize + 1];
        let (o, p): (Vec<u16>, Vec<u16>) = (text.encode_utf16().collect(), plain.encode_utf16().collect());
        let (mut i, mut j) = (0usize, 0usize);
        while i < o.len() {
            map[i] = j as i32;
            if j < p.len() && o[i] == p[j] {
                j += 1;
            }
            i += 1;
        }
        map[o.len()] = p.len() as i32;
        for r in &mut runs {
            r.0 = map[r.0 as usize];
            r.1 = map[r.1 as usize];
        }
    }
    (plain, runs, links)
}

/// `<p align="center">…</p>` (or right, or left), or a heading so (`<h1 align="center">`,
/// as a centred title goes out): the inside and its alignment.
fn aligned(l: &str) -> Option<(&str, u16)> {
    let (tag, rest) = ["p", "h1", "h2", "h3", "h4", "h5", "h6"].iter().find_map(|&tag| l.strip_prefix('<')?.strip_prefix(tag)?.strip_prefix(" align=\"").map(|r| (tag, r)))?;
    let (word, rest) = rest.split_once("\">")?;
    let inner = rest.strip_suffix(format!("</{tag}>").as_str())?;
    let a = match word {
        "center" => PFA_CENTER,
        "right" => PFA_RIGHT,
        "left" => PFA_LEFT,
        _ => return None,
    };
    Some((inner, a))
}

/// A block starting at line `l`, with the lines after it in `next`: what it holds, and
/// how many of `next` it takes.
fn block_from(raw: &str, next: &[&str]) -> Option<(Fill, usize)> {
    let l = raw.trim_start();
    // ``` code (```rust: with its language), to a closing fence at least as long (or the
    // end).
    if let Some((ticks, lang)) = crate::common::store::fence(l) {
        let body: Vec<String> = next.iter().take_while(|n| !crate::common::store::closes_fence(n, ticks)).map(|n| n.trim_end().to_string()).collect();
        let used = (body.len() + 1).min(next.len());
        return Some((Fill::Code(lang.to_string(), body), used));
    }
    // > quote lines, together, read as a note of their own: each a paragraph of its own
    // kind (`> - item`), or more blocks (`> > nested`, a fenced code block). Indented two
    // spaces a level, a quote under a list item.
    let quoted = |s: &str| {
        let t = s.trim_end();
        let q = t.trim_start();
        let mut lead = t.len() - q.len();
        // `- > text`, a quote as a list item: under the item before it.
        let d = q.chars().take_while(char::is_ascii_digit).count();
        let item = q.strip_prefix("- ").or(q.strip_prefix("* ")).or((d > 0).then(|| q[d..].strip_prefix(". ")).flatten());
        let q = match item {
            Some(r) if r.starts_with('>') => {
                lead += 2;
                r
            }
            _ => q,
        };
        q.strip_prefix("> ").or(q.strip_prefix('>')).map(|r| (lead, r.to_string()))
    };
    if let Some((lead, first)) = quoted(raw) {
        let mut inner = vec![first];
        // The quote goes on while its lines are indented as its first: a quote under a
        // list item and a quote after the list are two.
        for n in next {
            match quoted(n) {
                Some((l, q)) if l == lead => inner.push(q),
                _ => break,
            }
        }
        let used = inner.len() - 1;
        let inner: Vec<&str> = inner.iter().map(String::as_str).collect();
        return Some((Fill::Quote(lead / 2, parse_body(&inner)), used));
    }
    // | a | b | with a | --- | --- | line under it.
    if l.starts_with('|') {
        let aligns = separator(next.first()?)?;
        let mut rows = vec![table_cells(l)];
        for n in &next[1..] {
            let t = n.trim();
            if !t.starts_with('|') {
                break;
            }
            rows.push(table_cells(t));
        }
        let used = rows.len();
        let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(aligns.len()).max(1);
        let cells: Vec<Vec<Vec<Inline>>> = rows
            .iter()
            .map(|r| (0..cols).map(|c| r.get(c).map(|s| cell_lines(s).into_iter().map(parse_inline).collect()).unwrap_or_else(|| vec![parse_inline("")])).collect())
            .collect();
        let mut aligns = aligns;
        aligns.resize(cols, PFA_LEFT);
        return Some((Fill::Table(cells, aligns), used));
    }
    None
}

/// A table cell's lines: `<br>` breaks one, `\<br>` is text.
fn cell_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, _) in s.match_indices("<br>") {
        if !s[..i].ends_with('\\') {
            out.push(&s[from..i]);
            from = i + 4;
        }
    }
    out.push(&s[from..]);
    out
}

/// The cells of a table line `| a | b |`, without their pipes (`\|` is a pipe in a cell).
fn table_cells(l: &str) -> Vec<String> {
    let t = l.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').filter(|r| !r.ends_with('\\')).unwrap_or(t);
    let mut out = vec![String::new()];
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                out.last_mut().unwrap().push('|');
                chars.next();
            }
            '|' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|c| c.trim().to_string()).collect()
}

/// A table's `| --- | :---: | ---: |` line: each column's alignment, or None if `l` isn't one.
fn separator(l: &str) -> Option<Vec<u16>> {
    let cells = table_cells(l);
    cells
        .iter()
        .map(|c| {
            let dashes = c.trim_matches(':');
            if dashes.is_empty() || !dashes.chars().all(|ch| ch == '-') {
                return None;
            }
            Some(match (c.starts_with(':'), c.ends_with(':')) {
                (true, true) => PFA_CENTER,
                (false, true) => PFA_RIGHT,
                _ => PFA_LEFT,
            })
        })
        .collect()
}

/// Build `fill` in place of the empty paragraph at `at`.
fn put_block(h: HWND, at: i32, fill: &Fill) {
    let (kind, rows, cols) = match fill {
        Fill::Quote(..) => (Block::Quote, 1, 1),
        Fill::Code(..) => (Block::Code, 1, 1),
        Fill::Table(r, a) => (Block::Table, r.len().max(1), a.len().max(1)),
    };
    // In place of the placeholder's paragraph mark (the block ends in its own), so no
    // empty line is left: RichEdit won't fold one into a block that follows.
    let t = text(h);
    let over = if t.get(at as usize) == Some(&CR) { 1 } else { 0 };
    insert_block_over(h, at, over, kind, rows, cols);
    let t = text(h);
    let Some((_, rs)) = block_at(h, &t, at) else { return };
    if let Fill::Quote(level, _) = fill
        && *level > 0
    {
        set_quote_level(h, rs[0], *level);
    }
    let all: Vec<(usize, usize)> = rs.iter().flat_map(|r| cells(&t, *r)).collect();
    // From the last cell up, so the ones before stay put.
    for (n, &(a, _)) in all.iter().enumerate().rev() {
        match fill {
            Fill::Quote(_, body) => {
                // What it holds, blocks and all, goes in as a note's text does.
                set_sel(h, a as i32, a as i32);
                insert_text(h, &body.joined());
                put_body(h, a as i32, body);
            }
            Fill::Code(lang, lines) => {
                set_sel(h, a as i32, a as i32);
                insert_text(h, &lines.join("\r"));
                set_code_tag(h, a as i32, lang);
            }
            Fill::Table(rows, aligns) => {
                let (r, c) = (n / cols, n % cols);
                if let Some(paras) = rows.get(r).and_then(|row| row.get(c)) {
                    fill_cell(h, a as i32, paras, aligns.get(c).copied().unwrap_or(PFA_LEFT));
                }
            }
        }
    }
    style_blocks(h);
}

/// Paragraphs into the (empty) cell at `at`, with their marks and links.
fn fill_cell(h: HWND, at: i32, paras: &[Inline], align: u16) {
    let joined: Vec<&str> = paras.iter().map(|p| p.0.as_str()).collect();
    set_sel(h, at, at);
    insert_text(h, &joined.join("\r"));
    let mut pos = at;
    let mut links = Vec::new();
    for (s, runs, ls) in paras {
        if align != PFA_LEFT {
            set_sel(h, pos, pos);
            set_alignment(h, align);
        }
        for &(a, b, effect) in runs {
            set_sel(h, pos + a, pos + b);
            set_effect(h, effect, true);
        }
        links.extend(ls.iter().map(|(a, b, url)| (pos + a, pos + b, url.clone())));
        pos += s.encode_utf16().count() as i32 + 1;
    }
    for (a, b, url) in links.iter().rev() {
        set_link(h, *a, *b, url);
    }
}

/// Character-format runs within a line: (start, end, effect) in UTF-16 units.
type Runs = Vec<(i32, i32, u32)>;
/// Links within a line: (start, end, url) in UTF-16 units.
type Links = Vec<(i32, i32, String)>;

/// Strip `**bold**`, `_italic_` and `~~strike~~` marks from one line (as `to_markdown`
/// writes them), and the other ways assistants and other apps write them (`*italic*`,
/// `__bold__`, `***both***`), returning the text and the (start, end, effect) runs in
/// UTF-16 units. A mark without a partner stays literal text, as do one written escaped
/// (`\*`, `\_`, `\~`, and `\<` for a tag) and a `_` or `*` inside a word (snake_case,
/// 2*3*4).
pub(super) fn inline_marks(s: &str) -> (String, Runs) {
    let c: Vec<char> = s.chars().collect();
    let at = |k: usize| c.get(k).copied();
    let space = |k: Option<usize>| k.and_then(at).is_none_or(char::is_whitespace);
    let word = |k: Option<usize>| k.and_then(at).is_some_and(char::is_alphanumeric);
    // (slot, length, effect); each way of writing a mark has its slot, so `*` doesn't
    // close what `_` opened. Slot 3 is underline's.
    let mark = |k: usize| match (at(k), at(k + 1)) {
        (Some('*'), Some('*')) => Some((0, 2, CFM_BOLD)),
        (Some('~'), Some('~')) => Some((2, 2, CFM_STRIKEOUT)),
        (Some('_'), Some('_')) => Some((5, 2, CFM_BOLD)),
        (Some('_'), _) => Some((1, 1, CFM_ITALIC)),
        (Some('*'), _) => Some((4, 1, CFM_ITALIC)),
        _ => None,
    };
    // Marks made of `_` or `*` don't work inside a word.
    let inner = |slot: usize| matches!(slot, 1 | 4 | 5);
    // Can a mark of `slot` at `k` close a run / open one?
    let closes = |k: usize, slot: usize, len: usize| !space(k.checked_sub(1)) && (!inner(slot) || !word(Some(k + len)));
    let opens = |k: usize, slot: usize, len: usize| !space(Some(k + len)) && (!inner(slot) || !word(k.checked_sub(1)));
    let mut out = String::new();
    let mut n = 0i32;
    let mut open: [Option<i32>; 6] = [None; 6];
    let mut runs = Vec::new();
    let tag = |k: usize, s: &str| s.chars().enumerate().all(|(j, ch)| at(k + j) == Some(ch));
    let mut i = 0;
    while i < c.len() {
        // An escaped star is a star, and so on (`\[` is `md_links`' to read).
        if c[i] == '\\'
            && let Some(e) = at(i + 1).filter(|e| ESCAPED.contains(e) && *e != '[')
        {
            out.push(e);
            n += 1;
            i += 2;
            continue;
        }
        // Underline, written as HTML since Markdown has none.
        if tag(i, "<u>") && open[3].is_none() && (i + 3..c.len()).any(|j| tag(j, "</u>")) {
            open[3] = Some(n);
            i += 3;
            continue;
        }
        if tag(i, "</u>")
            && let Some(a) = open[3].take()
        {
            runs.push((a, n, CFM_UNDERLINE));
            i += 4;
            continue;
        }
        if let Some((slot, len, effect)) = mark(i) {
            match open[slot] {
                Some(a) if closes(i, slot, len) => {
                    runs.push((a, n, effect));
                    open[slot] = None;
                    i += len;
                    continue;
                }
                None if opens(i, slot, len)
                    && (i + len..c.len()).any(|j| mark(j).is_some_and(|m| m.0 == slot) && j > i + len && closes(j, slot, len) && at(j - 1) != Some('\\')) =>
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
        let (t, r) = inline_marks("an <u>underlined</u> word");
        assert_eq!((t.as_str(), r), ("an underlined word", vec![(3, 13, CFM_UNDERLINE)]));
        // Nested, as `inline_md` writes them.
        let (t, r) = inline_marks("**_both_**");
        assert_eq!(t, "both");
        assert_eq!(r, vec![(0, 4, CFM_ITALIC), (0, 4, CFM_BOLD)]);
        // Not marks: snake_case, a lone star pair, spaced underscores, stars in a word.
        for s in ["user_id and api_key", "2 ** 3", "a _ b _ c", "**open only", "2*3*4", "a * b * c", "snake__case__x"] {
            assert_eq!(inline_marks(s), (s.to_string(), vec![]), "{s}");
        }
        // As others write them.
        assert_eq!(inline_marks("an *aside* here"), ("an aside here".to_string(), vec![(3, 8, CFM_ITALIC)]));
        assert_eq!(inline_marks("__strong__ start"), ("strong start".to_string(), vec![(0, 6, CFM_BOLD)]));
        let (t, mut r) = inline_marks("***both***");
        r.sort_by_key(|x| x.2);
        assert_eq!((t.as_str(), r), ("both", vec![(0, 4, CFM_BOLD), (0, 4, CFM_ITALIC)]));
        assert_eq!(inline_marks("\\*not\\* italic"), ("*not* italic".to_string(), vec![]));
    }

    fn editor() -> HWND {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, WS_POPUP};
        let h = unsafe {
            windows_sys::Win32::System::LibraryLoader::LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        h
    }

    /// `md` imported comes out as `want`, from the editor and from the RTF a note is
    /// stored as; and `want` imported comes out the same again.
    fn round_trip(md: &str, want: &str) {
        let h = editor();
        load_markdown(h, md);
        assert_eq!(to_markdown(h, true), format!("# {want}"), "editor: {md}");
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        assert_eq!(to_markdown(h2, true), format!("# {want}"), "stored: {md}");
        let h3 = editor();
        load_markdown(h3, want);
        assert_eq!(to_markdown(h3, true), format!("# {want}"), "again: {want}");
    }

    /// What assistants write: `*italic*`, `__bold__`, `***both***` (written back the way
    /// the app writes marks), stars that are only stars, addresses with parentheses, and
    /// numbered lists that start past 1.
    #[test]
    fn markdown_as_others_write_it() {
        round_trip("T\nsome *italic*, __bold__ and ***both*** words", "T\nsome _italic_, **bold** and **_both_** words");
        round_trip("T\n2*3*4 is 24, 2 ** 3 is 8", "T\n2*3*4 is 24, 2 ** 3 is 8");
        // Stars typed as text stay text, however they'd read.
        round_trip("T\na \\*starred\\* word and \\*\\*two\\*\\*", "T\na \\*starred\\* word and \\*\\*two\\*\\*");
        round_trip("T\nsee [Foo](https://en.wikipedia.org/wiki/Foo_(bar)) now", "T\nsee [Foo](https://en.wikipedia.org/wiki/Foo_(bar)) now");
        round_trip("T\n5. five\n6. six\n  1. sub\n  2. sub two\n7. seven\ntext\n3. three", "T\n5. five\n6. six\n  1. sub\n  2. sub two\n7. seven\ntext\n3. three");
        // Only a list's first number counts, as in Markdown.
        round_trip("T\n4. a\n9. b", "T\n4. a\n5. b");
        round_trip("T\n> 2. quoted\n> 3. list", "T\n> 2. quoted\n> 3. list");
    }

    /// A note's length limit counts what it shows: a named link's hidden address and a
    /// table's row marks are let through on top.
    #[test]
    fn hidden_text_is_not_counted() {
        let h = editor();
        load_markdown(h, "T\nsee [docs](https://example.com/a/long/address) now\n| a | b |\n| --- | --- |\n| 1 | 2 |\nend");
        let t = text(h);
        let field = "HYPERLINK \"https://example.com/a/long/address\"".encode_utf16().count();
        let rows = t.iter().filter(|&&c| c == ROW_START).count();
        assert_eq!(rows, 2);
        assert_eq!(hidden_len(&t), field + rows * 4);
        assert_eq!(length_limit(&t), crate::common::store::MAX_NOTE_CHARS + field + rows * 4);
        // Plain text hides nothing.
        let h = editor();
        load_markdown(h, "T\nplain words");
        assert_eq!(hidden_len(&text(h)), 0);
        // A note saved longer than the limit keeps what it has.
        let long = vec![b'x' as u16; crate::common::store::MAX_NOTE_CHARS + 5];
        assert_eq!(length_limit(&long), long.len());
    }

    /// Where Tab has no tab character to type: in the title, the first paragraph.
    #[test]
    fn the_title_is_the_first_paragraph() {
        let h = editor();
        load_markdown(h, "Title\nbody");
        set_sel(h, 0, 0);
        assert!(sel_in_title(h));
        set_sel(h, 2, 5);
        assert!(sel_in_title(h));
        set_sel(h, 7, 7);
        assert!(!sel_in_title(h));
        set_sel(h, 2, 7);
        assert!(!sel_in_title(h), "reaching into the body");
    }

    /// Where `w` first is in the editor's text `t`.
    fn find(t: &[u16], w: &str) -> i32 {
        let w: Vec<u16> = w.encode_utf16().collect();
        t.windows(w.len()).position(|v| v == &w[..]).unwrap() as i32
    }

    /// `md` loaded comes out as `want`: from the editor, from the RTF a note is stored as,
    /// and from a file exported and imported again; and `want` loaded is the same note.
    fn cycle(md: &str, want: &str) {
        let h = editor();
        load_markdown(h, md);
        assert_eq!(to_markdown(h, true), want, "editor: {md}");
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        assert_eq!(to_markdown(h2, true), want, "stored: {md}");
        let exported = crate::common::store::hard_breaks(&to_markdown(h, true));
        let h3 = editor();
        load_markdown(h3, &exported);
        assert_eq!(to_markdown(h3, true), want, "exported as:\n{exported}");
        let h4 = editor();
        load_markdown(h4, want);
        assert_eq!(to_markdown(h4, true), want, "again: {want}");
        assert_eq!(text(h4), text(h), "the same note: {want}");
    }

    /// Quotes in quotes, and code blocks and tables in quotes, are blocks in the quote's
    /// cell, not text that only looks like them.
    #[test]
    fn blocks_inside_quotes() {
        let cases = [
            "# T\n> outer\n> > inner\n> back out",
            "# T\n> > > three deep",
            "# T\n> intro\n> ```rust\n> let x = 1;\n>\n>     indented\n> ```\n> after",
            "# T\n> ```\n> only code\n> ```",
            "# T\n> a\n> > b\n> ```\n> c\n> ```",
            "# T\n> > one\n>\n> > two",
            "# T\n> | a | b |\n> | --- | :---: |\n> | **1** | 2 |\n> under it",
            "# T\n> - item\n> > quoted after it\n> 2. numbered",
            "# T\n- item\n  > under it\n  > > and in that\nend",
        ];
        for md in cases {
            cycle(md, md);
        }
        // Each a block of its own, in the quote's cell.
        let h = editor();
        load_markdown(h, "T\n> outer\n> > inner\n> ```\n> code\n> ```\n> back out");
        let t = text(h);
        let kinds: Vec<Block> = blocks(h, &t).iter().map(|b| b.0).collect();
        assert_eq!(kinds, vec![Block::Quote, Block::Quote, Block::Code]);
        let inner = find(&t, "inner");
        let (_, rows) = block_at(h, &t, inner).unwrap();
        assert!(rows[0].start > blocks(h, &t)[0].1[0].start, "the inner quote is the innermost block there");
        // The light text previews read has them too.
        assert_eq!(to_markdown(h, false), "T\n> outer\n> > inner\n> ```\n> code\n> ```\n> back out");
        // What only looks like a nested quote stays text.
        cycle("T\n> \\> not nested\n> \\```", "# T\n> \\> not nested\n> \\```");
    }

    /// A code block's language comes back on its fence; the light text (previews, search)
    /// leaves it out, as it does the code's look.
    #[test]
    fn code_keeps_its_language() {
        for md in [
            "# T\n```rust\nfn main() {}\n```",
            "# T\n```\nplain\n```",
            "# T\n```c++\n\n```",
            "# T\n```md\n\u{2764}\u{FE0F} love\n```",
            // A fence that can hold one.
            "# T\n````md\n```\ninner\n```\n````",
        ] {
            cycle(md, md);
        }
        let h = editor();
        load_markdown(h, "T\n```python\nprint(1)\n```");
        assert_eq!(to_markdown(h, false), "T\n```\nprint(1)\n```");
        // Unwrapped, the code is text and its language goes with the block.
        let t = text(h);
        let p = find(&t, "print");
        unwrap(h, p);
        assert_eq!(to_markdown(h, true), "# T\nprint(1)");
        assert_eq!(to_markdown(h, false), "T\nprint(1)");
        // The caret starts the code's text past it, and what's typed there shows.
        let h = editor();
        load_markdown(h, "T\n```js\nx\n```");
        let t = text(h);
        let (_, _, ca, cb) = cell_at(h, &t, 4).unwrap();
        let start = past_tag(h, &t, ca, cb);
        assert_eq!(start, ca + 2);
        set_sel(h, start as i32, start as i32);
        replace_sel(h, "y");
        assert_eq!(to_markdown(h, true), "# T\n```js\nyx\n```");
    }

    /// Empty lines next to blocks come back as many as they were, from the editor and
    /// from the Markdown it writes.
    #[test]
    fn empty_lines_around_blocks_stay() {
        for md in [
            "# T\nbefore\n\n\n> q\n\n\nafter",
            "# T\nbefore\n\n\n\n```\nc\n```\n\n\n\nafter",
            "# T\n> a\n\n\n\n\n| x |\n| --- |\n| 1 |",
            "# T\n\n\n> q",
            "# T\n> one\n>\n>\n> > nested\n>\n>\n> two",
            "# T\n> > nested\n>\n>\n>",
        ] {
            cycle(md, md);
        }
        // Markdown's own blank line around a block is how it's set apart, not an empty
        // line in the note, and it's the same note whichever way it's written.
        cycle("T\nbefore\n\n> q\n\nafter", "# T\nbefore\n> q\nafter");
        // RichEdit keeps an empty paragraph between two blocks, and after one at the end
        // of a quote: one more empty line there is that same paragraph.
        cycle("T\n> q\n\n\n\n```\nc\n```", "# T\n> q\n```\nc\n```");
        cycle("T\n> > nested\n>\n>", "# T\n> > nested");
        // Made in the editor: empty lines above and below a quote, two between blocks.
        let h = editor();
        insert_text(h, "T\rbefore\r\rq\r\rafter\r\r\rcode");
        let q = find(&text(h), "q");
        wrap(h, q, q + 1, Block::Quote);
        let c = find(&text(h), "code");
        wrap(h, c, c + 4, Block::Code);
        let md = to_markdown(h, true);
        let h2 = editor();
        load_markdown(h2, &md);
        assert_eq!(text(h2), text(h), "{md}");
        assert_eq!(to_markdown(h2, true), md);
    }

    /// `text`, typed (no Markdown read), exported and imported again: the same note.
    fn typed_cycle(text: &str) -> String {
        let h = editor();
        insert_text(h, text);
        let md = to_markdown(h, true);
        let h2 = editor();
        load_markdown(h2, &crate::common::store::hard_breaks(&md));
        assert_eq!(String::from_utf16_lossy(&super::text(h2)), text, "went out as {md:?}");
        md
    }

    /// Typed text that reads as Markdown stays the text it is, through an export, a saved
    /// version or an assistant: it goes out escaped, and only where it would read so.
    #[test]
    fn typed_markdown_stays_text() {
        for text in [
            "T\r__init__ method",
            "T\r_not italic_ here",
            "T\r~~not struck~~ here",
            "T\r<u>tag</u> literal",
            "T\r[a](https://x.com) literal",
            "T\r<p align=\"center\">x</p>",
            "T\rC:\\_temp, a\\*b, \\[x](y), a\\~~b~~ and \\<u>c</u>",
            "_T_ and [t](x)\rbody",
        ] {
            typed_cycle(text);
        }
        // Nothing escaped that can't read as Markdown.
        assert_eq!(typed_cycle("T\rsnake_case, user__id, a ~~ b, 2*3, a < b, [x] and x~y"), "# T\nsnake_case, user__id, a ~~ b, 2*3, a < b, [x] and x~y");
        assert_eq!(typed_cycle("T\r__init__"), "# T\n\\_\\_init\\_\\_");
        // A `<br>` typed in a table cell stays text there.
        cycle("# T\n| a \\<br> b | c<br>d |\n| --- | --- |", "# T\n| a \\<br> b | c<br>d |\n| --- | --- |");
    }

    /// Plain lines that start as Markdown's quotes, lists or tables would (after spaces too,
    /// or after a backslash that's text) stay plain lines with that text.
    #[test]
    fn plain_lines_that_look_like_markdown() {
        for text in [
            "T\r>>> print(1)",
            "T\r>.< sigh",
            "T\r>",
            "T\r  - indented dash",
            "T\r   1. indented number",
            "T\r\t* tabbed star",
            "T\r| a | b |\r| --- | --- |",
            "T\r| a |\r| --- |\r| b |",
            "T\r| not a table |\rtext",
            "T\r\\> backslash first",
            "T\r\\- backslash dash",
            "T\r\\\\- two backslashes",
            "T\r  \\- spaces and a backslash",
            "T\r\\| backslash pipe",
            "T\r-",
            "T\r2.",
            "T\r[ ]",
        ] {
            typed_cycle(text);
        }
        assert_eq!(typed_cycle("T\r  - x\r| a |\r| --- |"), "# T\n  \\- x\n\\| a |\n| --- |");
        // In a quote too.
        let md = "# T\n> \\>>> py\n>   \\- x\n> \\| a |\n> | --- |";
        cycle(md, md);
    }

    /// A mark on part of a link's text, or starting in it, opens and closes inside its
    /// brackets; one on all of it may stay open around it.
    #[test]
    fn marks_in_links_nest() {
        for md in [
            "# T\n[**bold link**](https://a.b)",
            "# T\n[_it_ and **bold**](https://a.b) after",
            "# T\n**bold [link](https://a.b) more**",
            "# T\n**bold** [link](https://a.b) **more**",
            "# T\n**a** [**b** c](https://a.b)",
        ] {
            cycle(md, md);
        }
    }

    /// A ticked item goes out without the strikethrough it's drawn with, but with the
    /// tildes typed in it.
    #[test]
    fn ticked_items_keep_their_tildes() {
        for md in ["# T\n- [x] use ~~ operator", "# T\n- [x] a ~ b ~~ c", "# T\n- [x] \\~\\~typed\\~\\~ and **bold**", "# T\n- [ ] not ~~done~~ yet"] {
            cycle(md, md);
        }
        cycle("# T\n- [x] **~~bold done~~**", "# T\n- [x] **bold done**");
    }

    /// An empty list item comes back as one, not as a stray marker.
    #[test]
    fn empty_items_stay_items() {
        for (md, want) in [
            ("# T\n- a\n- \n- b", "# T\n- a\n- \n- b"),
            ("# T\n1. a\n2. \n3. b", "# T\n1. a\n2. \n3. b"),
            ("# T\n- a\n  - \n- b", "# T\n- a\n  - \n- b"),
            ("# T\n> - a\n> - \n> - b", "# T\n> - a\n> -\n> - b"),
            ("# T\n- a\n- ", "# T\n- a\n-"),
            ("# T\n1. a\n2.", "# T\n1. a\n2."),
        ] {
            cycle(md, want);
        }
        let h = editor();
        load_markdown(h, "T\n- a\n-\n- b");
        let t = text(h);
        set_sel(h, 4, 4);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        assert_eq!((String::from_utf16_lossy(&t).as_str(), pf.numbering), ("T\ra\r\rb", PFN_BULLET));
    }

    /// Text after a list goes out with the blank line Markdown needs there, and comes back
    /// without it; empty lines the note had there come back as many.
    #[test]
    fn text_after_a_list_keeps_its_lines() {
        for md in ["# T\n- item\nafter", "# T\n1. one\nafter", "# T\n- [ ] task\nafter", "# T\n- a\n  - b\nafter", "# T\n> - a\n> after", "# T\n- item\n\n\nafter", "# T\n- one\n\n- two"] {
            cycle(md, md);
        }
        // Markdown's own blank line there is how a list ends, not an empty line.
        cycle("T\n- item\n\nafter", "# T\n- item\nafter");
        // Made in the editor: an item, an empty line, text.
        let h = editor();
        insert_text(h, "T\ritem\r\rafter\rtwo\r\r\rend");
        for at in [2, 14] {
            set_sel(h, at, at);
            set_numbering(h, if at == 2 { PFN_BULLET } else { PFN_ARABIC });
        }
        let md = to_markdown(h, true);
        let h2 = editor();
        load_markdown(h2, &crate::common::store::hard_breaks(&md));
        assert_eq!(text(h2), text(h), "{md}");
    }

    /// A centred or right-aligned title, list item or checklist item keeps its alignment.
    #[test]
    fn list_items_and_the_title_keep_alignment() {
        let md = "<h1 align=\"center\">T</h1>\n- <p align=\"center\">bullet</p>\n1. <p align=\"right\">number</p>\n- [ ] <p align=\"center\">task</p>\n- [x] <p align=\"right\">**done**</p>\n> - <p align=\"center\">in a quote</p>";
        cycle(md, md);
        cycle("<h1 align=\"right\">T</h1>\nbody", "<h1 align=\"right\">T</h1>\nbody");
        let h = editor();
        load_markdown(h, md);
        let t = text(h);
        let at = find(&t, "bullet");
        set_sel(h, at, at);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        assert_eq!((pf.numbering, pf.alignment), (PFN_BULLET, PFA_CENTER));
        set_sel(h, 0, 0);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        assert_eq!(pf.alignment, PFA_CENTER);
        // The light text has none of it.
        assert_eq!(to_markdown(h, false).lines().next(), Some("T"));
    }

    /// A list item whose text starts like a quote or a checkbox stays that item with that
    /// text.
    #[test]
    fn item_text_that_looks_like_syntax() {
        let md = "# T\n- \\> not a quote\n- \\[ ] not a box\n- \\[x] nor this\n1. \\> numbered\n- [ ] > a task's is fine\n> - \\> in a quote";
        cycle(md, md);
        let h = editor();
        load_markdown(h, md);
        let t = text(h);
        assert!(blocks(h, &t).len() == 1, "only the quote at the end");
        let s = String::from_utf16_lossy(&t);
        assert!(s.contains("\r> not a quote\r[ ] not a box\r"), "{s}");
    }

    /// Table cells hold plain lines (the editor makes no lists there), so list markers in
    /// a cell are its text, and come back so.
    #[test]
    fn list_text_in_table_cells() {
        let md = "# T\n| - a<br>- b | [ ] c<br>1. d |\n| --- | --- |\n| x | y |";
        cycle(md, md);
    }
}
