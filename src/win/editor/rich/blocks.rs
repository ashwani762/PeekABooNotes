// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Quotes, code blocks and tables, all RichEdit table rows underneath.
//!
//! RichEdit draws a table row itself (borders, cell shading), edits it (Tab between cells,
//! Enter for another line in one) and keeps it in the RTF, so all three are rows: a quote
//! is one cell with a bar down its left side, a code block one shaded cell in a monospace
//! face, a table any number of cells in a light grid, its first row tinted as a header.
//! Borders a block doesn't show are drawn in the colour around them, as RichEdit outlines
//! a borderless cell in grey. Which kind a row is, is told by the cell margin it was made
//! with (RTF keeps it as `\trgaph`); rows from elsewhere are tables.
//!
//! In the editor's text a row reads U+FFF9 CR, then each cell's text ending in U+0007,
//! then U+FFFB CR. A quote can hold blocks of its own (`> > nested`, a fenced code block
//! in a quote): RichEdit nests their rows in its cell, and a cell's text then reads its
//! paragraphs with the nested rows among them. A nested row at the end of a cell always
//! has an empty paragraph after it, as two blocks always have one between them.

use super::*;

pub const ROW_START: u16 = 0xFFF9;
pub const ROW_END: u16 = 0xFFFB;
pub const CELL: u16 = 0x0007;

const EM_GETTABLEPARMS: u32 = WM_USER + 265;
const EM_SETTABLEPARMS: u32 = WM_USER + 307;
const EM_INSERTTABLE: u32 = WM_USER + 232;

/// Cell margins (twips), which also say what a row is.
const TABLE_GAP: i32 = 108;
const CODE_GAP: i32 = 161;
const QUOTE_GAP: i32 = 201;
/// Border widths, twips.
const HAIR: i16 = 15;
const BAR: i16 = 45;
/// Most cells a row may have (RichEdit's own limit).
const MAX_CELLS: usize = 63;
/// How far short of the text column's right edge every block ends, twips (as code blocks
/// always have).
const BLOCK_INSET: i32 = 2 * CODE_GAP;
/// Space above a code block's first line and below its last, twips.
const CODE_AIR: i32 = 90;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    Table,
    Code,
    Quote,
}

#[repr(C, packed(4))]
#[derive(Default, Clone, Copy)]
struct RowParms {
    cb_row: u8,
    cb_cell: u8,
    cells: u8,
    rows: u8,
    margin: i32,
    indent: i32,
    height: i32,
    /// nAlignment:3, fRTL, fKeep, fKeepFollow, fWrap, fIdentCells.
    bits: u32,
    start: i32,
    level: u8,
    cell: u8,
}

#[repr(C, packed(4))]
#[derive(Default, Clone, Copy)]
struct CellParms {
    width: i32,
    bits: u16,
    shading: u16,
    border: [i16; 4],
    border_color: [u32; 4],
    back: u32,
    fore: u32,
}

impl RowParms {
    fn new(start: i32, cells: usize) -> RowParms {
        RowParms { cb_row: size_of::<RowParms>() as u8, cb_cell: size_of::<CellParms>() as u8, cells: cells as u8, rows: 1, start, ..Default::default() }
    }
}

/// A row: where its U+FFF9 is and where it ends (just past its closing CR).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Row {
    pub start: usize,
    pub end: usize,
}

/// Whether the U+FFF9 at `i` starts a row (it opens a paragraph).
fn starts_row(t: &[u16], i: usize) -> bool {
    t[i] == ROW_START && (i == 0 || matches!(t[i - 1], CR | CELL))
}

/// Every row in `t`, in order, those nested in a cell too: each ends past the rows nested
/// in it.
pub fn rows(t: &[u16]) -> Vec<Row> {
    let mut out = Vec::new();
    let mut open = Vec::new();
    for i in 0..t.len() {
        if starts_row(t, i) {
            open.push(i);
        } else if t[i] == ROW_END
            && let Some(start) = open.pop()
        {
            out.push(Row { start, end: (i + 2).min(t.len()) });
        }
    }
    // A nested row closes before the row around it: back in order of where they start.
    out.sort_by_key(|r| r.start);
    out
}

/// The cells of `row`: each one's text, as (start, end) in `t` (end at its U+0007). A row
/// nested in a cell is part of that cell's text.
pub fn cells(t: &[u16], row: Row) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut a = row.start + 2;
    let mut depth = 0;
    for i in row.start + 2..row.end.min(t.len()) {
        match t[i] {
            _ if starts_row(t, i) => depth += 1,
            ROW_END if depth > 0 => depth -= 1,
            CELL if depth == 0 => {
                out.push((a, i));
                a = i + 1;
            }
            _ => {}
        }
    }
    out
}

fn get(h: HWND, row: Row) -> Option<(RowParms, Vec<CellParms>)> {
    let mut r = RowParms::new(row.start as i32, MAX_CELLS);
    let mut cells = vec![CellParms::default(); MAX_CELLS];
    let hr = send(h, EM_GETTABLEPARMS, &mut r as *mut _ as usize, cells.as_mut_ptr() as isize);
    if hr != 0 {
        return None;
    }
    cells.truncate(r.cells as usize);
    Some((r, cells))
}

fn set(h: HWND, row: Row, r: &RowParms, cells: &[CellParms]) {
    let mut r = *r;
    r.cb_row = size_of::<RowParms>() as u8;
    r.cb_cell = size_of::<CellParms>() as u8;
    r.start = row.start as i32;
    r.rows = 1;
    r.cells = cells.len() as u8;
    send(h, EM_SETTABLEPARMS, &r as *const _ as usize, cells.as_ptr() as isize);
}

/// What `row` is, by the cell margin it was made with.
pub fn kind(h: HWND, row: Row) -> Block {
    match get(h, row).map(|(r, _)| r.margin) {
        Some(CODE_GAP) => Block::Code,
        Some(QUOTE_GAP) => Block::Quote,
        _ => Block::Table,
    }
}

/// Blocks in `t`: a table's rows together, each quote and code block on its own; a block
/// nested in a cell comes after the one holding it.
pub fn blocks(h: HWND, t: &[u16]) -> Vec<(Block, Vec<Row>)> {
    let mut out: Vec<(Block, Vec<Row>)> = Vec::new();
    for row in rows(t) {
        let k = kind(h, row);
        // The row after a table's last one is more of it (a block nested in the row
        // before may have come in between).
        let table = (k == Block::Table).then(|| out.iter_mut().rev().find(|(kk, rs)| *kk == Block::Table && rs.last().is_some_and(|r| r.end == row.start))).flatten();
        match table {
            Some((_, rs)) => rs.push(row),
            None => out.push((k, vec![row])),
        }
    }
    out
}

/// The block holding `cp`, with its rows: the innermost one, in a block nested in a quote.
pub fn block_at(h: HWND, t: &[u16], cp: i32) -> Option<(Block, Vec<Row>)> {
    let cp = cp.max(0) as usize;
    blocks(h, t)
        .into_iter()
        .filter(|(_, rs)| rs.first().is_some_and(|r| r.start <= cp) && rs.last().is_some_and(|r| cp < r.end))
        .max_by_key(|(_, rs)| rs[0].start)
}

// ---------------------------------------------------------------- looks

