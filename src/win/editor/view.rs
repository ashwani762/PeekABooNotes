// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor on screen in one piece. RichEdit draws its text into a canvas (its text
//! services' `TxDraw`), what goes over the text goes on there (colour emoji, the checklist
//! circles and their ticks, a picked picture's frame, the faded edges), and the window gets
//! it all in one copy, in its WM_PAINT.
//!
//! Drawn straight onto the window, as they were, the overlays went on after RichEdit had
//! drawn, each reading the window back: the screen could show RichEdit's own frame in
//! between, the circles gone, the faded line below at full strength, the emoji in black
//! and white, the caret hidden.
//!
//! Changes made outside a paint (`present`: a key, a restyle, a frame of a tick or of the
//! wheel's glide) are drawn whole and compared with what's on screen a row at a time: only
//! the rows that differ are painted, so a restyle that changed nothing puts nothing on
//! screen.
//!
//! The caret is drawn in the frame too (`Caret`), and Windows' own is kept unseen.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateFontIndirectW, DeleteObject, EndPaint, GdiFlush, GetDC, GetTextMetricsW, GetUpdateRect, IntersectClipRect, InvalidateRect, LOGFONTW, PAINTSTRUCT, ReleaseDC,
    SRCCOPY, SelectObject, SetViewportOrgEx, TEXTMETRICW, UpdateWindow,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetCaretBlinkTime, GetClientRect, GetGUIThreadInfo, IsWindow, IsWindowVisible, KillTimer, SPI_GETCARETTIMEOUT, SPI_GETCARETWIDTH, SetCaretPos, SetTimer, SystemParametersInfoW,
};

use crate::win::editor::{checks, emoji, fade, rich};
use crate::win::render::gfx::{Canvas, Image};

/// What an editor's window shows, as last painted.
struct Shown {
    w: i32,
    h: i32,
    scroll: i32,
    /// Each client row's pixels, hashed.
    rows: Vec<u64>,
}

thread_local! {
    /// What editors are drawn on, kept while anything animates (a glide draws every frame).
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
    /// The editor `present` has just drawn whole on the canvas, for the paint it asks for,
    /// and its rows' hashes.
    static FRESH: Cell<isize> = const { Cell::new(0) };
    static FRESH_ROWS: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    static SHOWN: RefCell<HashMap<isize, Shown>> = RefCell::new(HashMap::new());
    static PAINTER: Cell<Option<Painter>> = const { Cell::new(None) };
}

/// What the note window draws over its editors (a picked picture's frame): editor, canvas,
/// the client row the canvas starts at.
type Painter = fn(HWND, &mut Canvas, i32);

/// Have `f` draw over every editor, after the circles and before the faded edges.
pub fn set_painter(f: Painter) {
    PAINTER.set(Some(f));
}

/// What a drawing of an editor takes in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Parts {
    /// All of it.
    All,
    /// The text as it reads (in colour emoji): what a checklist item's tick fades from.
    Text,
}

/// Draw client rows `top..top + rows` of editor `e`, `w` wide, onto `c` (its row 0 at
/// `top`): RichEdit's text, then what goes over it. False when RichEdit can't draw there.
pub fn compose(e: HWND, c: &mut Canvas, top: i32, w: i32, rows: i32, parts: Parts) -> bool {
    use windows::Win32::Graphics::Gdi::HDC;
    use windows::Win32::System::Com::DVASPECT_CONTENT;
    /// The view a windowed editor shows (TXTVIEW_ACTIVE): its own scroll and selection.
    const ACTIVE_VIEW: i32 = 0;
    let Some(ts) = rich::services(e) else { return false };
    let mut upd = windows::Win32::Foundation::RECT { left: 0, top, right: w, bottom: top + rows };
    let dc = c.dc();
    let drawn = unsafe {
        SetViewportOrgEx(dc, 0, -top, null_mut());
        // No bounds: the editor's own client area, as it lays itself out.
        let r = ts.TxDraw(DVASPECT_CONTENT, 0, null_mut(), null_mut(), HDC(dc as _), HDC(null_mut()), null_mut(), null_mut(), &mut upd, 0, 0, ACTIVE_VIEW);
        SetViewportOrgEx(dc, 0, 0, null_mut());
        r.is_ok()
    };
    if !drawn {
        return false;
    }
    c.make_opaque_rect(RECT { left: 0, top: 0, right: w, bottom: rows });
    let (t, a0, b0) = rich::screen_text(e);
    selected_pictures(e, c, top, rows, &t, a0, b0);
    emoji::draw(e, c, top, &t, a0, b0);
    if parts == Parts::All {
        checks::draw(e, c, top, w, &t, a0, b0);
        if let Some(f) = PAINTER.get() {
            f(e, c, top);
        }
        fade::draw(e, c, top);
    }
    true
}

