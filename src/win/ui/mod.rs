// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows and surfaces: the edge deck, note windows, All Notes, Settings, menus, toasts.

pub mod allnotes;
pub mod deck;
pub mod linkbox;
pub mod ghost;
pub mod menu;
pub mod note;
pub mod settings;
pub mod toast;
pub mod tooltip;

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_WORDBREAK};

use crate::common::store::{self, Align};
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::util::{mix, rect};

/// A note's preview body (from `store::body_of`) in `r`, on paper `color`: list markers
/// hang in their own column so wrapped lines line up under the text, nested items step
/// in, checklist items get the editor's circles (ticked ones filled, their text struck
/// and dimmed), and the last line that fits ends in an ellipsis. Quotes get a bar beside
/// them, code blocks a tinted box in a monospace face, and tables a compact grid. Returns
/// the height used and whether all of it fitted.
#[allow(clippy::too_many_arguments)]
pub fn draw_body(c: &mut Canvas, body: &str, f: &Font, r: RECT, color: u8, dark: bool, alpha: f32, s: f32) -> (i32, bool) {
    body_pass(Some(c), body, f, r, color, dark, alpha, s)
}

/// Height of `body` laid out by `draw_body` at width `w`.
pub fn body_height(body: &str, f: &Font, w: i32, s: f32) -> i32 {
    body_pass(None, body, f, crate::win::util::rect(0, 0, w, i32::MAX / 2), 0, false, 1.0, s).0
}

#[allow(clippy::too_many_arguments)]
fn body_pass(mut c: Option<&mut Canvas>, body: &str, f: &Font, r: RECT, color: u8, dark: bool, alpha: f32, s: f32) -> (i32, bool) {
    let paper = crate::win::theme::paper(color, dark);
    let ink = crate::win::theme::paper_ink(color, dark);
    let text_rgb = crate::win::theme::paper_subtext(color, dark);
    let done_rgb = mix(text_rgb, paper, 0.3);
    let struck = f.struck();
    let line_h = (f.ascent + f.descent).max(1);
    let p = Pass { f, r, alpha, s, dark, paper, ink, text: text_rgb, line_h };
    let step = (16.0 * s) as i32;
    // Circle size and column, in proportion to the editor's (diameter ~1.45 cap heights).
    let d = (f.cap as f32 * 1.45).round();
    let box_w = (d + 6.0 * s) as i32;
    // A little air between a block (quote, code, table) and what is around it.
    let gap = (5.0 * s).round() as i32;
    let lines: Vec<&str> = body.lines().collect();
    let (mut y, mut i, mut after_block) = (r.top, 0, false);
    while i < lines.len() {
        let l = lines[i];
        let block = block_at(&lines, i);
        let drawn = block.as_ref().is_some_and(|b| !matches!(b.0, Block::Empty));
        if (drawn || after_block) && y > r.top {
            y += gap;
        }
        // Past the bottom the text is simply cut off; `fade_out` then softens the cut.
        let room = r.bottom - y;
        if room <= 0 {
            return fade_out(c, r, line_h, paper);
        }
        if let Some((b, next)) = block {
            i = next;
            after_block = drawn;
            let cc = c.as_deref_mut();
            let used = match b {
                Block::Quote(q) => quote(cc, &p, q, y),
                Block::Code(code) => code_block(cc, &p, code, y),
                Block::Table { header, align, rows } => table(cc, &p, header, &align, rows, y),
                Block::Empty => Some(0),
            };
            match used {
                Some(h) => y += h,
                None => return fade_out(c, r, line_h, paper),
            }
            continue;
        }
        after_block = false;
        i += 1;
        let t = l.trim_start_matches(' ');
        let x = r.left + ((l.len() - t.len()) / 2) as i32 * step;
        // Text that only looks like a quote or a fence comes escaped (see `store::body_of`).
        let t = store::unescaped(t).unwrap_or(t);
        let (mark, text) = split_marker(t);
        let task = match mark {
            "○  " => Some(false),
            "●  " => Some(true),
            _ => None,
        };
        let tx = match task {
            Some(_) => x + box_w,
            None if mark.is_empty() => x,
            None => x + f.measure(mark).0,
        };
        let w = r.right - tx;
        if w < step {
            continue;
        }
        let (font, rgb) = if task == Some(true) { (&struck, done_rgb) } else { (f, text_rgb) };
        let h = font.wrapped_height(text, w);
        let Some(c) = c.as_deref_mut() else {
            y += h.max(line_h);
            continue;
        };
        match task {
            // A circle never pokes out below the cut.
            Some(_) if room < line_h => {}
            Some(done) => {
                let (cx, cy) = (x as f32 + d / 2.0, (y + f.ascent) as f32 - f.cap as f32 * 0.5);
                let circle = Shape::new(cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0);
                if done {
                    c.shape(&circle, ink, alpha, 1.0, 0.0);
                    crate::win::editor::checks::check(c, cx, cy, d, 0xFFFFFF, alpha, s);
                } else {
                    c.shape(&circle, crate::win::util::mix(ink, paper, 0.42), alpha, 1.0, 1.3 * s);
                }
            }
            None if !mark.is_empty() => {
                let clip = crate::win::util::rect(x - 2, y - line_h, tx - x + 4, r.bottom - y + line_h);
                c.line(mark, f, x, y + f.ascent, tx - x, Some(clip), text_rgb, alpha, DT_LEFT);
            }
            None => {}
        }
        let fits = h <= room;
        let flags = if fits { DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS } else { DT_LEFT | DT_WORDBREAK };
        if fits {
            c.text(text, font, crate::win::util::rect(tx, y, w, h), rgb, alpha, flags);
        } else {
            // Stop at the cut: glyph slack past it would show the tops of the next line
            // below the fade.
            c.text_above(text, font, crate::win::util::rect(tx, y, w, room), r.bottom, rgb, alpha, flags);
            return fade_out(Some(c), r, line_h, paper);
        }
        y += h.max(line_h);
    }
    (y - r.top, true)
}