thread_local! {
    /// The colours blocks are drawn in, per editor: (paper, ink, dark).
    static INK: std::cell::RefCell<std::collections::HashMap<isize, (u32, u32, bool)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Editor `h` sits on `paper` with text in `ink` (`dark`: a dark theme).
pub fn set_block_colors(h: HWND, paper: u32, ink: u32, dark: bool) {
    INK.with(|m| m.borrow_mut().insert(h as isize, (paper, ink, dark)));
}

pub fn forget_block_colors(h: HWND) {
    INK.with(|m| m.borrow_mut().remove(&(h as isize)));
}

fn colors(h: HWND) -> (u32, u32, bool) {
    INK.with(|m| m.borrow().get(&(h as isize)).copied()).unwrap_or((0xFFFFFF, 0x1D1D1F, false))
}

/// A code block's shading on `paper`.
pub fn code_back(paper: u32, ink: u32, dark: bool) -> u32 {
    crate::win::util::mix(paper, ink, if dark { 0.10 } else { 0.065 })
}

/// The monospace face for code: Cascadia Mono where it's installed (Windows 11), else
/// Consolas.
pub fn mono_face() -> &'static str {
    use std::cell::OnceCell;
    use windows_sys::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW, DEFAULT_CHARSET, DeleteDC, DeleteObject, FW_NORMAL, GetTextFaceW,
        OUT_TT_PRECIS, SelectObject,
    };
    thread_local! {
        static FACE: OnceCell<&'static str> = const { OnceCell::new() };
    }
    FACE.with(|c| {
        *c.get_or_init(|| unsafe {
            let name = crate::win::util::wide("Cascadia Mono");
            let f = CreateFontW(-12, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, 0, name.as_ptr());
            let dc = CreateCompatibleDC(null_mut());
            let old = SelectObject(dc, f);
            let mut buf = [0u16; 64];
            GetTextFaceW(dc, 64, buf.as_mut_ptr());
            SelectObject(dc, old);
            DeleteDC(dc);
            DeleteObject(f);
            if crate::win::util::from_wide(&buf).eq_ignore_ascii_case("Cascadia Mono") { "Cascadia Mono" } else { "Consolas" }
        })
    })
}

/// Width of the editor's text column, in twips.
pub(super) fn column_twips(h: HWND) -> i32 {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) }.max(96) as i32;
    // A hair in from the right edge, so the right border never sits on it.
    ((rc.right - rc.left) * 1440 / dpi - 30).max(1440)
}

/// Widths for `n` cells now `old` wide, to fill `total`: in proportion, and always adding
/// up to exactly `total`.
fn fit_widths(old: &[i32], total: i32) -> Vec<i32> {
    let sum: i32 = old.iter().map(|w| (*w).max(1)).sum::<i32>().max(1);
    let mut out: Vec<i32> = old.iter().map(|w| ((*w).max(1) as i64 * total as i64 / sum as i64) as i32).collect();
    let short = total - out.iter().sum::<i32>();
    if let Some(last) = out.last_mut() {
        *last += short;
    }
    out
}

/// Give every block in `h` its look for the current colours and width: borders, shading,
/// the code face, and cells that fill the text column. Cheap when nothing changed, and
/// never a step for Undo (a note made narrower would otherwise take Ctrl+Z first).
pub fn style_blocks(h: HWND) {
    let t = text(h);
    if !t.contains(&ROW_START) {
        return;
    }
    // Nor a change to the note: RichEdit reports refitting a table as one, which would
    // have every note with a table saved (and "edited") whenever it opened or was resized.
    let mask = send(h, EM_GETEVENTMASK, 0, 0);
    send(h, EM_SETEVENTMASK, 0, 0);
    still(h, || unrecorded(h, || style_blocks_now(h, &t)));
    send(h, EM_SETEVENTMASK, 0, mask);
}

fn style_blocks_now(h: HWND, t: &[u16]) {
    let (paper, ink, dark) = colors(h);
    let total = column_twips(h);
    let (a, b) = sel(h);
    let c = crate::win::util::cref;
    let all = rows(t);
    // Each row's cell margin and new cell widths, for the blocks nested in its cells.
    let mut fitted: std::collections::HashMap<usize, (i32, Vec<i32>)> = std::collections::HashMap::new();
    for (kind, rows) in blocks(h, t) {
        for (i, &row) in rows.iter().enumerate() {
            let Some((r, old)) = get(h, row) else { continue };
            // A block nested in a cell fills that cell's text, as one in the note fills the
            // text column (its parent comes first, so its widths are known).
            let room = all
                .iter()
                .filter(|p| p.start < row.start && row.end <= p.end)
                .max_by_key(|p| p.start)
                .and_then(|p| {
                    let n = cells(t, *p).iter().position(|&(a, b)| a <= row.start && row.start <= b)?;
                    let (margin, widths) = fitted.get(&p.start)?;
                    Some(widths.get(n)? - 2 * margin)
                })
                .unwrap_or(total);
            // A block's box runs from its indent across its cells' widths, whatever its cell
            // margin: every kind ends where a code block does, short of the right edge (a
            // table, with a narrower margin, ended 7 px further right than code above it).
            let widths = fit_widths(&old.iter().map(|c| c.width).collect::<Vec<_>>(), (room - BLOCK_INSET - r.indent).max(720));
            fitted.insert(row.start, (r.margin, widths.clone()));
            let cells: Vec<CellParms> = widths
                .iter()
                .map(|&w| match kind {
                    Block::Code => {
                        let back = c(code_back(paper, ink, dark));
                        CellParms { width: w, border: [HAIR; 4], border_color: [back; 4], back, ..Default::default() }
                    }
                    Block::Quote => {
                        let bar = c(crate::win::util::mix(paper, ink, if dark { 0.45 } else { 0.32 }));
                        CellParms { width: w, border: [BAR, HAIR, HAIR, HAIR], border_color: [bar, c(paper), c(paper), c(paper)], back: c(paper), ..Default::default() }
                    }
                    Block::Table => {
                        let line = c(crate::win::util::mix(paper, ink, if dark { 0.28 } else { 0.18 }));
                        let back = if i == 0 { c(crate::win::util::mix(paper, ink, if dark { 0.08 } else { 0.045 })) } else { c(paper) };
                        CellParms { width: w, border: [HAIR; 4], border_color: [line; 4], back, ..Default::default() }
                    }
                })
                .collect();
            let same = old.len() == cells.len()
                && old.iter().zip(&cells).all(|(o, n)| {
                    let (ow, nw, ob, nb, oc, nc, ox, nx) = (o.width, n.width, o.border, n.border, o.border_color, n.border_color, o.back, n.back);
                    (ow - nw).abs() < 20 && ob == nb && oc == nc && ox == nx
                });
            if !same {
                set(h, row, &r, &cells);
            }
        }
        if kind == Block::Code {
            // Code is monospace, a little smaller than the text around it, in one colour.
            let t = text(h);
            for &row in &rows {
                let Some(row) = super::blocks::rows(&t).into_iter().find(|x| x.start == row.start) else { continue };
                for (ca, cb) in cells(&t, row) {
                    // Already so: leave it, or every restyle would be a step for Undo.
                    if code_styled(h, ca as i32, cb as i32, c(ink)) {
                        continue;
                    }
                    let cf = code_format(c(ink));
                    // The cell's mark too: an empty line there (a new block's) types in the
                    // mark's look, which was the body font, larger and lower, until the
                    // block was next restyled.
                    for (x, y) in [(cb, cb + 1), (ca, cb)] {
                        set_sel(h, x as i32, y as i32);
                        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
                    }
                    // A little air above the first line and below the last, inside the shading.
                    let mut pf = ParaFormat2::new();
                    pf.mask = PFM_SPACEBEFORE | PFM_SPACEAFTER;
                    send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
                    // The last line is given both: a selection ending at an empty last line's
                    // start leaves that line out of the clearing above, and Enter at the end
                    // of the first line had given it the first line's air above it, so the
                    // last line sat lower than the rest until it held text and was restyled.
                    let last = para_start(&t, cb as i32) as usize;
                    for (at, before, after) in [(ca, CODE_AIR, if last == ca { CODE_AIR } else { 0 }), (last, if last == ca { CODE_AIR } else { 0 }, CODE_AIR)] {
                        set_sel(h, at as i32, at as i32);
                        let mut pf = ParaFormat2::new();
                        pf.mask = PFM_SPACEBEFORE | PFM_SPACEAFTER;
                        pf.space_before = before;
                        pf.space_after = after;
                        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
                    }
                }
            }
        }
        if kind == Block::Table {
            // A cell's text sits in the middle of it: the space a paragraph has after it in
            // the note's rhythm, under a cell's last line, put it 6 px from the top and 10
            // from the bottom.
            let t = text(h);
            for &row in &rows {
                for (_, cb) in cells(&t, row) {
                    set_sel(h, cb as i32, cb as i32);
                    let mut pf = ParaFormat2::new();
                    send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                    if pf.space_after != 0 {
                        pf = ParaFormat2::new();
                        pf.mask = PFM_SPACEAFTER;
                        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
                    }
                }
            }
        }
    }
    set_sel(h, a, b);
}