/// The pictures in editor `e`'s selection, on `c` (its client rows `top..top + rows`):
/// RichEdit draws each as its colour negative, which turned a photo into an X-ray in the
/// middle of a drag across it. Inverted back, and tinted with the selection's colour as
/// the text around it is marked. `t` holds the text on screen, `a0..b0` what shows.
///
/// A picture is drawn from the top of its line, as wide as its character, down to the
/// paper RichEdit leaves under it (it sits a little below the baseline, and its line's
/// spacing comes after): what's inverted is down to the first row of the paper alone.
fn selected_pictures(e: HWND, c: &mut Canvas, top: i32, rows: i32, t: &rich::Span, a0: usize, b0: usize) {
    const OBJECT: u16 = 0xFFFC;
    let (a, b) = rich::sel(e);
    // Hidden (a picture picked, framed instead): drawn as it is.
    if a == b || rich::selection_held_hidden(e) {
        return;
    }
    let Some((paper, _)) = checks::paper(e) else { return };
    let (from, to) = ((a.min(b).max(0) as usize).max(a0).max(t.from), (a.max(b).max(0) as usize).min(b0).min(t.from + t.t.len()));
    for cp in from..to {
        if t.t[cp - t.from] != OBJECT {
            continue;
        }
        let Some(bx) = rich::char_box(e, cp as i32) else { continue };
        let (x0, x1) = (bx.left.max(0), bx.right.min(c.w));
        let (y0, y1) = ((bx.top - top).max(0), (bx.bottom - top).min(rows));
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let mut y = y0;
        while y < y1 && !c.row(y)[x0 as usize..x1 as usize].iter().all(|&p| p & 0xFF_FFFF == paper) {
            y += 1;
        }
        let r = RECT { left: x0, top: y0, right: x1, bottom: y };
        if r.bottom <= r.top {
            continue;
        }
        c.invert_rect(r);
        let hl = unsafe { windows_sys::Win32::Graphics::Gdi::GetSysColor(windows_sys::Win32::Graphics::Gdi::COLOR_HIGHLIGHT) };
        c.fade_rect(r, ((hl & 0xFF) << 16) | (hl & 0xFF00) | ((hl >> 16) & 0xFF), 0.8);
        // Its paragraph mark selected too (Shift+Right from a picked picture): RichEdit marks
        // it with a slim block of the selection's colour, as tall as the picture's line, so
        // a bar ran down the picture's side and on past it through the line's spacing.
        let eop = cp + 1;
        if eop < a.max(b) as usize
            && eop < t.from + t.t.len()
            && t.t[eop - t.from] == rich::CR
            && let Some(m) = rich::char_box(e, eop as i32)
        {
            unmark(c, RECT { left: m.left.max(0), top: (m.top - top).max(0), right: m.right.min(c.w), bottom: (m.bottom - top).min(rows) }, paper);
        }
    }
}

/// Paint out the block RichEdit marks a selected paragraph mark with, in its box `r`: the
/// colour at the top of its middle, wherever it is in the box, goes back to the paper.
fn unmark(c: &mut Canvas, r: RECT, paper: u32) {
    if r.right <= r.left || r.bottom <= r.top {
        return;
    }
    let mark = c.row(r.top)[((r.left + r.right) / 2) as usize] & 0xFF_FFFF;
    if mark == paper {
        return;
    }
    for y in r.top..r.bottom {
        let row = c.row(y)[r.left as usize..r.right as usize].to_vec();
        for (x, p) in (r.left..).zip(row) {
            if p & 0xFF_FFFF == mark {
                c.fill_raw(x, y, x + 1, y + 1, 0xFF00_0000 | paper);
            }
        }
    }
}

/// Client rows `top..top + rows` of editor `e` as their text reads now (`Parts::Text`).
pub fn look(e: HWND, top: i32, rows: i32) -> Option<Image> {
    let w = client(e).0;
    if w <= 0 || rows <= 0 {
        return None;
    }
    let mut c = Canvas::new(w, rows);
    compose(e, &mut c, top, w, rows, Parts::Text).then_some(())?;
    c.snapshot(RECT { left: 0, top: 0, right: w, bottom: rows })
}

fn client(e: HWND) -> (i32, i32) {
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(e, &mut rc) };
    (rc.right, rc.bottom)
}