/// The body ran past the bottom of `r`: fade its last line and a half into the paper, so
/// the cut reads as "continues" rather than as a sliced line.
fn fade_out(c: Option<&mut Canvas>, r: RECT, line_h: i32, paper: u32) -> (i32, bool) {
    if let Some(c) = c {
        let batching = c.batching();
        c.flush();
        let fade = (line_h * 3 / 2).min(r.bottom - r.top);
        for k in 0..fade {
            let row = crate::win::util::rect(r.left - 2, r.bottom - fade + k, r.right - r.left + 4, 1);
            c.fade_rect(row, paper, 1.0 - (k + 1) as f32 / fade as f32);
        }
        if batching {
            c.begin_batch();
        }
    }
    (r.bottom - r.top, false)
}

/// Split a preview line into its list marker ("•  ", "○  ", "1. ", "a. ") and its text.
fn split_marker(t: &str) -> (&str, &str) {
    for m in ["•  ", "○  ", "●  "] {
        if t.starts_with(m) {
            return t.split_at(m.len());
        }
    }
    let n = t.chars().take_while(|c| c.is_ascii_alphanumeric()).count();
    if (1..=5).contains(&n) && t[n..].starts_with(". ") && (t[..n].chars().all(|c| c.is_ascii_digit()) || t[..n].chars().all(|c| c.is_ascii_lowercase())) {
        return t.split_at(n + 2);
    }
    ("", t)
}

/// What every part of one preview is drawn with.
struct Pass<'a> {
    f: &'a Font,
    r: RECT,
    alpha: f32,
    s: f32,
    dark: bool,
    paper: u32,
    ink: u32,
    text: u32,
    line_h: i32,
}