/// Code's look: monospace, a little smaller than the text around it, plain, in `ink`.
fn code_format(ink: u32) -> CharFormat2 {
    let mut cf = CharFormat2::new();
    cf.mask = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT;
    set_face(&mut cf, mono_face());
    cf.height = body_size() * 9 / 10;
    cf.color = ink;
    cf
}

/// Whether the code cell text from `a` to `b` already has the code look: the monospace
/// face at its size in `ink`, plain, with air above its first line and below its last,
/// and the cell's mark in that face (what an empty line types in).
fn code_styled(h: HWND, a: i32, b: i32, ink: u32) -> bool {
    if !font_is(h, b, b + 1, mono_face(), body_size() * 9 / 10) {
        return false;
    }
    set_sel(h, a, b);
    let cf = char_format(h, SCF_SELECTION);
    let all = CFM_FACE | CFM_SIZE | CFM_COLOR | CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT;
    let face = String::from_utf16_lossy(&cf.face[..cf.face.iter().position(|&c| c == 0).unwrap_or(cf.face.len())]);
    let plain = cf.mask & all == all && cf.effects & (CFM_BOLD | CFM_ITALIC | CFM_UNDERLINE | CFM_STRIKEOUT) == 0;
    if !plain || face != mono_face() || cf.height != body_size() * 9 / 10 || cf.color != ink {
        return false;
    }
    let para = |at: i32| {
        set_sel(h, at, at);
        let mut pf = ParaFormat2::new();
        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
        (pf.space_before, pf.space_after)
    };
    // Every line, not just the first and the last: Enter on the first line gives the new
    // line the first one's air above it, which left a gap in the middle of the block.
    let t = text(h);
    let mut starts = vec![a];
    starts.extend((a..b).filter(|&i| t.get(i as usize) == Some(&CR)).map(|i| i + 1).filter(|&i| i <= b));
    let n = starts.len();
    starts.iter().enumerate().all(|(k, &at)| {
        let (before, after) = para(at);
        before == if k == 0 { CODE_AIR } else { 0 } && after == if k + 1 == n { CODE_AIR } else { 0 }
    })
}

// ---------------------------------------------------------------- making blocks

/// RTF for `rows` x `cols` empty cells of `kind`, full width (`style_blocks` gives them
/// their look once they're in).
fn skeleton(kind: Block, rows: usize, cols: usize, total: i32) -> String {
    let gap = match kind {
        Block::Table => TABLE_GAP,
        Block::Code => CODE_GAP,
        Block::Quote => QUOTE_GAP,
    };
    let mut def = format!("\\trowd\\trgaph{gap}\\trleft0");
    for k in 1..=cols {
        def.push_str(&format!("\\cellx{}", total * k as i32 / cols as i32));
    }
    let mut out = String::from("{\\rtf1\\ansi ");
    for _ in 0..rows {
        out.push_str(&def);
        out.push(' ');
        for _ in 0..cols {
            out.push_str(&format!("\\pard\\intbl\\sl{}\\slmult1\\sa{}\\cell ", LINE_MULT * 12, PARA_AFTER));
        }
        out.push_str("\\row ");
    }
    out.push('}');
    out
}

/// Insert an empty block at paragraph start `at` (the paragraph moves down below it);
/// returns where its first cell's text goes.
pub fn insert_block(h: HWND, at: i32, kind: Block, rows: usize, cols: usize) -> i32 {
    insert_block_over(h, at, 0, kind, rows, cols)
}

/// `insert_block`, replacing the `over` characters at `at`.
pub fn insert_block_over(h: HWND, at: i32, over: i32, kind: Block, rows: usize, cols: usize) -> i32 {
    set_sel(h, at, at + over);
    insert_rtf(h, &skeleton(kind, rows.max(1), cols.clamp(1, MAX_CELLS), column_twips(h)));
    style_blocks(h);
    at + 2
}

/// The cell text range holding `cp`, if it's in a block: (block, cell start, cell end).
pub fn cell_at(h: HWND, t: &[u16], cp: i32) -> Option<(Block, Vec<Row>, usize, usize)> {
    let (kind, rows) = block_at(h, t, cp)?;
    let cp = cp.max(0) as usize;
    let row = *rows.iter().find(|r| r.start <= cp && cp < r.end)?;
    let (a, b) = cells(t, row).into_iter().find(|&(a, b)| a <= cp && cp <= b)?;
    Some((kind, rows, a, b))
}

/// The width text has in the cell holding `cp`, in twips (inside its margins), or None
/// outside a block.
pub(super) fn cell_text_width(h: HWND, t: &[u16], cp: i32) -> Option<i32> {
    let (_, rows, a, _) = cell_at(h, t, cp)?;
    let row = *rows.iter().find(|r| r.start <= a && a < r.end)?;
    let n = cells(t, row).iter().position(|&(x, _)| x == a)?;
    let (r, cs) = get(h, row)?;
    cs.get(n).map(|c| c.width - 2 * r.margin)
}

// ---------------------------------------------------------------- code's language