/// A row's pixels, hashed.
fn hash(px: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in px {
        h = (h ^ *p as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The canvas, at least `w` x `h`, for `f`.
fn with_canvas<R>(w: i32, h: i32, f: impl FnOnce(&mut Canvas) -> R) -> R {
    CANVAS.with(|cell| {
        let mut slot = cell.borrow_mut();
        // Grown, never shrunk: a canvas the size of the note is made once.
        if slot.as_ref().is_none_or(|c| c.w < w || c.h < h) {
            let (cw, ch) = slot.as_ref().map_or((w, h), |c| (c.w.max(w), c.h.max(h)));
            *slot = Some(Canvas::new(cw, ch));
        }
        f(slot.as_mut().unwrap())
    })
}

/// Editor `e`'s WM_PAINT: all of it drawn on the canvas, the part asked for copied to the
/// window. False when RichEdit can't draw it that way (it paints itself then).
pub fn on_paint(e: HWND) -> bool {
    let (w, h) = client(e);
    if rich::services(e).is_none() {
        return false;
    }
    let scroll = rich::scroll_y(e);
    // Moved or resized since it was last painted (RichEdit moves what's on screen itself
    // when it scrolls outside `quiet`): all of it. Otherwise whole rows, so each row on
    // screen is known to be one drawn here.
    let moved = SHOWN.with(|m| m.borrow().get(&(e as isize)).is_none_or(|s| s.w != w || s.h != h || s.scroll != scroll));
    if !moved && paint_blink(e, w, h) {
        return true;
    }
    unsafe {
        let mut u: RECT = std::mem::zeroed();
        if moved {
            InvalidateRect(e, std::ptr::null(), 0);
        } else if GetUpdateRect(e, &mut u, 0) != 0 {
            InvalidateRect(e, &RECT { left: 0, top: u.top, right: w, bottom: u.bottom }, 0);
        }
    }
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let dc = unsafe { BeginPaint(e, &mut ps) };
    let (y0, y1) = (ps.rcPaint.top.max(0), ps.rcPaint.bottom.min(h));
    if w > 0 && y1 > y0 {
        let fresh = FRESH.replace(0) == e as isize;
        let mut rows = if fresh { FRESH_ROWS.with(|r| std::mem::take(&mut *r.borrow_mut())) } else { Vec::new() };
        with_canvas(w, h, |c| {
            // Drawn whole even for a part: what goes over the text looks around it (the
            // paper or selection under a circle).
            if !fresh && frame(e, c, w, h) {
                rows = (0..h).map(|y| hash(&c.row(y)[..w as usize])).collect();
            }
            if rows.len() == h as usize {
                unsafe {
                    // RichEdit's class hands out its parent's DC (CS_PARENTDC): keep to the
                    // editor.
                    IntersectClipRect(dc, 0, 0, w, h);
                    BitBlt(dc, 0, y0, w, y1 - y0, c.dc(), 0, y0, SRCCOPY);
                }
            }
        });
        record(e, (w, h, scroll), moved, &rows, y0, y1);
    }
    unsafe { EndPaint(e, &ps) };
    if !crate::win::render::anim::active() {
        CANVAS.with(|c| c.borrow_mut().take());
    }
    true
}

/// Editor `e`'s window (`w`, `h`, scrolled to `scroll`) shows rows `y0..y1` of a frame whose
/// rows hash to `rows`: all it shows is unknown first when it `moved`.
fn record(e: HWND, (w, h, scroll): (i32, i32, i32), moved: bool, rows: &[u64], y0: i32, y1: i32) {
    SHOWN.with(|m| {
        let mut m = m.borrow_mut();
        let s = m.entry(e as isize).or_insert(Shown { w, h, scroll, rows: Vec::new() });
        if moved || s.rows.len() != h as usize {
            *s = Shown { w, h, scroll, rows: vec![0; h as usize] };
        }
        for y in y0..y1 {
            if let Some(r) = rows.get(y as usize) {
                s.rows[y as usize] = *r;
            }
        }
    });
}

/// Put what changed in editor `e` on screen now, in one paint: it's drawn whole, and the
/// rows that differ from what the window shows are painted. Inside `rich::quiet` it waits
/// for that to finish.
pub fn present(e: HWND) {
    if unsafe { IsWindow(e) == 0 || IsWindowVisible(e) == 0 } || rich::is_quiet(e) {
        return;
    }
    let (w, h) = client(e);
    if w <= 0 || h <= 0 {
        return;
    }
    let Some(changed) = changes(e, w, h) else {
        // RichEdit paints itself.
        unsafe { InvalidateRect(e, std::ptr::null(), 0) };
        return;
    };
    if changed.is_empty() {
        return;
    }
    FRESH.set(e as isize);
    unsafe {
        for (a, b) in changed {
            InvalidateRect(e, &RECT { left: 0, top: a, right: w, bottom: b }, 0);
        }
        UpdateWindow(e);
    }
    FRESH.set(0);
}

/// Editor `e` (`w` x `h`) drawn whole on the canvas, its rows' hashes kept for the paint
/// that shows it (`FRESH_ROWS`): the runs of rows that differ from what its window shows.
/// None when RichEdit can't draw it.
fn changes(e: HWND, w: i32, h: i32) -> Option<Vec<(i32, i32)>> {
    let scroll = rich::scroll_y(e);
    let rows = with_canvas(w, h, |c| frame(e, c, w, h).then(|| (0..h).map(|y| hash(&c.row(y)[..w as usize])).collect::<Vec<u64>>()))?;
    let changed = SHOWN.with(|m| {
        let m = m.borrow();
        let shown = m.get(&(e as isize)).filter(|s| s.w == w && s.h == h && s.scroll == scroll);
        changed_rows(shown.map(|s| &s.rows[..]), &rows)
    });
    FRESH_ROWS.with(|r| *r.borrow_mut() = rows);
    Some(changed)
}

/// The runs of rows (from, to) whose hashes `rows` differ from those `shown` (all of
/// them when what's shown isn't known, or is another size).
fn changed_rows(shown: Option<&[u64]>, rows: &[u64]) -> Vec<(i32, i32)> {
    let shown = shown.filter(|s| s.len() == rows.len());
    let differs = |y: usize| shown.is_none_or(|s| s[y] != rows[y]);
    let mut runs = Vec::new();
    let mut y = 0;
    while y < rows.len() {
        if !differs(y) {
            y += 1;
            continue;
        }
        let a = y;
        while y < rows.len() && differs(y) {
            y += 1;
        }
        runs.push((a as i32, y as i32));
    }
    runs
}

/// Editor `e` is going.
pub fn forget(e: HWND) {
    SHOWN.with(|m| m.borrow_mut().remove(&(e as isize)));
    OWN.with(|o| o.borrow_mut().remove(&(e as isize)));
    CARET.with(|k| {
        let mut k = k.borrow_mut();
        if k.e == e as isize {
            k.e = 0;
            k.at = None;
            unsafe { KillTimer(e, CARET_TIMER) };
        }
    });
}

/// Let the canvas go (with no note open).
pub fn trim() {
    CANVAS.with(|c| c.borrow_mut().take());
    SHOWN.with(|m| m.borrow_mut().retain(|&e, _| unsafe { IsWindow(e as HWND) } != 0));
    OWN.with(|o| o.borrow_mut().retain(|&e| unsafe { IsWindow(e as HWND) } != 0));
    EXTENTS.with(|x| x.borrow_mut().clear());
}

// ---------------------------------------------------------------- the caret

/// The caret's blink: a timer on the editor's window.
pub const CARET_TIMER: usize = 0xCA7;

/// The caret's colour (premultiplied ARGB): the body text's (paper is always light).
const CARET_ARGB: u32 = 0xFF00_0000 | crate::common::theme::PAPER_TEXT;

/// The caret of the editor with the focus, drawn in its frame like everything else.
/// Windows' own is drawn over the window by inverting what's there, outside any frame, and
/// inverted back to take it away: where the view had painted the rows it was on (a key,
/// a restyle, the next letter), taking it away drew it again, in pieces left behind as
/// ticks over and under the letters, and one moved while drawing was held (a typed space)
/// stayed put until the next letter painted. In the frame, it's where the selection is,
/// once, and the next frame drawn without it takes it away wherever it was.
struct Caret {
    /// The editor with the focus (0: none).
    e: isize,
    /// In the shown half of a blink.
    on: bool,
    /// Where it was last drawn (shown or not), in client pixels, and the frame's pixels
    /// under it then, for a blink to put back.
    at: Option<RECT>,
    under: Vec<u32>,
    /// When it last moved: it stops blinking, shown, after Windows' caret timeout.
    since: Instant,
    /// The paint under way is a blink's: just the caret.
    blinking: bool,
}

/// A font, for its extent: face, size in twips, weight, italic, DPI.
type FontKey = ([u16; 32], i32, u16, bool, u32);

thread_local! {
    static CARET: RefCell<Caret> = RefCell::new(Caret { e: 0, on: false, at: None, under: Vec::new(), since: Instant::now(), blinking: false });
    /// Editors whose caret the view draws (`own_caret`).
    static OWN: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
    /// Fonts' (ascent, descent) in px.
    static EXTENTS: RefCell<HashMap<FontKey, (i32, i32)>> = RefCell::new(HashMap::new());
}

/// Draw editor `e`'s caret in its frame, from now on, and keep Windows' own unseen. Only
/// for an editor the view draws (RichEdit's text services there): one that paints itself
/// keeps Windows' caret.
pub fn own_caret(e: HWND) {
    if rich::services(e).is_some() && rich::hide_caret(e) {
        OWN.with(|o| o.borrow_mut().insert(e as isize));
    }
}

/// Editor `e` took the focus (`has`) or lost it: its caret shows, blinking, or goes.
pub fn focus(e: HWND, has: bool) {
    if !OWN.with(|o| o.borrow().contains(&(e as isize))) {
        return;
    }
    let changed = CARET.with(|k| {
        let mut k = k.borrow_mut();
        if has {
            *k = Caret { e: e as isize, on: true, at: None, under: Vec::new(), since: Instant::now(), blinking: false };
            true
        } else if k.e == e as isize {
            k.e = 0;
            k.at = None;
            true
        } else {
            false
        }
    });
    if !changed {
        return;
    }
    if has {
        // Whatever reset the document's look (its RTF put back) leaves Windows' caret unseen.
        rich::hide_caret(e);
    } else {
        unsafe { KillTimer(e, CARET_TIMER) };
    }
    present(e);
}

/// Editor `e`'s caret, if something moved it (a click, an arrow, a command setting the
/// selection), drawn where it is now.
pub fn caret_moved(e: HWND) {
    let Some(at) = CARET.with(|k| {
        let k = k.borrow();
        (k.e == e as isize).then_some(k.at)
    }) else {
        return;
    };
    let (w, h) = client(e);
    if !same(caret_rect(e, w, h), at) {
        present(e);
    }
}

/// What a blink does.
enum Blink {
    /// Nothing (it rests shown, or there's no caret).
    Nothing,
    /// The caret moved without being drawn: a frame, to draw it where it is.
    Frame,
    /// Show or hide it: a paint of just these pixels.
    Caret(RECT),
}

/// A blink of editor `e`'s caret (its `CARET_TIMER`): shown or hidden in a paint of just its
/// own pixels. Moved without being drawn, it's drawn where it is now, in a frame.
pub fn blink(e: HWND) {
    match blink_step(e) {
        Blink::Nothing => {}
        Blink::Frame => present(e),
        Blink::Caret(r) => {
            CARET.with(|k| k.borrow_mut().blinking = true);
            unsafe {
                InvalidateRect(e, &r, 0);
                UpdateWindow(e);
            }
            CARET.with(|k| k.borrow_mut().blinking = false);
        }
    }
}

/// The caret's next blink in editor `e`: shown if hidden and the other way round.
fn blink_step(e: HWND) -> Blink {
    let Some((at, on, since)) = CARET.with(|k| {
        let k = k.borrow();
        (k.e == e as isize).then_some((k.at, k.on, k.since))
    }) else {
        unsafe { KillTimer(e, CARET_TIMER) };
        return Blink::Nothing;
    };
    let (w, h) = client(e);
    if !same(caret_rect(e, w, h), at) {
        return Blink::Frame;
    }
    // Left still for Windows' caret timeout: it stays, shown, and stops blinking.
    let rest = caret_timeout().is_some_and(|t| since.elapsed() >= t);
    // None to show (text selected): the timer goes on looking, so a caret that comes back
    // without anything drawing it (a selection let go) shows within a blink.
    let Some(r) = at else {
        if rest {
            unsafe { KillTimer(e, CARET_TIMER) };
        }
        return Blink::Nothing;
    };
    if rest {
        unsafe { KillTimer(e, CARET_TIMER) };
        if on {
            return Blink::Nothing;
        }
    }
    CARET.with(|k| k.borrow_mut().on = rest || !on);
    Blink::Caret(r)
}

/// A blink's paint, when that's all this paint is: the caret's pixels, shown or not, from
/// what the frame had under it. False for any other paint.
fn paint_blink(e: HWND, w: i32, h: i32) -> bool {
    let Some(r) = CARET.with(|k| {
        let k = k.borrow();
        k.at.filter(|_| k.blinking && k.e == e as isize)
    }) else {
        return false;
    };
    let mut u: RECT = unsafe { std::mem::zeroed() };
    let inside = unsafe { GetUpdateRect(e, &mut u, 0) } != 0 && u.left >= r.left && u.top >= r.top && u.right <= r.right && u.bottom <= r.bottom;
    let Some(px) = caret_pixels(e).filter(|_| inside) else {
        return false;
    };
    let (cw, ch) = (r.right - r.left, r.bottom - r.top);
    let mut c = Canvas::new(cw, ch);
    c.copy_in(RECT { left: 0, top: 0, right: cw, bottom: ch }, &px);
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    unsafe {
        let dc = BeginPaint(e, &mut ps);
        GdiFlush();
        IntersectClipRect(dc, 0, 0, w, h);
        BitBlt(dc, r.left, r.top, cw, ch, c.dc(), 0, 0, SRCCOPY);
        EndPaint(e, &ps);
    }
    blinked(e, r);
    true
}

/// What editor `e`'s caret's place shows now: the caret, or (in the hidden half of a
/// blink) the frame's pixels under it. None without a caret drawn.
fn caret_pixels(e: HWND) -> Option<Vec<u32>> {
    CARET.with(|k| {
        let k = k.borrow();
        let r = k.at.filter(|_| k.e == e as isize)?;
        let n = ((r.right - r.left) * (r.bottom - r.top)) as usize;
        (k.under.len() == n).then(|| if k.on { vec![CARET_ARGB; n] } else { k.under.clone() })
    })
}

/// Editor `e`'s rows under `r` were painted by a blink (or by RichEdit itself): they aren't
/// the frame's any more, and the next frame paints them.
fn blinked(e: HWND, r: RECT) {
    SHOWN.with(|m| {
        if let Some(s) = m.borrow_mut().get_mut(&(e as isize)) {
            for y in r.top.max(0)..r.bottom.min(s.rows.len() as i32) {
                s.rows[y as usize] = 0;
            }
        }
    });
}

/// Whether two places for the caret are the same (or both none).
fn same(a: Option<RECT>, b: Option<RECT>) -> bool {
    let k = |r: Option<RECT>| r.map(|r| (r.left, r.top, r.right, r.bottom));
    k(a) == k(b)
}

/// Editor `e`'s frame, `w` x `h`: `compose`d whole, and its caret over all of it.
fn frame(e: HWND, c: &mut Canvas, w: i32, h: i32) -> bool {
    if !compose(e, c, 0, w, h, Parts::All) {
        return false;
    }
    put_caret(e, c, w, h);
    true
}

/// Editor `e`'s caret onto `c` (its text `compose`d, `w` x `h`) if the screen shows it now,
/// leaving its blink be: for a picture of the window, a closing note's card, which lost it
/// in its first frame.
pub fn caret_onto(e: HWND, c: &mut Canvas, w: i32, h: i32) {
    let shown = CARET.with(|k| {
        let k = k.borrow();
        k.e == e as isize && k.on
    });
    if let Some(r) = caret_rect(e, w, h).filter(|_| shown) {
        c.fill_raw(r.left, r.top, r.right, r.bottom, CARET_ARGB);
    }
}

/// Editor `e`'s caret onto its frame `c`, if it has the focus: shown, or (in the hidden
/// half of a blink) not, and the pixels under it kept for the blink. One that moved shows
/// at once, and blinks from then.
fn put_caret(e: HWND, c: &mut Canvas, w: i32, h: i32) {
    if CARET.with(|k| k.borrow().e) != e as isize {
        return;
    }
    let r = caret_rect(e, w, h);
    let moved = CARET.with(|k| {
        let mut k = k.borrow_mut();
        let moved = !same(r, k.at);
        if moved {
            k.on = true;
            k.since = Instant::now();
        }
        k.at = r;
        k.under = r.map_or_else(Vec::new, |r| c.copy_out(r));
        if let Some(r) = r.filter(|_| k.on) {
            c.fill_raw(r.left, r.top, r.right, r.bottom, CARET_ARGB);
        }
        moved
    });
    if moved && let Some(r) = r {
        // Windows' caret, unseen, goes where it is too, for what follows it (a magnifier, a
        // screen reader).
        if system_caret(e).is_some_and(|s| (s.left, s.top) != (r.left, r.top)) {
            unsafe { SetCaretPos(r.left, r.top) };
        }
        blink_from_now(e);
    }
}

/// Editor `e`'s caret blinks from now, a whole blink shown first (or not at all, with
/// blinking turned off in Windows).
fn blink_from_now(e: HWND) {
    unsafe {
        let t = GetCaretBlinkTime();
        if t == 0 || t == u32::MAX {
            KillTimer(e, CARET_TIMER);
        } else {
            SetTimer(e, CARET_TIMER, t, None);
        }
    }
}

/// After a click in editor `e`: its caret drawn afresh where it is, shown, blinking from
/// now. Clicking out of a selection, the caret stayed away for a second, until its second
/// blink: RichEdit draws what a click changes itself, outside the view's frames (the lines
/// the selection let go of, the caret's among them, in its own look), so what the window
/// was last given there isn't what it shows, and the caret's rows are painted again.
pub fn caret_again(e: HWND) {
    let Some(at) = CARET.with(|k| {
        let k = k.borrow();
        (k.e == e as isize).then_some(k.at)
    }) else {
        return;
    };
    let (w, h) = client(e);
    let now = caret_rect(e, w, h);
    for r in [at, now].into_iter().flatten() {
        blinked(e, r);
    }
    CARET.with(|k| {
        let mut k = k.borrow_mut();
        k.on = true;
        k.since = Instant::now();
    });
    if now.is_some() {
        blink_from_now(e);
    }
    present(e);
}

/// Where editor `e`'s caret goes in its client area (`w` x `h`): at the insertion point, as
/// tall as the font there and as wide as Windows' caret, as RichEdit sizes its own. None
/// with something selected, or out of view.
fn caret_rect(e: HWND, w: i32, h: i32) -> Option<RECT> {
    let (a, b) = rich::sel(e);
    if a != b {
        return None;
    }
    let (x, base) = rich::caret_point(e)?;
    let (ascent, descent) = font_extent(e, &rich::char_format(e, rich::SCF_SELECTION));
    let r = RECT { left: x, top: base - ascent, right: x + caret_width(e), bottom: base + descent };
    let r = RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(w), bottom: r.bottom.min(h) };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// Windows' caret, if editor `e` has it: where RichEdit put it, unseen.
fn system_caret(e: HWND) -> Option<RECT> {
    let mut g: GUITHREADINFO = unsafe { std::mem::zeroed() };
    g.cbSize = size_of::<GUITHREADINFO>() as u32;
    (unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut g) } != 0 && g.hwndCaret == e).then_some(g.rcCaret)
}

/// The caret's width in px: RichEdit's own caret's (Windows' text cursor thickness).
fn caret_width(e: HWND) -> i32 {
    let mut set: u32 = 1;
    unsafe { SystemParametersInfoW(SPI_GETCARETWIDTH, 0, &mut set as *mut _ as _, 0) };
    let set = set.clamp(1, 20) as i32;
    // Its own, unless that's an italic caret's slant (wider than the thickness at its DPI).
    let most = set * (unsafe { GetDpiForWindow(e) }.max(96) as i32 + 95) / 96;
    system_caret(e).map(|r| r.right - r.left).filter(|&w| (1..=most).contains(&w)).unwrap_or(set)
}

/// How long the caret blinks after it last moved, if Windows says (it rests shown then).
fn caret_timeout() -> Option<Duration> {
    let mut ms: u32 = 0;
    let ok = unsafe { SystemParametersInfoW(SPI_GETCARETTIMEOUT, 0, &mut ms as *mut _ as _, 0) } != 0;
    (ok && ms > 0 && ms != u32::MAX).then(|| Duration::from_millis(ms as u64))
}

/// The ascent and descent, in px, of character format `cf`'s font in editor `e`.
fn font_extent(e: HWND, cf: &rich::CharFormat2) -> (i32, i32) {
    const CFE_BOLD: u32 = 0x0001;
    const CFE_ITALIC: u32 = 0x0002;
    let dpi = unsafe { GetDpiForWindow(e) }.max(96);
    let weight = if cf.weight != 0 {
        cf.weight
    } else if cf.effects & CFE_BOLD != 0 {
        700
    } else {
        400
    };
    let key = (cf.face, cf.height, weight, cf.effects & CFE_ITALIC != 0, dpi);
    if let Some(m) = EXTENTS.with(|x| x.borrow().get(&key).copied()) {
        return m;
    }
    let px = ((cf.height as i64 * dpi as i64 + 720) / 1440) as i32;
    let m = unsafe {
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfHeight = -px;
        lf.lfWeight = weight as i32;
        lf.lfItalic = key.3 as u8;
        lf.lfCharSet = cf.charset;
        lf.lfFaceName = cf.face;
        let f = CreateFontIndirectW(&lf);
        let dc = GetDC(null_mut());
        let old = SelectObject(dc, f);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        let ok = GetTextMetricsW(dc, &mut tm) != 0;
        SelectObject(dc, old);
        DeleteObject(f);
        ReleaseDC(null_mut(), dc);
        if ok { (tm.tmAscent, tm.tmDescent) } else { (px * 4 / 5, px / 5) }
    };
    EXTENTS.with(|x| x.borrow_mut().insert(key, m));
    m
}

/// For tests: what an editor's window shows, painted as the view paints it (hidden windows
/// paint nothing, and `present` skips them).
#[cfg(test)]
pub(crate) struct Screen {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

#[cfg(test)]
impl Screen {
    /// Editor `e`'s window, painted whole.
    pub fn new(e: HWND) -> Screen {
        SHOWN.with(|m| m.borrow_mut().remove(&(e as isize)));
        let (w, h) = client(e);
        let mut s = Screen { w, h, px: vec![0; (w * h) as usize] };
        s.present(e);
        s
    }

    /// `present` and the paint it asks for: the band of rows from the first that changed
    /// to the last, all of them once the view has moved.
    pub fn present(&mut self, e: HWND) {
        let (w, h) = (self.w, self.h);
        let scroll = rich::scroll_y(e);
        let moved = SHOWN.with(|m| m.borrow().get(&(e as isize)).is_none_or(|s| s.w != w || s.h != h || s.scroll != scroll));
        let changed = changes(e, w, h).expect("drawn");
        let (Some(first), Some(last)) = (changed.first(), changed.last()) else { return };
        let (y0, y1) = if moved { (0, h) } else { (first.0, last.1) };
        let rows = FRESH_ROWS.with(|r| std::mem::take(&mut *r.borrow_mut()));
        CANVAS.with(|c| {
            let mut c = c.borrow_mut();
            let c = c.as_mut().unwrap();
            for y in y0..y1 {
                self.px[(y * w) as usize..((y + 1) * w) as usize].copy_from_slice(&c.row(y)[..w as usize]);
            }
        });
        record(e, (w, h, scroll), moved, &rows, y0, y1);
    }

    /// A blink of the caret, and what it paints.
    pub fn blink(&mut self, e: HWND) {
        match blink_step(e) {
            Blink::Nothing => {}
            Blink::Frame => self.present(e),
            Blink::Caret(r) => {
                let px = caret_pixels(e).expect("a caret");
                let cw = (r.right - r.left) as usize;
                for y in r.top..r.bottom {
                    let k = (y - r.top) as usize * cw;
                    let at = (y * self.w + r.left) as usize;
                    self.px[at..at + cw].copy_from_slice(&px[k..k + cw]);
                }
                blinked(e, r);
            }
        }
    }
}

/// For tests: editor `e`'s frame, drawn whole now, where its caret is, and whether it's
/// drawn there (not in the hidden half of a blink).
#[cfg(test)]
pub(crate) fn frame_now(e: HWND) -> (Vec<u32>, Option<RECT>, bool) {
    let (w, h) = client(e);
    let mut c = Canvas::new(w, h);
    assert!(frame(e, &mut c, w, h));
    let (at, on) = CARET.with(|k| {
        let k = k.borrow();
        (k.at.filter(|_| k.e == e as isize), k.on)
    });
    (c.copy_out(RECT { left: 0, top: 0, right: w, bottom: h }), at, on)
}

/// For tests: the caret's colour.
#[cfg(test)]
pub(crate) const CARET_COLOR: u32 = CARET_ARGB;

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};

    const PAPER: u32 = 0xF3E9C6;

    /// A checklist note, the title with an emoji and every item with one too.
    fn checklist(w: i32, h: i32) -> HWND {
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, w, h, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        let mut s = String::from("Garden \u{1F331}");
        for i in 1..=20 {
            s.push_str(&format!("\r\u{2610} item number {i} \u{1F680}"));
        }
        rich::insert_text(e, &s);
        rich::apply_base_format(e, 0x303030, PAPER);
        rich::style_boxes(e);
        rich::restyle(e);
        checks::set_style(e, 0x303030, PAPER, 1.0);
        e
    }

    fn drawn(e: HWND, top: i32, w: i32, rows: i32) -> Vec<u32> {
        let mut c = Canvas::new(w, rows);
        assert!(compose(e, &mut c, top, w, rows, Parts::All));
        c.copy_out(RECT { left: 0, top: 0, right: w, bottom: rows })
    }

    /// `drawn`, without the circles and the faded edges (the editor's look let go).
    fn bare(e: HWND, w: i32, h: i32) -> Vec<u32> {
        checks::forget(e);
        let px = drawn(e, 0, w, h);
        checks::set_style(e, 0x303030, PAPER, 1.0);
        px
    }

    /// How far rows `a..b` of `px` (`w` wide) are from the paper, on average.
    fn ink(px: &[u32], w: i32, a: i32, b: i32) -> f32 {
        let rows = &px[(a * w) as usize..(b * w) as usize];
        let d = |p: u32| (0..3).map(|k| ((p >> (8 * k)) & 0xFF).abs_diff((PAPER >> (8 * k)) & 0xFF)).sum::<u32>();
        rows.iter().map(|&p| d(p) as f32).sum::<f32>() / rows.len() as f32
    }

    /// Every frame of the editor has all that goes over its text, drawn with it before it
    /// reaches the screen: each checklist circle, the emoji in colour, both edges faded.
    /// Drawn on the window after RichEdit's own frame, they could miss a frame (circles
    /// gone, emoji black, the faded line at full strength).
    #[test]
    fn a_frame_has_its_circles_emoji_and_faded_edges() {
        let (w, h) = (360, 260);
        let e = checklist(w, h);
        rich::set_scroll_y(e, 60);
        let all = drawn(e, 0, w, h);
        let text = bare(e, w, h);
        // A circle on every item wholly in view (its box is drawn in the paper colour).
        let t = rich::text(e);
        let mut circles = 0;
        for (i, _) in t.iter().enumerate().filter(|&(i, &c)| c == rich::BOX && (i == 0 || t[i - 1] == rich::CR)) {
            let (cx, cy, r) = checks::circle(e, i as i32, t.get(i + 1).copied());
            if cy - r < 20.0 || cy + r > (h - 20) as f32 {
                continue;
            }
            let ring = (cx - (r - 4.0)).round() as i32;
            let (x0, x1) = (ring - 1, ring + 2);
            let row = cy.round() as i32;
            let dark = (x0..x1).any(|x| {
                let (a, b) = (all[(row * w + x) as usize], text[(row * w + x) as usize]);
                ink(&[a], 1, 0, 1) > ink(&[b], 1, 0, 1) + 60.0
            });
            assert!(dark, "item at {i}: a circle at {cx},{cy}");
            circles += 1;
        }
        assert!(circles >= 6, "{circles} items checked");
        // The emoji in colour: some of their pixels far from grey.
        let colourful = all.iter().filter(|&&p| {
            let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
            r.max(g).max(b) - r.min(g).min(b) > 90
        });
        assert!(colourful.count() > 100);
        // The edges fade into the paper: text on their outermost rows all but gone. (Scrolled
        // so that a line runs across each.)
        let edge = |all: &[u32], text: &[u32], a: i32, b: i32| (ink(all, w, a, b), ink(text, w, a, b));
        let (top, bottom) = (60..90)
            .map(|y| {
                rich::set_scroll_y(e, y);
                let (all, text) = (drawn(e, 0, w, h), bare(e, w, h));
                (edge(&all, &text, 0, 4), edge(&all, &text, h - 4, h))
            })
            .fold(((0.0, 0.0), (0.0, 0.0)), |(t, b), (t2, b2)| (if t2.1 > t.1 { t2 } else { t }, if b2.1 > b.1 { b2 } else { b }));
        assert!(top.1 > 20.0 && top.0 < top.1 * 0.3, "top: {top:?}");
        assert!(bottom.1 > 20.0 && bottom.0 < bottom.1 * 0.3, "bottom: {bottom:?}");
        // Drawn a band at a time (a paint of part of it), each row comes out the same.
        let part = drawn(e, 0, w, h);
        for top in [40, 97, 150] {
            let rows = drawn(e, top, w, 50);
            assert_eq!(rows, part[(top * w) as usize..((top + 50) * w) as usize], "rows {top}..");
        }
        unsafe { DestroyWindow(e) };
    }

    /// A picture in a selection is drawn as it is, tinted with the selection's colour, not
    /// as the colour negative RichEdit draws; a picked one (its selection hidden, a frame
    /// round it) as it is.
    #[test]
    fn a_selected_picture_is_tinted_not_inverted() {
        let (w, h) = (400, 300);
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, w, h, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        rich::insert_text(e, "Title\rbefore the picture\r\rafter it");
        let t = rich::text(e);
        let at = t.iter().enumerate().filter(|&(_, &c)| c == rich::CR).nth(1).unwrap().0 as i32 + 1;
        rich::set_sel(e, at, at);
        rich::insert_rtf(e, &format!("{{\\rtf1 {}}}", rich::test_picture(48, 24, 1440, 720)));
        rich::apply_base_format(e, 0x1D1D1F, PAPER);
        checks::set_style(e, 0x303030, PAPER, 1.0);
        let obj = rich::text(e).iter().position(|&c| c == 0xFFFC).unwrap() as i32;
        let draw = |top: i32, rows: i32| {
            let mut c = Canvas::new(w, rows);
            assert!(compose(e, &mut c, top, w, rows, Parts::Text));
            c.copy_out(RECT { left: 0, top: 0, right: w, bottom: rows })
        };
        rich::set_sel(e, 0, 0);
        let plain = draw(0, h);
        // The picture: its grey pixels.
        let grey = |p: u32| p & 0xFF_FFFF == 0x80_8080;
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if grey(plain[(y * w + x) as usize]) {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        let p0 = windows_sys::Win32::Foundation::POINT { x: x0, y: y0 };
        let (pw, ph) = (x1 - x0, y1 - y0);
        assert!(pw > 40 && ph > 20, "{pw} x {ph}");
        let hl = unsafe { windows_sys::Win32::Graphics::Gdi::GetSysColor(windows_sys::Win32::Graphics::Gdi::COLOR_HIGHLIGHT) };
        let hl = ((hl & 0xFF) << 16) | (hl & 0xFF00) | ((hl >> 16) & 0xFF);
        let near = |a: u32, b: u32| (0..3).all(|k| ((a >> (8 * k)) & 0xFF).abs_diff((b >> (8 * k)) & 0xFF) <= 2);
        for focused in [false, true] {
            if focused {
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(e, 0x0007, 0, 0) };
            }
            rich::set_sel(e, obj - 4, obj + 4);
            // Whole, and a band through the picture's middle (a paint of part of it).
            for (top, rows) in [(0, h), (p0.y + 20, 30)] {
                let sel = draw(top, rows);
                let mut checked = 0;
                for y in p0.y.max(top)..(p0.y + ph).min(top + rows) {
                    for x in p0.x..p0.x + pw {
                        let (s, p) = (sel[((y - top) * w + x) as usize], plain[(y * w + x) as usize]);
                        let k = |sh: u32| ((((p >> sh) & 0xFF) as f32 * 0.8 + ((hl >> sh) & 0xFF) as f32 * 0.2).round() as u32) << sh;
                        assert!(near(s, k(16) | k(8) | k(0)), "focused {focused}, at {x},{y}: {s:06x} for {p:06x}");
                        checked += 1;
                    }
                }
                assert!(checked >= 30 * pw.min(96) as usize / 2, "{checked}");
            }
        }
        // The picture and its paragraph mark (Shift+Right from a picked picture): no bar of
        // the selection's colour down its side and below it, nothing changed right of it.
        rich::set_sel(e, obj, obj + 2);
        let with_mark = draw(0, h);
        let m = rich::char_box(e, obj + 1).unwrap();
        assert!(m.left >= p0.x + pw - 1 && m.right > m.left && m.bottom > p0.y + ph, "the mark's box: {} {} {} {}", m.left, m.top, m.right, m.bottom);
        let changed: Vec<(i32, i32)> = (p0.y - 4..(p0.y + ph + 60).min(h))
            .flat_map(|y| (p0.x + pw..w).map(move |x| (x, y)))
            .filter(|&(x, y)| with_mark[(y * w + x) as usize] & 0xFF_FFFF != plain[(y * w + x) as usize] & 0xFF_FFFF)
            .collect();
        assert!(changed.is_empty(), "{} pixels right of the picture changed, from {:?} to {:?}", changed.len(), changed.first(), changed.last());
        // Picked: its selection hidden, drawn as it is.
        rich::set_sel(e, obj, obj + 1);
        rich::hold_selection_hidden(e, true);
        let picked = draw(0, h);
        rich::hold_selection_hidden(e, false);
        for y in p0.y..p0.y + ph {
            for x in p0.x..p0.x + pw {
                assert_eq!(picked[(y * w + x) as usize] & 0xFFFFFF, plain[(y * w + x) as usize] & 0xFFFFFF);
            }
        }
        unsafe { DestroyWindow(e) };
    }

    /// What a change puts on screen is the rows that differ from what it shows.
    #[test]
    fn only_the_rows_that_changed_are_shown_again() {
        let shown = [1u64, 2, 3, 4, 5, 6];
        assert_eq!(changed_rows(Some(&shown), &shown), vec![]);
        assert_eq!(changed_rows(Some(&shown), &[1, 9, 9, 4, 5, 7]), vec![(1, 3), (5, 6)]);
        assert_eq!(changed_rows(Some(&shown), &[0, 2, 3, 4, 5, 6]), vec![(0, 1)]);
        // Not known, or another size: all of it.
        assert_eq!(changed_rows(None, &shown), vec![(0, 6)]);
        assert_eq!(changed_rows(Some(&shown[..3]), &shown), vec![(0, 6)]);
    }
}