/// A run of preview lines laid out as one piece.
enum Block<'a> {
    Quote(&'a [&'a str]),
    /// The lines between the fences.
    Code(&'a [&'a str]),
    /// No header row when the preview starts partway into the table (`store::body_from`).
    Table { header: Option<&'a str>, align: Vec<Align>, rows: &'a [&'a str] },
    /// A code block with nothing in it, or a table with no rows: nothing to show.
    Empty,
}

/// The block starting at line `i`, and the line after it; None for an ordinary line.
fn block_at<'a>(lines: &'a [&'a str], i: usize) -> Option<(Block<'a>, usize)> {
    let l = lines[i];
    let run = |from: usize, is: fn(&str) -> bool| from + lines[from..].iter().take_while(|l| is(l)).count();
    if store::is_fence(l) {
        // Unclosed, it runs to the end (a preview can stop partway into one).
        let end = lines[i + 1..].iter().position(|l| store::is_fence(l)).map_or(lines.len(), |k| i + 1 + k);
        let code = &lines[i + 1..end];
        let b = if code.iter().all(|l| l.trim().is_empty()) { Block::Empty } else { Block::Code(code) };
        return Some((b, (end + 1).min(lines.len())));
    }
    if store::quoted(l).is_some() {
        let end = run(i, |l| store::quoted(l).is_some());
        return Some((Block::Quote(&lines[i..end]), end));
    }
    let row = |l: &str| store::table_cells(l).is_some();
    if !row(l) {
        return None;
    }
    let (header, align, from) = match (store::table_align(l), lines.get(i + 1).and_then(|n| store::table_align(n))) {
        (Some(align), _) => (None, align, i + 1),
        (None, Some(align)) => (Some(l), align, i + 2),
        _ => return None,
    };
    let end = run(from, row);
    let rows = &lines[from..end];
    let b = if header.is_none() && rows.is_empty() { Block::Empty } else { Block::Table { header, align, rows } };
    Some((b, end))
}

/// A quote's paragraphs beside a soft bar. Returns the height used, or None when it ran
/// past the bottom (drawn down to there).
fn quote(c: Option<&mut Canvas>, p: &Pass, lines: &[&str], y: i32) -> Option<i32> {
    let bar = (2.5 * p.s).max(2.0);
    let tx = p.r.left + (bar + 12.0 * p.s).round() as i32;
    let w = (p.r.right - tx).max(1);
    let paras: Vec<(&str, i32)> = lines
        .iter()
        .map(|l| {
            let t = store::quoted(l).unwrap_or("");
            (t, p.f.wrapped_height(t, w).max(p.line_h))
        })
        .collect();
    let h: i32 = paras.iter().map(|q| q.1).sum();
    let Some(c) = c else { return Some(h) };
    let bottom = p.r.bottom;
    // Along the text's line boxes, a little in from their ends; never below the cut.
    let inset = 2.0 * p.s;
    let (b0, b1) = (y as f32 + inset, ((y + h) as f32 - inset).min(bottom as f32));
    if b1 > b0 {
        c.rrect(p.r.left as f32, b0, bar, b1 - b0, bar / 2.0, p.text, 0.55 * p.alpha);
    }
    let mut ty = y;
    for (t, th) in paras {
        let room = bottom - ty;
        if room <= 0 {
            return None;
        }
        if th > room {
            c.text_above(t, p.f, rect(tx, ty, w, room), bottom, p.text, p.alpha, DT_LEFT | DT_WORDBREAK);
            return None;
        }
        c.text(t, p.f, rect(tx, ty, w, th), p.text, p.alpha, DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS);
        ty += th;
    }
    Some(h)
}

/// A code block: its lines as written, unwrapped, in a monospace face on a tinted box.
/// Returns the height used, or None when it ran past the bottom.
fn code_block(c: Option<&mut Canvas>, p: &Pass, lines: &[&str], y: i32) -> Option<i32> {
    let cf = block_font(p.f, Variant::Code);
    let pad = (6.0 * p.s).round() as i32;
    let lh = (cf.ascent + cf.descent).max(1);
    let h = 2 * pad + lh * lines.len() as i32;
    let Some(c) = c else { return Some(h) };
    let bottom = p.r.bottom;
    let w = p.r.right - p.r.left;
    let tint = mix(p.paper, p.ink, if p.dark { 0.09 } else { 0.06 });
    c.rrect(p.r.left as f32, y as f32, w as f32, h.min(bottom - y) as f32, 5.0 * p.s, tint, p.alpha);
    // A long line ends in an ellipsis; nothing shows below the cut.
    let (x, tw) = (p.r.left + pad, w - 2 * pad);
    let clip = rect(x - 2, y, tw + 4, bottom - y);
    for (k, l) in lines.iter().enumerate() {
        let top = y + pad + k as i32 * lh;
        if top >= bottom {
            break;
        }
        let l: Cow<str> = if l.contains('\t') { l.replace('\t', "    ").into() } else { (*l).into() };
        c.line(&l, &cf, x, top + cf.ascent, tw, Some(clip), p.text, p.alpha, DT_LEFT);
    }
    (h <= bottom - y).then_some(h)
}

/// A table as a compact grid of one-line cells: equal columns, as many as fit at a
/// readable width (the rest are left out), hairlines between rows and columns, the
/// header row in semibold. Returns the height used, or None when it ran past the bottom.
fn table(c: Option<&mut Canvas>, p: &Pass, header: Option<&str>, align: &[Align], rows: &[&str], y: i32) -> Option<i32> {
    let pad_y = (3.0 * p.s).round() as i32;
    let row_h = p.line_h + 2 * pad_y;
    let h = row_h * (header.is_some() as i32 + rows.len() as i32);
    let Some(c) = c else { return Some(h) };
    let cells: Vec<Vec<String>> = header.into_iter().chain(rows.iter().copied()).map(|r| store::table_cells(r).unwrap_or_default().into_iter().map(store::cell_text).collect()).collect();
    let cols = cells.iter().map(Vec::len).max().unwrap_or(0) as i32;
    let w = p.r.right - p.r.left;
    let shown = cols.min(w / ((56.0 * p.s) as i32).max(1)).max(1);
    let edge = |k: i32| p.r.left + w * k / shown;
    let pad_x = (6.0 * p.s).round() as i32;
    let bottom = p.r.bottom;
    let hair = 0.15 * p.alpha;
    let head = header.map(|_| block_font(p.f, Variant::Header));
    for (k, row) in cells.iter().enumerate() {
        let top = y + k as i32 * row_h;
        if top >= bottom {
            break;
        }
        if k > 0 {
            c.rrect(p.r.left as f32, top as f32, w as f32, 1.0, 0.0, p.ink, hair);
        }
        let font = if k == 0 { head.as_ref().unwrap_or(p.f) } else { p.f };
        let cut = (top + row_h).min(bottom);
        for (j, t) in (0..shown).zip(row) {
            let x0 = edge(j) + if j > 0 { pad_x } else { 0 };
            let x1 = edge(j + 1) - if j + 1 < shown { pad_x } else { 0 };
            let flag = match align.get(j as usize) {
                Some(Align::Center) => DT_CENTER,
                Some(Align::Right) => DT_RIGHT,
                _ => DT_LEFT,
            };
            c.line(t, font, x0, top + pad_y + font.ascent, x1 - x0, Some(rect(x0, top, x1 - x0, cut - top)), p.text, p.alpha, flag);
        }
    }
    for j in 1..shown {
        c.rrect(edge(j) as f32, y as f32, 1.0, h.min(bottom - y) as f32, 0.0, p.ink, hair);
    }
    (h <= bottom - y).then_some(h)
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    Code,
    Header,
}

thread_local! {
    /// Fonts made from a body font for its code blocks and table headers, kept beside it:
    /// previews are drawn on every frame of an animation. Holding the body font keeps it
    /// alive, so no other font can come to look like it (`Font::same`).
    static VARIANTS: RefCell<Vec<(Font, Variant, Font)>> = const { RefCell::new(Vec::new()) };
    static MONO: Cell<Option<&'static str>> = const { Cell::new(None) };
}

fn block_font(f: &Font, v: Variant) -> Font {
    if let Some(hit) = VARIANTS.with(|m| m.borrow().iter().find(|e| e.1 == v && e.0.same(f)).map(|e| e.2.clone())) {
        return hit;
    }
    let made = match v {
        Variant::Code => Font::new(f.size() * 0.9, 400, mono_face()),
        Variant::Header => f.variant(f.size(), 600),
    };
    VARIANTS.with(|m| {
        let mut m = m.borrow_mut();
        // Only a few body fonts are in use at a time (the deck's, a flying card's).
        if m.len() >= 8 {
            m.remove(0);
        }
        m.push((f.clone(), v, made.clone()));
    });
    made
}

/// Cascadia Mono where it is installed (Windows 11), Consolas everywhere else.
fn mono_face() -> &'static str {
    MONO.with(|m| {
        let face = m.get().unwrap_or_else(|| if crate::win::render::dw::installed("Cascadia Mono") { "Cascadia Mono" } else { "Consolas" });
        m.set(Some(face));
        face
    })
}