/// Where a code block's language (the `rust` of ```rust) is in its cell's text `a..b`:
/// it's kept as hidden text at the cell's start, which the RTF keeps while RichEdit draws
/// nothing for it, moves the caret past it, never types into it and leaves it out of a
/// copy. None for code without one. Only hidden text at the very start is it: emoji
/// selectors' stand-ins are hidden too, and so is a link's `HYPERLINK "url"` (which notes
/// opened in 0.6.0 can have in code).
pub fn code_tag(h: HWND, t: &[u16], a: usize, b: usize) -> Option<(usize, usize)> {
    use windows::Win32::UI::Controls::RichEdit::{tomCharFormat, tomTrue};
    // Up to the first link instruction, if one starts it there is none.
    let b = instructions(t, a, b.min(t.len())).first().map_or(b.min(t.len()), |l| l.0);
    let r = range(h, a as i32, a as i32)?;
    let mut x = a;
    // A run of formatting at a time.
    while x < b {
        let (end, hidden) = unsafe {
            r.SetRange(x as i32, x as i32).ok()?;
            r.MoveEnd(tomCharFormat.0, 1).ok()?;
            let end = (r.GetEnd().ok()?.max(0) as usize).clamp(x + 1, b);
            (end, r.GetFont().ok()?.GetHidden().ok()? == tomTrue.0)
        };
        if !hidden {
            break;
        }
        match t[x..end].iter().position(|&c| unmark(c) != c) {
            Some(k) => {
                x += k;
                break;
            }
            None => x = end,
        }
    }
    (x > a).then_some((a, x))
}

/// Where the text of the code cell `a..b` starts, past a language kept at its start.
pub fn past_tag(h: HWND, t: &[u16], a: usize, b: usize) -> usize {
    match code_tag(h, t, a, b) {
        Some((s, e)) if s == a => e,
        _ => a,
    }
}

/// Keep `lang` as the language of the code cell starting at `a` (its text already in).
pub(super) fn set_code_tag(h: HWND, a: i32, lang: &str) {
    if lang.is_empty() {
        return;
    }
    set_sel(h, a, a);
    insert_text(h, lang);
    set_sel(h, a, a + lang.encode_utf16().count() as i32);
    set_effect(h, CFM_HIDDEN, true);
}

/// Put the paragraphs from `a` to `b` (paragraph starts and ends) into a new quote or code
/// block in their place, formatting and all (code keeps only the text).
pub fn wrap(h: HWND, a: i32, b: i32, kind: Block) {
    let t = text(h);
    let body = String::from_utf16_lossy(&t[a as usize..b as usize]);
    let len = b - a;
    let cell = insert_block(h, a, kind, 1, 1);
    let Some(row) = rows(&text(h)).into_iter().find(|r| r.start == a as usize) else { return };
    // The paragraphs are below the block now; once copied in, further down by their length.
    let src = row.end as i32;
    if len > 0 {
        if kind == Block::Code {
            set_sel(h, cell, cell);
            replace_sel(h, &body);
        } else if let (Some(dst), Some(from)) = (range(h, cell, cell), range(h, src, src + len)) {
            unsafe {
                let _ = dst.SetFormattedText(&from);
            }
        }
    }
    let src = src + len;
    let t = text(h);
    let end = if t.get((src + len) as usize) == Some(&CR) { src + len + 1 } else { src + len };
    set_sel(h, src, end);
    replace_sel(h, "");
    // In code a list item is just text; a quote keeps its lists and checklists.
    if kind == Block::Code {
        set_sel(h, cell, cell + len);
        let mut pf = ParaFormat2::new();
        // A checklist item's spacing goes too: code lines keep the note's rhythm.
        pf.mask = PFM_NUMBERING | PFM_STARTINDENT | PFM_OFFSET | PFM_LINESPACING | PFM_SPACEAFTER;
        pf.line_spacing_rule = 5;
        pf.line_spacing = LINE_MULT;
        pf.space_after = PARA_AFTER;
        send(h, EM_SETPARAFORMAT, 0, &pf as *const _ as isize);
    }
    style_blocks(h);
    // The line left below the block, when there's nothing on it, is plain text (it was a
    // list item when one was wrapped).
    let t = text(h);
    if let Some((_, rows)) = block_at(h, &t, cell)
        && let Some(end) = rows.last().map(|r| r.end as i32)
        && para_start(&t, end) == para_end(&t, end)
    {
        set_sel(h, end, end);
        set_numbering(h, 0);
    }
    set_sel(h, cell + len, cell + len);
}

/// Take the quote or code block holding `cp` apart: its text goes back to ordinary
/// paragraphs where it was. Returns where that text now starts and ends.
pub fn unwrap(h: HWND, cp: i32) -> Option<(i32, i32)> {
    let mut t = text(h);
    let (kind, mut rows, ca, mut cb) = cell_at(h, &t, cp)?;
    if kind == Block::Table {
        return None;
    }
    // Code's language goes with the block: hidden in a paragraph, it would be text nobody
    // sees that still went out with the note.
    if kind == Block::Code
        && let Some((s, e)) = code_tag(h, &t, ca, cb)
    {
        set_sel(h, s as i32, e as i32);
        replace_sel(h, "");
        t = text(h);
        (_, rows, _, cb) = cell_at(h, &t, ca as i32)?;
    }
    let row = rows[0];
    let len = (cb - ca) as i32;
    let at = row.end as i32;
    // A paragraph after the block to put the text in.
    set_sel(h, at, at);
    insert_text(h, "\r");
    set_sel(h, at, at);
    if len > 0
        && let (Some(dst), Some(src)) = (range(h, at, at), range(h, ca as i32, cb as i32))
    {
        unsafe {
            let _ = dst.SetFormattedText(&src);
        }
    }
    // Back to body type: code was monospace.
    if kind == Block::Code {
        set_sel(h, at, at + len);
        set_body_format(h);
    }
    set_sel(h, row.start as i32, row.end as i32);
    replace_sel(h, "");
    let start = row.start as i32;
    set_sel(h, start, start);
    Some((start, start + len))
}

/// How far the quote (or code block) `row` is indented under a list item, in levels (0: at
/// the margin).
pub fn quote_level(h: HWND, row: Row) -> usize {
    get(h, row).map_or(0, |(r, _)| if r.indent <= 0 { 0 } else { 1 + level_at(r.indent - BULLET_TEXT) as usize })
}

/// How many blocks the block of `row` is nested in (a quote in a quote: 1).
pub fn depth(t: &[u16], row: Row) -> usize {
    rows(t).iter().filter(|p| p.start < row.start && row.end <= p.end).count()
}

/// Quotes go this deep, as RichEdit nests tables.
pub const MAX_QUOTE_DEPTH: usize = 15;

/// Indent the quote (or code block) `row` to `level`: level 1 lines its bar (or box) up
/// under a list item's text,
/// each level after that a nesting step further, under an item of the level before (the
/// deepest, under an item of the deepest list level).
pub fn set_quote_level(h: HWND, row: Row, level: usize) {
    let Some((mut r, cells)) = get(h, row) else { return };
    let level = level.min(MAX_LEVEL as usize + 1);
    r.indent = if level == 0 { 0 } else { BULLET_TEXT + level_indent(level as i32 - 1) };
    set(h, row, &r, &cells);
    style_blocks(h);
}

// ---------------------------------------------------------------- tables

