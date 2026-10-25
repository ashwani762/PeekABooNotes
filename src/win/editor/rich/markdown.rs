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
    // The block just written, while nothing else has come after it.
    let mut last_block = None;
    while start <= t.len() {
        // Two blocks always have an empty paragraph between them (RichEdit won't let two
        // tables touch): that's their gap, not a blank line.
        if t.get(start) == Some(&CR) && start >= 2 && t[start - 2] == ROW_END && t.get(start + 1) == Some(&ROW_START) {
            start += 1;
            continue;
        }
        // Quotes, code blocks and tables go out whole.
        if t.get(start) == Some(&ROW_START)
            && let Some((kind, rows)) = block_at(h, &t, start as i32)
        {
            // Two quotes (or tables) in a row would read back as one: a blank line keeps
            // them two (and import drops it again).
            if last_block == Some(kind) && kind != Block::Code {
                out.push('\n');
            }
            last_block = Some(kind);
            out.push_str(&block_md(h, &t, kind, &rows, inline));
            out.push('\n');
            // A quote under a list item is part of it: the list's count goes on after it.
            if kind != Block::Quote || quote_level(h, rows[0]) == 0 {
                numbers = [0; MAX_LEVEL as usize + 1];
            }
            start = rows.last().map_or(t.len(), |r| r.end);
            if start >= t.len() {
                break;
            }
            continue;
        }
        let end = t[start..].iter().position(|&c| c == CR).map(|i| start + i).unwrap_or(t.len());
        last_block = None;
        out.push_str(&para_md(h, &t, start, end, inline, &mut numbers));
        out.push('\n');
        if end >= t.len() {
            break;
        }
        start = end + 1;
    }
    // Inline mode is for export: plain `-` lists in the light mode are what we store.
    if !inline {
        out = out.lines().map(light_task).collect::<Vec<_>>().join("\n");
    }
    // A line break within a paragraph (Shift+Enter) goes out as a line: never as the
    // control character RichEdit keeps it as.
    out.replace('\u{B}', "\n").trim_end().to_string()
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
    // A centred or right-aligned line keeps its alignment on the way out.
    let align = if inline && start > 0 && num == 0 && !task {
        match pf.alignment {
            PFA_CENTER => Some("center"),
            PFA_RIGHT => Some("right"),
            _ => None,
        }
    } else {
        None
    };
    let mut out = prefix;
    // Plain text that only looks like a list or heading ("- not a list", typed and
    // reverted with Backspace) is escaped, so it comes back as text.
    let title = inline && start == 0 && out.is_empty() && end > 0;
    if out.is_empty() && !title && crate::common::store::looks_like_marker(&visible(t, body_start, end)) {
        out.push('\\');
    }
    if title {
        out.push_str("# ");
        out.push_str(&with_links(h, t, 0, end));
    } else if let Some(md) = inline.then(|| item_link(h, t, body_start)).flatten().and_then(|l| attachment_md(&l.2)) {
        out.push_str(&md);
    } else if inline {
        let body = inline_md(h, t, body_start, end);
        // A ticked item's strikethrough is how it looks, not part of its text (also
        // inside other marks: **~~bread~~**).
        let done = task && t[start] == CHECKED;
        out.push_str(&if done { body.replace("~~", "") } else { body });
    } else if let Some(md) = light_attachment(h, t, body_start, end) {
        out.push_str(&md);
    } else {
        out.push_str(&visible(t, body_start, end));
    }
    match align {
        Some(a) if !out.is_empty() => format!("<p align=\"{a}\">{out}</p>"),
        _ => out,
    }
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
            // Each paragraph as it would be outside (`> - item`), indented as the quote is.
            let indent = "  ".repeat(quote_level(h, rows[0]));
            let mut numbers = [0usize; MAX_LEVEL as usize + 1];
            let lines: Vec<String> = paras(a, b).into_iter().map(|(x, y)| format!("{indent}> {}", para_md(h, t, x, y, inline, &mut numbers)).trim_end().to_string()).collect();
            out.push_str(&lines.join("\n"));
        }
        Block::Code => {
            let (a, b) = cells(t, rows[0]).first().copied().unwrap_or((rows[0].start, rows[0].start));
            out.push_str("```\n");
            for (x, y) in paras(a, b) {
                out.push_str(&unmark_str(&t[x..y]).replace('\u{B}', "\n"));
                out.push('\n');
            }
            out.push_str("```");
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
    let mut out = String::new();
    let fx = Effects { from: a, of: effects(h, a, b) };
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
fn effects(h: HWND, a: usize, b: usize) -> Vec<u32> {
    use windows::Win32::UI::Controls::RichEdit::tomCharFormat;
    const ALL: u32 = CFM_BOLD | CFM_ITALIC | CFM_STRIKEOUT | CFM_UNDERLINE;
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
            out.push('[');
            cur = marks(fx, h, t, name, shown_end, cur, out, true);
            out.push_str(&format!("]({url})"));
            out.push_str(&unmark_str(&t[shown_end..end]));
            i = end;
            continue;
        }
        let n = char_len(t, i, b);
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
        out.push_str(&unmark_str(&t[i..i + n]));
        i += n;
    }
    cur
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
    let mut lines: Vec<Line> = Vec::new();
    // Quotes, code and tables: an empty line holds each one's place until the text is in.
    let mut fills: Vec<(usize, Fill)> = Vec::new();
    let norm = md.replace("\r\n", "\n");
    // A file's final newline (and any blank lines after the text) is not an empty item.
    let src: Vec<&str> = norm.trim_end().split('\n').collect();
    let mut i = 0;
    while i < src.len() {
        // Trailing spaces only ever mean a line break (`hard_breaks`), which a line is anyway.
        let raw = src[i].trim_end();
        i += 1;
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
    let joined: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    set_sel(h, 0, -1);
    insert_text(h, &joined.join("\r"));
    // Start clean: a reused control keeps the last document's list and character formats.
    set_sel(h, 0, -1);
    set_numbering(h, 0);
    set_alignment(h, PFA_LEFT);
    set_effect(h, CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT, false);
    let mut pos = 0i32;
    // Everything that inserts text (links' hidden instructions, blocks), by position.
    let mut later: Vec<(i32, Later)> = Vec::new();
    let mut fills = fills.into_iter().peekable();
    for (n, line) in lines.iter().enumerate() {
        later.extend(line.links.iter().map(|(a, b, url)| (pos + a, Later::Link(pos + b, url.clone()))));
        if let Some((_, fill)) = fills.next_if(|(at, _)| *at == n) {
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
                set_link(h, at, b, &url);
            }
            Later::Block(fill) => put_block(h, at, &fill),
        }
    }
    set_sel(h, 0, 0);
}