/// Insert a row like the one holding `cp`, above it or below it; the caret goes into its
/// first cell.
pub fn insert_row(h: HWND, cp: i32, below: bool) {
    let t = text(h);
    let Some((Block::Table, rows, ..)) = cell_at(h, &t, cp) else { return };
    let Some(&row) = rows.iter().find(|r| r.start <= cp as usize && (cp as usize) < r.end) else { return };
    let Some((mut rp, cells)) = get(h, row) else { return };
    // Streamed in at a row's start, RTF would nest a table in that row: RichEdit's own
    // row insertion puts it beside.
    let at = if below { row.end } else { row.start } as i32;
    rp.cb_row = size_of::<RowParms>() as u8;
    rp.cb_cell = size_of::<CellParms>() as u8;
    rp.rows = 1;
    rp.start = at;
    rp.cells = cells.len() as u8;
    set_sel(h, at, at);
    send(h, EM_INSERTTABLE, &rp as *const _ as usize, cells.as_ptr() as isize);
    if let Some(r) = self::rows(&text(h)).into_iter().find(|r| r.start == at as usize) {
        base_spacing(h, r.start as i32, r.end as i32);
    }
    style_blocks(h);
    set_sel(h, at + 2, at + 2);
}

/// Add a column left or right of the cell holding `cp`, in every row of its table.
pub fn insert_column(h: HWND, cp: i32, right: bool) {
    let t = text(h);
    let Some((Block::Table, rows, ca, _)) = cell_at(h, &t, cp) else { return };
    let row0 = *rows.iter().find(|r| r.start <= cp as usize && (cp as usize) < r.end).unwrap_or(&rows[0]);
    let col = cells(&t, row0).iter().position(|&(a, _)| a == ca).unwrap_or(0);
    let at = if right { col + 1 } else { col };
    change_columns(h, &rows, |cells| {
        if cells.len() < MAX_CELLS {
            let mut c = cells[col.min(cells.len() - 1)];
            c.width = cells.iter().map(|c| c.width).sum::<i32>() / (cells.len() as i32 + 1);
            Some((at, c))
        } else {
            None
        }
    });
    let t = text(h);
    let r = super::blocks::rows(&t).into_iter().find(|r| r.start == row0.start).unwrap_or(row0);
    if let Some(&(a, _)) = cells(&t, r).get(at) {
        set_sel(h, a as i32, a as i32);
    }
}

/// Remove the column of the cell holding `cp` from every row of its table (the last
/// column takes the whole table with it).
pub fn delete_column(h: HWND, cp: i32) {
    let t = text(h);
    let Some((Block::Table, rows, ca, _)) = cell_at(h, &t, cp) else { return };
    let row0 = *rows.iter().find(|r| r.start <= cp as usize && (cp as usize) < r.end).unwrap_or(&rows[0]);
    let n = cells(&t, row0).len();
    if n <= 1 {
        delete_block(h, cp);
        return;
    }
    let col = cells(&t, row0).iter().position(|&(a, _)| a == ca).unwrap_or(0);
    // Each row's cell goes, text and all, from the last row up so positions hold.
    for row in rows.iter().rev() {
        let t = text(h);
        let Some(r) = super::blocks::rows(&t).into_iter().find(|x| x.start == row.start) else { continue };
        let Some((a, b)) = cells(&t, r).get(col).copied() else { continue };
        let Some((rp, old)) = get(h, r) else { continue };
        // Empty the cell first: RichEdit drops a cell only when there's nothing in it.
        if b > a {
            set_sel(h, a as i32, b as i32);
            replace_sel(h, "");
        }
        let mut cells: Vec<CellParms> = old.clone();
        cells.remove(col.min(cells.len() - 1));
        let mut rp = rp;
        rp.cell = col as u8;
        set(h, r, &rp, &cells);
    }
    style_blocks(h);
}

fn change_columns(h: HWND, rows: &[Row], add: impl Fn(&[CellParms]) -> Option<(usize, CellParms)>) {
    for row in rows.iter().rev() {
        let t = text(h);
        let Some(r) = super::blocks::rows(&t).into_iter().find(|x| x.start == row.start) else { continue };
        let Some((rp, old)) = get(h, r) else { continue };
        let Some((at, c)) = add(&old) else { continue };
        let mut cells = old.clone();
        cells.insert(at.min(cells.len()), c);
        let mut rp = rp;
        rp.cell = at as u8;
        set(h, r, &rp, &cells);
    }
    style_blocks(h);
}

/// Remove the row holding `cp` (the last row takes the table with it).
pub fn delete_row(h: HWND, cp: i32) {
    let t = text(h);
    let Some((Block::Table, rows, ..)) = cell_at(h, &t, cp) else { return };
    let Some(&row) = rows.iter().find(|r| r.start <= cp as usize && (cp as usize) < r.end) else { return };
    set_sel(h, row.start as i32, row.end as i32);
    replace_sel(h, "");
    set_sel(h, row.start as i32, row.start as i32);
    style_blocks(h);
}