/// What a Markdown line makes of its paragraph.
enum Kind {
    Plain,
    Bullet,
    Number,
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
    let (k, rest) = if let Some(r) = crate::common::store::unescaped(l) {
        (Kind::Plain, r)
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
        match l[d..].strip_prefix(". ") {
            Some(r) if d > 0 => (Kind::Number, r),
            _ => (Kind::Plain, raw),
        }
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
        Kind::Number => set_numbering(h, PFN_ARABIC),
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
    if matches!(k, Kind::Number) {
        // After a nested sublist RichEdit would restart the count: carry it on.
        let before = siblings_before(h, &text(h), pos, *level);
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
}

/// A paragraph's text with its inline marks and links taken out: (text, runs, links).
type Inline = (String, Runs, Links);

/// A block's content, read from Markdown.
enum Fill {
    /// How far it's indented under a list item, and its paragraphs.
    Quote(usize, Vec<Line>),
    Code(Vec<String>),
    /// Rows of cells (each cell's paragraphs), and each column's alignment.
    Table(Vec<Vec<Vec<Inline>>>, Vec<u16>),
}

enum Later {
    Link(i32, String),
    Block(Fill),
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

/// `<p align="center">…</p>` (or right, or left): the inside and its alignment.
fn aligned(l: &str) -> Option<(&str, u16)> {
    let rest = l.strip_prefix("<p align=\"")?;
    let (word, rest) = rest.split_once("\">")?;
    let inner = rest.strip_suffix("</p>")?;
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
    // ``` code, to the closing fence (or the end).
    if l.starts_with("```") {
        let body: Vec<String> = next.iter().take_while(|n| !n.trim().starts_with("```")).map(|n| n.trim_end().to_string()).collect();
        let used = (body.len() + 1).min(next.len());
        return Some((Fill::Code(body), used));
    }
    // > quote lines, together, each a paragraph of its own kind (`> - item`); indented
    // two spaces a level, a quote under a list item.
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
        let mut paras = vec![parse_line(&first)];
        // The quote goes on while its lines are indented as its first: a quote under a
        // list item and a quote after the list are two.
        for n in next {
            match quoted(n) {
                Some((l, q)) if l == lead => paras.push(parse_line(&q)),
                _ => break,
            }
        }
        let used = paras.len() - 1;
        return Some((Fill::Quote(lead / 2, paras), used));
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
            .map(|r| (0..cols).map(|c| r.get(c).map(|s| s.split("<br>").map(parse_inline).collect()).unwrap_or_else(|| vec![parse_inline("")])).collect())
            .collect();
        let mut aligns = aligns;
        aligns.resize(cols, PFA_LEFT);
        return Some((Fill::Table(cells, aligns), used));
    }
    None
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
        Fill::Code(_) => (Block::Code, 1, 1),
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
            Fill::Quote(_, lines) => fill_quote(h, a as i32, lines),
            Fill::Code(lines) => {
                set_sel(h, a as i32, a as i32);
                insert_text(h, &lines.join("\r"));
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

/// A quote's paragraphs into its (empty) cell at `at`: lists, checklists and all.
fn fill_quote(h: HWND, at: i32, lines: &[Line]) {
    let joined: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    set_sel(h, at, at);
    insert_text(h, &joined.join("\r"));
    let mut pos = at;
    let mut links = Vec::new();
    for line in lines {
        format_line(h, pos, line);
        links.extend(line.links.iter().map(|(a, b, url)| (pos + a, pos + b, url.clone())));
        pos += line.text.encode_utf16().count() as i32 + 1;
    }
    for (a, b, url) in links.iter().rev() {
        set_link(h, *a, *b, url);
    }
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
    let mut open: [Option<i32>; 4] = [None; 4];
    let mut runs = Vec::new();
    let tag = |k: usize, s: &str| s.chars().enumerate().all(|(j, ch)| at(k + j) == Some(ch));
    let mut i = 0;
    while i < c.len() {
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
        let (t, r) = inline_marks("an <u>underlined</u> word");
        assert_eq!((t.as_str(), r), ("an underlined word", vec![(3, 13, CFM_UNDERLINE)]));
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