/// Remove the whole block holding `cp`.
pub fn delete_block(h: HWND, cp: i32) {
    let t = text(h);
    let Some((_, rows)) = block_at(h, &t, cp) else { return };
    let (a, b) = (rows[0].start as i32, rows.last().map_or(0, |r| r.end) as i32);
    set_sel(h, a, b);
    replace_sel(h, "");
    set_sel(h, a, a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, ES_MULTILINE, WS_POPUP};

    fn editor() -> HWND {
        let h = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        setup(h);
        h
    }

    fn plain(h: HWND) -> String {
        text(h)
            .iter()
            .map(|&c| match c {
                CR => '/',
                CELL => '|',
                ROW_START => '[',
                ROW_END => ']',
                c => char::from_u32(c as u32).unwrap_or('?'),
            })
            .collect()
    }

    fn shape(h: HWND) -> Vec<(Block, usize, usize)> {
        let t = text(h);
        blocks(h, &t).into_iter().map(|(k, rs)| (k, rs.len(), cells(&t, rs[0]).len())).collect()
    }

    /// A table ends where a code block above it does (a block's box runs across its cells'
    /// widths, whatever its margin), and its cells' last lines have no paragraph space
    /// under them, so the text sits in the middle of the cell.
    #[test]
    fn tables_line_up_with_code() {
        let h = editor();
        load_markdown(h, "T\n```\ncode\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n> quoted");
        set_block_colors(h, 0xFFFFFF, 0x202020, false);
        style_blocks(h);
        let t = text(h);
        let all = blocks(h, &t);
        let width = |rs: &[Row]| get(h, rs[0]).unwrap().1.iter().map(|c| c.width).sum::<i32>();
        assert_eq!(all.len(), 3);
        for (kind, rs) in &all {
            assert!((width(rs) - width(&all[0].1)).abs() <= 2, "{kind:?}: {} for {}", width(rs), width(&all[0].1));
            if *kind == Block::Table {
                for &r in rs {
                    for (_, cb) in cells(&t, r) {
                        set_sel(h, cb as i32, cb as i32);
                        let mut pf = ParaFormat2::new();
                        send(h, EM_GETPARAFORMAT, 0, &mut pf as *mut _ as isize);
                        assert_eq!(pf.space_after, 0);
                    }
                }
            }
        }
    }

    /// A quote or code block in a quote is a row nested in the quote's cell, and fills that
    /// cell's text rather than the note's width, at any width the note is.
    #[test]
    fn nested_blocks_fit_their_cell() {
        use windows_sys::Win32::UI::WindowsAndMessaging::MoveWindow;
        let h = editor();
        load_markdown(h, "T\n> outer\n> > inner\n> ```\n> code\n> ```");
        assert_eq!(plain(h), "T/[/outer/[/inner|]//[/code|]/|]/");
        for w in [400, 260, 600] {
            unsafe { MoveWindow(h, 0, 0, w, 300, 0) };
            style_blocks(h);
            let t = text(h);
            let all = rows(&t);
            assert_eq!(all.len(), 3);
            let (outer, ocells) = get(h, all[0]).unwrap();
            let room = ocells[0].width - 2 * outer.margin;
            for &r in &all[1..] {
                assert!(r.end <= all[0].end, "nested in the quote");
                let (p, cs) = get(h, r).unwrap();
                let used = cs.iter().map(|c| c.width).sum::<i32>() + BLOCK_INSET + p.indent;
                assert!((room - used).abs() <= 2, "width {w}: {used} in {room}");
            }
            assert!(ocells[0].width + BLOCK_INSET <= column_twips(h) + 2);
            // Drawn inside it: further in than the quote's own text.
            let x = |w: &str| pos_from_char(h, plain(h).find(w).unwrap() as i32).x;
            assert!(x("inner") > x("outer") && x("code") > x("outer"), "width {w}");
        }
        // Stored and opened again: the same rows.
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        assert_eq!(plain(h2), plain(h));
        assert_eq!(shape(h2), vec![(Block::Quote, 1, 1), (Block::Quote, 1, 1), (Block::Code, 1, 1)]);
    }

    #[test]
    fn widths_fill_exactly() {
        assert_eq!(fit_widths(&[1000, 1000, 2000], 8000), vec![2000, 2000, 4000]);
        let w = fit_widths(&[1, 1, 1], 1000);
        assert_eq!(w.iter().sum::<i32>(), 1000);
    }

    #[test]
    fn rows_and_cells_in_text() {
        let s: Vec<u16> = "a\r\u{FFF9}\rx\u{7}y\u{7}\u{FFFB}\rb".encode_utf16().collect();
        let r = rows(&s);
        assert_eq!(r, vec![Row { start: 2, end: 10 }]);
        assert_eq!(cells(&s, r[0]), vec![(4, 5), (6, 7)]);
    }

    #[test]
    fn make_wrap_unwrap() {
        let h = editor();
        insert_text(h, "Title\rone\rtwo\rend");
        // A table after the title.
        let at = 6;
        insert_block(h, at, Block::Table, 2, 3);
        assert_eq!(shape(h), vec![(Block::Table, 2, 3)]);
        assert_eq!(plain(h), "Title/[/|||]/[/|||]/one/two/end");
        // Two paragraphs into a quote, formatting kept.
        let s = plain(h);
        let (a, b) = (s.find("one").unwrap() as i32, s.find("two").unwrap() as i32 + 3);
        wrap(h, a, b, Block::Quote);
        assert_eq!(plain(h), "Title/[/|||]/[/|||]/[/one/two|]/end");
        assert_eq!(shape(h).last().copied(), Some((Block::Quote, 1, 1)));
        // And back.
        let q = plain(h).find("one").unwrap() as i32;
        unwrap(h, q);
        assert_eq!(plain(h), "Title/[/|||]/[/|||]/one/two/end");
        // Code keeps the lines.
        let s = plain(h);
        let (a, b) = (s.find("one").unwrap() as i32, s.find("two").unwrap() as i32 + 3);
        wrap(h, a, b, Block::Code);
        assert_eq!(plain(h), "Title/[/|||]/[/|||]/[/one/two|]/end");
        assert_eq!(shape(h).last().copied(), Some((Block::Code, 1, 1)));
        // The kinds survive the RTF a note is stored as.
        let rtf = get_rtf(h);
        let h2 = editor();
        set_rtf(h2, &rtf);
        assert_eq!(shape(h2), shape(h));
    }

    #[test]
    fn table_rows_and_columns() {
        let h = editor();
        insert_text(h, "T\r");
        insert_block(h, 2, Block::Table, 2, 2);
        // Fill: a|b / c|d
        for (n, s) in ["d", "c", "b", "a"].iter().enumerate() {
            let t = text(h);
            let all: Vec<(usize, usize)> = rows(&t).iter().flat_map(|r| cells(&t, *r)).collect();
            let (x, _) = all[3 - n];
            set_sel(h, x as i32, x as i32);
            replace_sel(h, s);
        }
        assert_eq!(plain(h), "T/[/a|b|]/[/c|d|]/");
        let b = plain(h).find('b').unwrap() as i32;
        insert_column(h, b, true);
        assert_eq!(shape(h), vec![(Block::Table, 2, 3)]);
        assert_eq!(plain(h), "T/[/a|b||]/[/c|d||]/");
        let a = plain(h).find('a').unwrap() as i32;
        delete_column(h, a);
        assert_eq!(plain(h), "T/[/b||]/[/d||]/");
        let d = plain(h).find('d').unwrap() as i32;
        insert_row(h, d, false);
        assert_eq!(plain(h), "T/[/b||]/[/||]/[/d||]/");
        let d = plain(h).find('d').unwrap() as i32;
        delete_row(h, d);
        assert_eq!(plain(h), "T/[/b||]/[/||]/");
        delete_block(h, 3);
        assert_eq!(plain(h), "T/");
    }

    #[test]
    fn markdown_round_trip() {
        let h = editor();
        let md = "Title\nIntro line\n> quoted **bold** words\n> second\n```\nfn main() {\n    x | y\n}\n```\n| Name | Qty |\n| --- | ---: |\n| Eggs | 12 |\n| **Milk** | 1 a\\|b |\n<p align=\"center\">Centered</p>\nend";
        load_markdown(h, md);
        assert_eq!(shape(h), vec![(Block::Quote, 1, 1), (Block::Code, 1, 1), (Block::Table, 3, 2)]);
        assert_eq!(to_markdown(h, true), format!("# {md}"));
        // Stored, reloaded: the same.
        let rtf = get_rtf(h);
        let h2 = editor();
        set_rtf(h2, &rtf);
        assert_eq!(to_markdown(h2, true), format!("# {md}"));
        // The light form (titles, search, previews): no marks, no alignment.
        // Export writes Markdown's blank lines and line breaks around blocks; importing that
        // brings the same note back.
        let exported = crate::common::store::hard_breaks(&to_markdown(h, true));
        let h3 = editor();
        load_markdown(h3, exported.strip_prefix("# ").unwrap());
        assert_eq!(to_markdown(h3, true), format!("# {md}"), "{exported}");
        let light = to_markdown(h, false);
        assert!(light.contains("> quoted bold words\n> second\n```\nfn main() {"), "{light}");
        assert!(light.contains("| Name | Qty |\n| --- | ---: |\n| Eggs | 12 |\n| Milk | 1 a\\|b |\nCentered\nend"), "{light}");
    }

    #[test]
    fn links_survive_rtf() {
        let md = "T\nPlain with a [link](https://example.com).";
        let h = editor();
        load_markdown(h, md);
        assert_eq!(to_markdown(h, true), format!("# {md}"), "before storing");
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        assert_eq!(to_markdown(h2, true), format!("# {md}"), "after storing:\n{}", get_rtf(h));
    }

    #[test]
    fn quotes_hold_lists_and_sit_under_items() {
        let h = editor();
        let md = "T\n> quoted **bold**\n> - bullet\n>   - nested\n> - [ ] task\n> - [x] done\n> 1. one\n> 2. two\n> <p align=\"center\">centred</p>\nafter\n- item\n  > under the item\n  > second line\nend";
        load_markdown(h, md);
        assert_eq!(shape(h), vec![(Block::Quote, 1, 1), (Block::Quote, 1, 1)]);
        let t = text(h);
        let q = blocks(h, &t);
        assert_eq!((quote_level(h, q[0].1[0]), quote_level(h, q[1].1[0])), (0, 1));
        assert_eq!(to_markdown(h, true), format!("# {md}"));
        // Stored and opened again: the same.
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        assert_eq!(to_markdown(h2, true), format!("# {md}"));
        // The light text keeps the markers previews draw.
        let light = to_markdown(h, false);
        assert!(light.contains("> - bullet\n>   - nested\n> [ ] task\n> [x] done\n> 1. one"), "{light}");
        assert!(light.contains("- item\n  > under the item"), "{light}");
    }

    /// Checklist lines are as tall as prose, in a quote too, even when an item is empty:
    /// its circle is body-sized and would otherwise hang below a shorter line (and a
    /// quote's bar). In every typeface and size.
    #[test]
    fn task_lines_are_body_lines() {
        for (font, size) in (0..4).flat_map(|f| (0..3).map(move |s| (f, s))) {
            set_typography(font, size);
            let h = editor();
            apply_base_format(h, 0x1D1D1F, 0xFFFFFF);
            load_markdown(h, "T\none\ntwo\n- [ ] x\n- [ ] y\n> qone\n> qtwo\n> - [ ] alpha\n> - [ ] beta\nend");
            style_boxes(h);
            let y = |h: HWND, w: &str| {
                let t = text(h);
                let w: Vec<u16> = w.encode_utf16().collect();
                let i = t.windows(w.len()).position(|v| v == &w[..]).unwrap();
                pos_from_char(h, i as i32).y
            };
            let pitch = y(h, "two") - y(h, "one");
            let at = format!("font {font} size {size}");
            assert_eq!(y(h, "x") - y(h, "two"), pitch, "{at}: prose to task");
            assert_eq!(y(h, "y") - y(h, "x"), pitch, "{at}: task to task");
            assert_eq!(y(h, "qtwo") - y(h, "qone"), pitch, "{at}: quoted prose");
            assert_eq!(y(h, "alpha") - y(h, "qtwo"), pitch, "{at}: quoted prose to task");
            assert_eq!(y(h, "beta") - y(h, "alpha"), pitch, "{at}: quoted task to task");
            let end = y(h, "end");
            for w in ["y", "beta"] {
                let t = text(h);
                let w: Vec<u16> = w.encode_utf16().collect();
                let i = t.windows(w.len()).position(|v| v == &w[..]).unwrap() as i32;
                set_sel(h, i, i + w.len() as i32);
                replace_sel(h, "");
                style_boxes(h);
            }
            assert_eq!(y(h, "end"), end, "{at}: empty items");
            // After the RTF a note is stored as, the same.
            let h2 = editor();
            set_rtf(h2, &get_rtf(h));
            apply_base_format(h2, 0x1D1D1F, 0xFFFFFF);
            assert_eq!(y(h2, "end"), end, "{at}: stored");
        }
        set_typography(0, 1);
    }

    /// Code follows the text size in Settings (a little smaller than the text around it),
    /// as a note opens and when the size is changed with it open.
    #[test]
    fn code_follows_the_text_size() {
        let code_size = |h: HWND| {
            let t = text(h);
            let c = t.iter().position(|&c| c == b'x' as u16).unwrap() as i32;
            set_sel(h, c, c + 1);
            let cf = char_format(h, SCF_SELECTION);
            (cf.height, face_is(&cf, mono_face()))
        };
        let h = editor();
        for size in 0..3 {
            set_typography(0, size);
            // Opened, as a note is: content in, then the note's look.
            let o = editor();
            load_markdown(o, "T\nbody\n```\nx = 1\n```");
            apply_base_format(o, 0x1D1D1F, 0xFFFFFF);
            style_blocks(o);
            assert_eq!(code_size(o), (body_size() * 9 / 10, true), "opened at size {size}");
            // Open while the size changes: the note's look again.
            if size == 0 {
                load_markdown(h, "T\nbody\n```\nx = 1\n```");
            }
            bulk(h, || {
                apply_base_format(h, 0x1D1D1F, 0xFFFFFF);
                style_blocks(h);
            });
            assert_eq!(code_size(h), (body_size() * 9 / 10, true), "changed to size {size}");
        }
        set_typography(0, 1);
    }

    /// A checklist line holding something taller than text (a picture, large type) grows
    /// to hold it, rather than being clipped to a body line.
    #[test]
    fn tall_task_lines_grow() {
        let h = editor();
        apply_base_format(h, 0x1D1D1F, 0xFFFFFF);
        load_markdown(h, "T\none\ntwo\n- [ ] pic\n- [ ] after");
        style_boxes(h);
        let y = |h: HWND, w: &str| {
            let t = text(h);
            let w: Vec<u16> = w.encode_utf16().collect();
            let i = t.windows(w.len()).position(|v| v == &w[..]).unwrap();
            pos_from_char(h, i as i32).y
        };
        let pitch = y(h, "two") - y(h, "one");
        assert_eq!(y(h, "after") - y(h, "pic"), pitch, "a body line to start with");
        // A picture 120 DIP tall after the item's text.
        let t = text(h);
        let at = t.windows(3).position(|v| v == [b'p' as u16, b'i' as u16, b'c' as u16]).unwrap() as i32 + 3;
        set_sel(h, at, at);
        insert_rtf(h, &format!("{{\\rtf1 {}}}", test_picture(1, 1, 3600, 1800)));
        style_boxes(h);
        let tall = y(h, "after") - y(h, "pic");
        assert!(tall > pitch * 3, "the line is {tall} px, a body line {pitch}");
        // And so it stays once stored and opened again.
        let h2 = editor();
        set_rtf(h2, &get_rtf(h));
        apply_base_format(h2, 0x1D1D1F, 0xFFFFFF);
        assert_eq!(y(h2, "after") - y(h2, "pic"), tall, "stored");
    }

    /// An empty item is still a task, and `- > text` (a quote as a list item, as other
    /// editors write one) goes under the item before it.
    #[test]
    fn empty_tasks_and_item_quotes() {
        let h = editor();
        load_markdown(h, "T\n- [ ]\n> - [ ]\n- a\n  - > under a's item");
        let lines = |md: String| md.lines().map(str::trim_end).collect::<Vec<_>>().join("\n");
        assert_eq!(lines(to_markdown(h, true)), "# T\n- [ ]\n> - [ ]\n- a\n    > under a's item");
        assert_eq!(lines(to_markdown(h, false)), "T\n[ ]\n> [ ]\n- a\n    > under a's item");
        // The light form imports as the same note.
        let h2 = editor();
        load_markdown(h2, &to_markdown(h, false));
        assert_eq!(lines(to_markdown(h2, true)), lines(to_markdown(h, true)));
    }

    /// The text's bottom is measured, not guessed: a tall last line (a picture, a big
    /// font) ends below where a body line would.
    #[test]
    fn content_bottom_follows_a_tall_last_line() {
        let h = editor();
        insert_text(h, "T\rone\rtwo");
        let t = text(h);
        let two = t.iter().position(|&c| c == b'w' as u16).unwrap() as i32 - 1;
        let top = pos_from_char(h, two).y;
        let normal = content_bottom(h).unwrap() - top;
        assert!(normal > 5, "a line has height: {normal}");
        set_sel(h, two, two + 3);
        let mut cf = CharFormat2::new();
        cf.mask = CFM_SIZE;
        cf.height = 1200;
        send(h, EM_SETCHARFORMAT, SCF_SELECTION, &cf as *const _ as isize);
        let top = pos_from_char(h, two).y;
        let tall = content_bottom(h).unwrap() - top;
        assert!(tall > normal * 2, "{tall} vs {normal}");
    }

    /// Enter at the end of a note makes an empty last line: the text runs down to its
    /// bottom, so the view may scroll to it (and its caret) and isn't pulled back.
    #[test]
    fn content_bottom_counts_an_empty_last_line() {
        let h = editor();
        insert_text(h, "T\rone\rtwo");
        let one_line = content_bottom(h).unwrap() - pos_from_char(h, text(h).len() as i32 - 3).y;
        let before = content_bottom(h).unwrap();
        let end = text(h).len() as i32;
        set_sel(h, end, end);
        replace_sel(h, "\r");
        let after = content_bottom(h).unwrap();
        assert!(after - before >= one_line - 1, "{before} then {after}, a line {one_line}");
        assert!(after >= pos_from_char(h, text(h).len() as i32).y + one_line - 1);
        // An empty note is one empty line.
        let empty = editor();
        assert!(content_bottom(empty).unwrap() > 5);
    }

    #[test]
    fn resizing_is_not_an_undo_step() {
        use windows_sys::Win32::UI::WindowsAndMessaging::MoveWindow;
        const EM_UNDO: u32 = 0x00C7;
        let h = editor();
        load_markdown(h, "T\n> quote\n| a | b |\n| --- | --- |\n| 1 | 2 |\nend");
        send(h, EM_EMPTYUNDOBUFFER, 0, 0);
        let end = text(h).len() as i32;
        set_sel(h, end, end);
        replace_sel(h, " typed");
        // The note is made narrower: its blocks are refitted.
        unsafe { MoveWindow(h, 0, 0, 250, 300, 0) };
        style_blocks(h);
        // Ctrl+Z takes back the typing, not the refit.
        assert_eq!(send(h, EM_UNDO, 0, 0), 1);
        assert!(!String::from_utf16_lossy(&text(h)).contains("typed"));
    }

    /// Notes written every way the editor can hold them: each must come back the same from
    /// the editor, from the RTF a note is stored as, and from a file exported and imported.
    #[test]
    fn every_combination_round_trips() {
        let cases = [
            // Each block at the start (under the title), alone, and at the end.
            "T\n> q",
            "T\n```\ncode\n```",
            "T\n| a | b |\n| --- | --- |\n| 1 | 2 |",
            "T\nbefore\n> q",
            // Every pair of blocks, back to back.
            "T\n> q\n```\nc\n```",
            "T\n```\nc\n```\n> q",
            "T\n> q\n| a |\n| --- |\n| 1 |",
            "T\n| a |\n| --- |\n| 1 |\n> q",
            "T\n```\nc\n```\n| a |\n| --- |\n| 1 |",
            "T\n| a |\n| --- |\n| 1 |\n```\nc\n```",
            "T\n> one\ntext\n> two",
            // Marks and links inside a quote and a table.
            "T\n> **bold** _italic_ <u>under</u> ~~strike~~ and a [link](https://example.com)",
            "T\n| **b** | _i_ | [l](https://example.com) |\n| --- | :---: | ---: |\n| <u>u</u> | ~~s~~ | x\\|y |",
            "T\n| one<br>two | three |\n| --- | --- |\n| 1 | 2 |",
            // Markdown's own characters are only text in code.
            "T\n```\n- not a list\n> not a quote\n| not | a table |\n**not bold** _x_ [x](y)\n    indented\n```",
            // Lists inside a quote, and quotes under list items.
            "T\n> - a\n>   - b\n>     - c\n> 1. one\n> 2. two\n> - [ ] task\n> - [x] done",
            "T\n- item\n  > under it\n- next\n  - nested\n    > under that",
            // Code under list items, its own indents kept, and a count going on after it.
            "T\n- ab\n  ```rust\n  xyz\n\n    indented\n  ```\n- next\n  - nested\n    ```\n    deeper\n    ```",
            "T\n1. one\n  ```\n  code\n  ```\n2. two",
            "T\n1. first\n  > quoted\n2. second",
            // Alignment, inside and outside blocks.
            "T\n<p align=\"center\">c</p>\n<p align=\"right\">r</p>\n> <p align=\"center\">in a quote</p>",
            // Lists and checklists around blocks.
            "T\n- a\n- b\n> q\n- c\n- [ ] d",
            // Text that only looks like syntax stays text.
            "T\n\\> not a quote\n\\```",
        ];
        for md in cases {
            let want = format!("# {md}");
            let h = editor();
            load_markdown(h, md);
            assert_eq!(to_markdown(h, true), want, "editor: {md}");
            let h2 = editor();
            set_rtf(h2, &get_rtf(h));
            assert_eq!(to_markdown(h2, true), want, "stored: {md}");
            let exported = crate::common::store::hard_breaks(&to_markdown(h, true));
            let h3 = editor();
            load_markdown(h3, exported.strip_prefix("# ").unwrap());
            assert_eq!(to_markdown(h3, true), want, "exported as:\n{exported}");
            // Light text: titles and previews read it.
            let light = to_markdown(h, false);
            assert_eq!(crate::common::store::title_of(&light), "T", "title: {light}");
        }
    }

    /// A new code block's empty line types in code's look from the first letter, wherever
    /// the caret was meanwhile (it typed the body font until the block was next restyled).
    #[test]
    fn a_new_code_block_types_in_code() {
        let code = |h: HWND, a: i32| font_is(h, a, a + 1, mono_face(), body_size() * 9 / 10);
        let h = editor();
        insert_text(h, "Title\r");
        let cell = insert_block(h, 6, Block::Code, 1, 1);
        set_sel(h, 0, 0);
        set_sel(h, cell, cell);
        replace_sel(h, "x");
        assert!(code(h, cell), "{}", plain(h));
        // A block that was made before: restyled so too.
        let h = editor();
        insert_text(h, "Title\r");
        let cell = insert_block(h, 6, Block::Code, 1, 1);
        let mark = cell;
        set_sel(h, mark, mark + 1);
        set_body_format(h);
        assert!(!code(h, mark));
        style_blocks(h);
        assert!(code(h, mark));
        set_sel(h, cell, cell);
        replace_sel(h, "y");
        assert!(code(h, cell));
    }
}
