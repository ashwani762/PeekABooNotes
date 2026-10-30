// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! All Notes: a custom-drawn library window.
//!
//! Left: large title, search field, a segmented filter whose selection pill glides, and a
//! list whose selection highlight (tinted with each note's paper colour) springs from row
//! to row. Right: the selected note as a paper card with its actions and colour swatches;
//! opening grows the note window out of that card. Below ~720px the detail pane folds away.
//!
//! Typography: native ClearType text on an opaque canvas, every line placed on an explicit
//! baseline (4px rhythm), icons and dots optically centred on cap height.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::{EM_SETCUEBANNER, EM_SETSEL, MARGINS};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, FadeOut, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::common::store::{body_of, search_rest, title_of};
use crate::win::ui::without_objects;
use crate::win::theme::{self, motion};
use crate::common::i18n::time::{edited_time, relative_time};
use crate::win::util::{WM_MOUSELEAVE, cref, from_wide, key_down, mix, rect, wide, x_of, y_of};

mod editor;
mod list;
mod layout;
mod draw;
mod kbd;
use editor::*;
use list::*;
use layout::*;
use draw::*;
use kbd::*;

const CLASS: &str = "PeekABooLibrary";
const ID_SEARCH: usize = 10;
/// Longest search, in characters: a pasted paragraph fits, a pasted note stops there.
const SEARCH_MAX: usize = 2000;
/// Makes the card's editor once search typing pauses.
const T_SYNC: usize = 1;
/// Brings the rows' times ("Edited 2 min ago") up to date while the window is open.
const T_TIMES: usize = 2;
const TIMES_MS: u32 = 30_000;
/// The filter segments: Notes, Archive, All.
fn segments() -> [&'static str; 3] {
    [t!("segment_notes"), t!("segment_archive"), t!("segment_all")]
}

// Vertical rhythm (device-independent px).
const TITLE_BASE: f32 = 52.0;
const SEARCH_Y: f32 = 68.0;
const SEARCH_H: f32 = 36.0;
const SEG_Y: f32 = 116.0;
const SEG_H: f32 = 32.0;
const LIST_TOP: f32 = 164.0;
const ROW_H: f32 = 68.0;
const ROW_TITLE_BASE: f32 = 27.0;
const ROW_META_BASE: f32 = 49.0;
const TOOLBAR_Y: f32 = 44.0;
const TOOLBAR_H: f32 = 30.0;
/// An icon in a toolbar button, and its gap before the label.
const BUTTON_ICON: f32 = 14.0;
const BUTTON_ICON_GAP: f32 = 6.0;
const CARD_Y: f32 = 92.0;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Min,
    Max,
    Close,
    New,
    Share,
    Settings,
    Search,
    Segment(usize),
    /// A search found nothing here, but the link under it shows the matches elsewhere.
    Elsewhere,
    Row(usize),
    Archive,
    Delete,
    Swatch(usize),
    Card,
    /// Bring the selected note back from its own window to edit it here.
    OpenHere,
}

/// Where the keyboard is: the child controls (the search field, the note editor) or one
/// of the window's own drawn controls. The segments and the swatches are one Tab stop
/// each; Left and Right move within them.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Focus {
    Search,
    Segments,
    List,
    Elsewhere,
    Archive,
    Delete,
    Editor,
    OpenHere,
    Swatches,
    New,
    Share,
    Settings,
}

struct Row {
    id: u64,
    title: String,
    off_deck: bool,
    order: i64,
    body: String,
    /// How long the note's text was when `title` and `body` were made from it.
    text_len: usize,
    time: String,
    created: i64,
    updated: i64,
    color: u8,
    archived: bool,
    archived_at: Option<i64>,
    /// While searching: how well it matched, the matched parts of the title (char
    /// ranges), and a piece of the body around a match the title doesn't show.
    score: i32,
    hits: Vec<(usize, usize)>,
    snippet: Option<String>,
    /// The matches inside `snippet`.
    snippet_hits: Vec<(usize, usize)>,
}

struct Pal {
    window: u32,
    list: u32,
    line: u32,
    text: u32,
    sub: u32,
    /// The search field's shortcut hint, quieter than `sub` but still 4.5:1 on the field.
    ter: u32,
    hover: (u32, f32),
    field: (u32, f32),
    pill: u32,
    danger: u32,
    focus: u32,
}

fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            window: 0x1C1C1E,
            list: 0x232325,
            line: 0x38383A,
            text: 0xF5F5F7,
            sub: 0x98989D,
            ter: 0xA1A1A6,
            hover: (0xFFFFFF, 0.06),
            field: (0x767680, 0.24),
            pill: 0x636366,
            danger: 0xFF453A,
            focus: 0x0A84FF,
        }
    } else {
        Pal {
            window: 0xF5F5F7,
            list: 0xFFFFFF,
            line: 0xE5E5EA,
            text: 0x1D1D1F,
            sub: 0x6E6E73,
            ter: 0x6A6A6F,
            hover: (0x000000, 0.045),
            field: (0x767680, 0.12),
            pill: 0xFFFFFF,
            danger: 0xFF3B30,
            focus: 0x0A84FF,
        }
    }
}

struct Fonts {
    big: Font,
    count: Font,
    row_title: Font,
    row_time: Font,
    seg: Font,
    button: Font,
    chip: Font,
    card_title: Font,
    card_meta: Font,
    card_body: Font,
    empty: Font,
    icons: Font,
    button_icon: Font,
    chrome: Font,
    edit: Font,
    /// The native search EDIT needs a GDI font.
    edit_gdi: HFONT,
}

impl Fonts {
    fn new(s: f32) -> Fonts {
        let f = |px: f32, w: u32, face: &str| Font::new(px * s, w, face);
        let icons = crate::win::ui::note::icon_face();
        Fonts {
            big: f(28.0, FW_SEMIBOLD, theme::FACE_DISPLAY),
            count: f(13.0, FW_NORMAL, theme::FACE_TEXT),
            row_title: f(14.5, FW_SEMIBOLD, theme::FACE_TEXT),
            row_time: f(13.0, FW_NORMAL, theme::FACE_TEXT),
            seg: f(13.0, FW_NORMAL, theme::FACE_TEXT),
            button: f(13.0, FW_SEMIBOLD, theme::FACE_TEXT),
            chip: f(12.5, FW_NORMAL, theme::FACE_TEXT),
            card_title: crate::win::editor::rich::preview_fonts(s).0,
            card_meta: f(12.5, FW_NORMAL, theme::FACE_TEXT),
            card_body: crate::win::editor::rich::preview_fonts(s).1,
            empty: f(15.0, FW_SEMIBOLD, theme::FACE_TEXT),
            icons: f(16.0, FW_NORMAL, icons),
            button_icon: f(13.0, FW_NORMAL, icons),
            chrome: f(10.0, FW_NORMAL, icons),
            edit: f(14.0, FW_NORMAL, theme::FACE_TEXT),
            edit_gdi: crate::win::render::gfx::gdi_font((14.0 * s).round() as i32, FW_NORMAL as i32, theme::FACE_TEXT),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.edit_gdi) };
    }
}

struct Layout {
    w: i32,
    h: i32,
    wide: bool,
    list_w: f32,
    search: Shape,
    seg: Shape,
    list_top: f32,
    row_h: f32,
    detail_x: f32,
    card: Shape,
}

struct Lib {
    hwnd: HWND,
    search: HWND,
    s: f32,
    fonts: Fonts,
    canvas: Option<Canvas>,
    rows: Vec<Row>,
    mode: usize,
    query: String,
    /// Searching found nothing in this list, but this many in the other one (Notes or the
    /// Archive): the empty list offers to show them.
    elsewhere: usize,
    sel: Vec<u64>, // the last entry is the primary selection
    anchor: Option<u64>,
    /// The drawn control with the keys while the window itself has focus, and the swatch
    /// it's on when that's the swatches.
    kbd: Focus,
    swatch_k: usize,
    /// The focus ring shows: the keyboard moved focus since the last click.
    cues: bool,
    hover: Hit,
    pressed: Hit,
    tracking: bool,
    nc_hot: bool,
    last_click: (Instant, u64),
    seg_x: Spring,
    scroll: Spring,
    sel_y: Spring,
    hover_y: Spring,
    hover_a: Spring,
    list_a: Spring,
    detail_a: Spring,
    bar_a: Spring,
    last_scroll: Instant,
    /// The empty list's words, fading in once the last row has faded out of it.
    empty_a: Spring,
    /// What the card showed when last drawn: the one note selected, if one is, and whether
    /// it was open in its own window ("Open Here" in its place).
    card_was: (Option<u64>, bool),
    /// Inline editor hosted in the detail card (a child note window) and its note.
    editor: HWND,
    editor_id: u64,
    /// A row being dragged to a new place (Notes view only).
    drag: Option<ListDrag>,
    /// Rows' visual offsets while they make room for a dragged row, or close the gap a
    /// row left (or open one for a row that came), by note id.
    offs: std::collections::HashMap<u64, Spring>,
    /// The mode and search the rows were last made for: rows are only seen to leave and
    /// come within one list.
    listed: (usize, String),
    /// Rows that just left the list (archived, deleted), fading where they were as the
    /// rows below close up over them.
    leaving: Vec<Leaving>,
    /// Rows that just came into it (an undo), fading in once the rows below have made
    /// most of the room: (seconds still to wait, fade).
    arriving: std::collections::HashMap<u64, (f32, Spring)>,
    /// Seconds the rows wait before closing a gap, while the row that left fades: they
    /// slid up through its words, two rows of text on top of each other for a moment.
    hold: f32,
    /// The selection's colour on its way from the row selected before (`sel_from`, as far
    /// as `sel_k`), and the row and colour it was last drawn for.
    sel_from: u32,
    sel_k: Spring,
    sel_last: Option<(u64, u32)>,
    /// The detail card before another note's took its place, as it was on screen (its
    /// rect and pixels), cross-fading into the new one; and the inline editor's picture,
    /// taken as it closed, for that.
    old_detail: Option<(RECT, Vec<u32>)>,
    editor_pic: Option<(RECT, crate::win::render::gfx::Image)>,
    /// The inline editor, just made and still hidden, drawn as it will show (where, and its
    /// pixels): the card cross-fades from the preview to it, and then the editor is shown.
    editor_first: Option<(RECT, Vec<u32>)>,
    /// Rows or the card just changed: the next frame steps one refresh, however long the
    /// work that changed them (a delete, an undo) kept the frames waiting.
    fresh: bool,
    /// The list before a switch to Notes, Archive or All, as it was on screen (its rect
    /// and pixels), cross-fading into the new one.
    old_list: Option<(RECT, Vec<u32>)>,
}

/// A row that left the list, the place it had, and whether it was the selected one.
struct Leaving {
    row: Row,
    at: usize,
    fade: FadeOut,
    selected: bool,
}

/// How long a row that left takes to fade, before the rows below close up; and how long
/// a row that came waits, as the rows below make room, before it fades in.
const LEAVE_S: f32 = 0.12;
const ARRIVE_WAIT_S: f32 = 0.16;

/// How rows close a gap or open one, and the selection with them.
const GAP: (f32, f32) = (0.9, 0.3);

struct ListDrag {
    id: u64,
    from: usize,
    to: usize,
    /// Pointer offset from the row's top when grabbed.
    grab: f32,
    start_y: i32,
    active: bool,
    /// The dragged row's top, in list space (0 = first row).
    y: f32,
}

type CardKey = (i32, i32, i32, i32, u32, bool);

thread_local! {
    static CARD_CACHE: RefCell<Option<(CardKey, Vec<u32>)>> = const { RefCell::new(None) };
    static SYNCING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LIB: RefCell<Option<Lib>> = const { RefCell::new(None) };
    static START_ARCHIVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The search field's background brush and its colour. Not in `Lib`: the field asks
    /// for it while it draws, which it does at once as the focus leaves it, often from
    /// inside a borrow of `Lib` (a click on a row); it went without, and drew itself white.
    static EDIT_BRUSH: std::cell::Cell<(HBRUSH, u32)> = const { std::cell::Cell::new((null_mut(), u32::MAX)) };
}

fn with<R>(f: impl FnOnce(&mut Lib) -> R) -> Option<R> {
    LIB.with(|l| l.try_borrow_mut().ok().and_then(|mut l| l.as_mut().map(f)))
}

/// The inline editor, when it's showing note `id`.
pub fn editor_for(id: u64) -> Option<HWND> {
    let h = with(|l| (l.editor_id == id && !l.editor.is_null()).then_some(l.editor)).flatten();
    // What's done to it shows (still behind its picture, it would change unseen).
    if h.is_some() {
        reveal_editor();
    }
    h
}

pub fn hwnd() -> HWND {
    with(|l| l.hwnd).unwrap_or(null_mut())
}

/// The note editor inside the detail card, if one is up.
pub fn editor() -> HWND {
    with(|l| l.editor).unwrap_or(null_mut())
}

fn text_w(f: &Font, s: &str) -> i32 {
    f.measure(s).0
}

pub fn show(archived: bool) {
    let existing = with(|l| {
        set_mode(l, if archived { 1 } else { 0 });
        l.hwnd
    });
    if let Some(h) = existing {
        sync_editor();
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
        return;
    }
    START_ARCHIVED.with(|c| c.set(archived));
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    // On the monitor in use, at its scale (the window takes that monitor's DPI): where it
    // was left when that was on this monitor, else in its middle.
    let (work, s) = crate::win::ui::screen::active();
    let saved = app::store(|st| st.d.settings.library_rect).map(|[l, t, r, b]| RECT { left: l, top: t, right: r, bottom: b });
    let r = saved
        .filter(|r| corners(crate::win::ui::screen::of_rect(r).0) == corners(work))
        .and_then(|r| on_screen(r, work, (420.0 * s) as i32))
        .unwrap_or_else(|| crate::win::ui::screen::centred(work, (960.0 * s) as i32, (620.0 * s) as i32));
    let (x, y, w, h) = (r.left, r.top, r.right - r.left, r.bottom - r.top);
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            wide(CLASS).as_ptr(),
            wide(&title(if archived { 1 } else { 0 })).as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            null_mut(),
            null_mut(),
            app::hinst(),
            null_mut(),
        );
        SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

fn corners(r: RECT) -> (i32, i32, i32, i32) {
    (r.left, r.top, r.right, r.bottom)
}

/// Window rect `r` moved (and made smaller where it must be) to lie wholly in `work`, or
/// None when it's narrower or shorter than `min`, a size it can't have been left at.
fn on_screen(r: RECT, work: RECT, min: i32) -> Option<RECT> {
    let (w, h) = ((r.right - r.left).min(work.right - work.left), (r.bottom - r.top).min(work.bottom - work.top));
    if w < min.min(work.right - work.left) || h < min.min(work.bottom - work.top) {
        return None;
    }
    let x = r.left.clamp(work.left, work.right - w);
    let y = r.top.clamp(work.top, work.bottom - h);
    Some(RECT { left: x, top: y, right: x + w, bottom: y + h })
}

/// Keep where the window is and its size, so it opens there again (not while it's
/// minimized or maximized: neither is a place it was put).
fn save_rect(h: HWND) {
    if unsafe { IsIconic(h) != 0 || IsZoomed(h) != 0 } {
        return;
    }
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(h, &mut r) } == 0 {
        return;
    }
    let rect = Some([r.left, r.top, r.right, r.bottom]);
    if app::store(|s| s.d.settings.library_rect) == rect {
        return;
    }
    app::store_mut(|s| s.d.settings.library_rect = rect);
    app::save();
}

/// The window's title (what the taskbar and Alt+Tab show): the Archive's while it shows
/// the archive.
fn title(mode: usize) -> String {
    if mode == 1 { t!("window_archive", t!("app_name")) } else { t!("window_all_notes", t!("app_name")) }
}

/// Select these notes, when All Notes is open (an Undo has just brought them back).
pub fn select(ids: &[u64]) {
    let h = with(|l| {
        let found: Vec<u64> = ids.iter().copied().filter(|id| l.rows.iter().any(|r| r.id == *id)).collect();
        if found.is_empty() {
            return None;
        }
        l.anchor = found.last().copied();
        l.sel = found;
        if let Some(i) = primary_index(l) {
            let lay = layout(l);
            l.sel_y.to(lay.list_top + i as f32 * lay.row_h);
            ensure_visible(l, i);
        }
        swap_detail(l);
        Some(l.hwnd)
    })
    .flatten();
    if let Some(h) = h {
        paint_now(h);
        sync_editor();
        anim::wake(Client::List);
    }
}

pub fn refresh() {
    let h = with(|l| {
        rebuild(l);
        // The note on the card opened in its own window, or came back from it: the card and
        // the "Open Here" standing in for it cross-fade (each went in a frame), the editor
        // that just closed in the picture.
        let now = card_now(l);
        if now.0.is_some() && now.0 == l.card_was.0 && now.1 != l.card_was.1 {
            swap_detail(l);
            anim::wake(Client::List);
        }
        l.card_was = now;
        l.hwnd
    });
    if let Some(h) = h {
        // The note's editor goes (it opened in a window of its own) or comes before the
        // window is drawn, so it's drawn whole once: drawn first, the card where the editor
        // was showed blank for a frame.
        sync_editor();
        paint_now(h);
    }
}

/// Note `id` was typed in. Unless a search or a newest-first order depends on what it
/// says now, only its row is made again (typing never moves a note in or out of a list),
/// and the window is drawn again only when the row reads differently.
pub fn note_edited(id: u64) {
    let full = with(|l| {
        if !l.query.trim().is_empty() || (l.mode != 0 && !editor_focused(l)) {
            return true;
        }
        let mode = l.mode;
        let Some(row) = app::store(|s| {
            s.note(id).filter(|n| match mode {
                1 => n.archived,
                2 => true,
                _ => !n.archived,
            })
            .and_then(|n| row_of(n, &crate::common::search::Query::new(""), false))
        }) else {
            return false;
        };
        let Some(r) = l.rows.iter_mut().find(|r| r.id == id) else { return true };
        let same = r.title == row.title && r.body == row.body && r.time == row.time && edited_time(r.updated) == edited_time(row.updated);
        *r = row;
        if !same {
            unsafe { InvalidateRect(l.hwnd, std::ptr::null(), 0) };
        }
        false
    });
    if full == Some(true) {
        refresh();
    }
}

/// A note made here joins the list where new notes go (the bottom, unless Settings says
/// the top), selected and ready to type in the detail card. Only the deck's own + opens a
/// note window.
fn new_here() {
    let id = app::store_mut(|s| s.create("", ""));
    app::save();
    // Nothing may hide it: a search is cleared (outside the borrow: the search box's
    // change notification comes straight back here) and the Notes view shown.
    if let Some(search) = with(|l| (!l.query.is_empty()).then_some(l.search)).flatten() {
        unsafe { SetWindowTextW(search, wide("").as_ptr()) };
    }
    with(|l| {
        set_mode(l, 0);
        rebuild(l);
        if let Some(i) = l.rows.iter().position(|r| r.id == id) {
            select_row(l, i, false, false);
            ensure_visible(l, i);
        }
    });
    app::refresh();
    if let Some(e) = with(|l| l.editor).filter(|e| !e.is_null()) {
        reveal_editor();
        unsafe { SetFocus(e) };
    }
}

/// Note typography changed: the card's preview fonts (the only ones in the note's face
/// and size) and the inline editor follow. The rest stay, among them the search field's
/// GDI font, which the field holds on to.
pub fn retheme_editor() {
    with(|l| {
        let (title, body) = crate::win::editor::rich::preview_fonts(l.s);
        l.fonts.card_title = title;
        l.fonts.card_body = body;
    });
    restyle_editor();
}

/// The note's paper changed (its colour, or the theme): restyle the inline editor, if
/// there is one, and redraw the card. No fonts are made for it.
fn restyle_editor() {
    // Its picture is out of date now.
    reveal_editor();
    if let Some(e) = with(|l| l.editor)
        && !e.is_null() {
            crate::win::ui::note::retheme(e);
        }
    CARD_CACHE.with(|c| c.borrow_mut().take());
    if let Some(h) = with(|l| l.hwnd) {
        paint_now(h);
    }
}

/// The interface language changed: window title, search placeholder, rows (their
/// times are formatted per language), then repaint.
pub fn relabel() {
    let Some((h, search)) = with(|l| (l.hwnd, l.search)) else { return };
    unsafe {
        SetWindowTextW(h, wide(&title(with(|l| l.mode).unwrap_or(0))).as_ptr());
        SendMessageW(search, EM_SETCUEBANNER, 1, wide(t!("search_placeholder")).as_ptr() as isize);
    }
    refresh();
}

pub fn retheme() {
    let Some((h, search)) = with(|l| (l.hwnd, l.search)) else { return };
    theme::set_dark_titlebar(h, app::dark());
    // The inline editor's paper, the card's cached pixels and the search field (its own
    // control, which only repaints when told) all follow the new theme.
    restyle_editor();
    unsafe {
        InvalidateRect(search, std::ptr::null(), 1);
        InvalidateRect(h, std::ptr::null(), 0);
    }
    paint_now(h);
}

/// Note `id` changed colour: if the inline editor shows it, its paper follows.
pub fn recolor(id: u64) {
    if editor_for(id).is_some() {
        restyle_editor();
    }
}

/// Another note (or several) takes the detail card: what it shows now cross-fades into
/// what it will, rather than the card turning another colour, empty, in one frame and its
/// text fading in after. The inline editor that just closed is in the picture too.
fn swap_detail(l: &mut Lib) {
    let lay = layout(l);
    // The editor's picture: taken as it closed, or now while it's still there.
    let pic = l.editor_pic.take().or_else(|| if l.editor.is_null() || l.old_detail.is_some() { None } else { editor::editor_picture(l.editor) });
    if lay.wide
        && let Some(c) = l.canvas.as_mut()
    {
        let r = rect(lay.list_w as i32 + 1, 0, c.w - lay.list_w as i32 - 1, c.h);
        if l.old_detail.is_none() {
            if let Some((at, img)) = pic {
                c.over(&img, at.left, at.top);
            }
            l.old_detail = Some((r, c.copy_out(r)));
        }
    }
    l.detail_a.snap(0.0);
    l.detail_a.launch(1.0);
    l.fresh = true;
}

/// The card as it shows now (its note's preview) cross-fades to the inline editor's picture
/// (`editor_first`), the editor itself shown when it's over. False when there's no card on
/// screen to fade from.
fn fade_to_editor(l: &mut Lib) -> bool {
    let lay = layout(l);
    let Some(c) = l.canvas.as_mut().filter(|_| lay.wide) else { return false };
    let r = rect(lay.list_w as i32 + 1, 0, c.w - lay.list_w as i32 - 1, c.h);
    l.old_detail = Some((r, c.copy_out(r)));
    l.detail_a.snap(0.0);
    l.detail_a.launch(1.0);
    l.fresh = true;
    anim::wake(Client::List);
    true
}

/// The selection's colour as it shows now, for the note at row `i`.
fn sel_color(l: &Lib, i: usize, dark: bool) -> u32 {
    let now = theme::paper(l.rows[i].color, dark);
    let to = match l.sel_last {
        Some((id, c)) if id != l.rows[i].id || c != now => c,
        _ => now,
    };
    mix(l.sel_from, to, clamp01(l.sel_k.value))
}

pub fn frame(dt: f32) -> bool {
    let r = with(|l| {
        if l.last_scroll.elapsed().as_secs_f32() > 0.9 {
            l.bar_a.to(0.0);
        }
        let dt = if std::mem::take(&mut l.fresh) { dt.min(anim::period()) } else { dt };
        let mut moving = false;
        for sp in [&mut l.seg_x, &mut l.hover_y, &mut l.hover_a, &mut l.list_a, &mut l.detail_a, &mut l.bar_a, &mut l.sel_k] {
            moving |= sp.step(dt);
        }
        // A row that left fades first; then the rows below, and the selection, close up.
        if l.hold > 0.0 {
            l.hold -= dt;
            moving = true;
        } else {
            // The list's scroll too: at its end, one row fewer let it scroll back while the
            // row that left was still fading.
            moving |= l.scroll.step(dt);
            moving |= l.sel_y.step(dt);
            for sp in l.offs.values_mut() {
                moving |= sp.step(dt);
            }
        }
        for g in l.leaving.iter_mut() {
            g.fade.step(dt);
            moving = true;
        }
        // Faded to nothing (this frame shows nothing of it).
        l.leaving.retain(|g| !g.fade.done());
        for (wait, sp) in l.arriving.values_mut() {
            if *wait > 0.0 {
                *wait -= dt;
                moving = true;
            } else {
                moving |= sp.step(dt);
            }
        }
        l.arriving.retain(|_, (_, sp)| sp.value < 0.998);
        // Once the last row that left has faded, the empty list's words fade in.
        if l.leaving.is_empty() && l.empty_a.target < 1.0 {
            l.empty_a.launch(1.0);
        }
        moving |= l.empty_a.step(dt);
        if l.list_a.value >= 0.999 {
            l.old_list = None;
        }
        // The card's cross-fade over, its note's editor comes (it waited: see `sync_editor`),
        // or shows (it came in behind its picture).
        let faded = l.old_detail.is_some() && l.detail_a.value >= 0.999;
        if l.detail_a.value >= 0.999 {
            l.old_detail = None;
        }
        // The selection turns the colour of the note it's now on, from the colour it shows.
        let now = primary_index(l).map(|i| (l.rows[i].id, theme::paper(l.rows[i].color, app::dark())));
        if now != l.sel_last {
            match (l.sel_last, now) {
                (Some((_, was)), Some(_)) => {
                    l.sel_from = mix(l.sel_from, was, clamp01(l.sel_k.value));
                    l.sel_k.snap(0.0);
                    l.sel_k.launch(1.0);
                    moving = true;
                }
                _ => l.sel_k.snap(1.0),
            }
            l.sel_last = now;
        }
        (moving || l.bar_a.value > 0.01, l.hwnd, faded)
    });
    match r {
        Some((moving, h, faded)) => {
            if faded {
                // On to its editor's picture, the editor coming behind it; or, that over, the
                // editor shown.
                reveal_editor();
                sync_editor();
            }
            paint_now(h);
            moving
        }
        None => false,
    }
}

/// What the card shows: the one note selected, if one is, and whether it's open in its
/// own window.
fn card_now(l: &Lib) -> (Option<u64>, bool) {
    (l.sel.last().copied().filter(|_| l.sel.len() == 1), in_window(l).is_some())
}

// ---------------------------------------------------------------- inline editor

// ---------------------------------------------------------------- model

// ---------------------------------------------------------------- layout

// ---------------------------------------------------------------- painting

// ---------------------------------------------------------------- input

enum Action {
    None,
    Edit,
    New,
    Share(Vec<u64>),
    Open(u64, Option<(RECT, f32)>),
    Archive(Vec<u64>, bool),
    Delete(Vec<u64>),
    Color(u64, u8),
    Menu(u64),
    Settings,
    OpenHere(u64),
    ClearSearch,
    /// The Undo toast's Undo, from the keyboard.
    Undo,
    /// The selection moved with the arrow keys.
    Browse,
    Focus(Focus),
    Reorder(Vec<u64>),
    Close,
    Min,
    MaxToggle,
}

fn perform(hwnd: HWND, a: Action) {
    if matches!(a, Action::Browse) {
        // Arrowing through the list, as when typing a search: each note shows its quick
        // preview, and its editor comes once the keys rest (making one for every press
        // was most of what a press cost).
        if with(|l| wanted_editor(l).map(|w| w.0) != Some(l.editor_id)).unwrap_or(false) {
            close_editor();
        }
        paint_now(hwnd);
        unsafe { SetTimer(hwnd, T_SYNC, 250, None) };
        return;
    }
    if matches!(a, Action::Open(..) | Action::Archive(..) | Action::Delete(_) | Action::Color(..) | Action::Share(_)) {
        close_editor();
    }
    perform_inner(hwnd, a);
    // A picture of the editor that closed for it, which no other note took the card from.
    with(|l| l.editor_pic = None);
    sync_editor();
}

fn perform_inner(hwnd: HWND, a: Action) {
    match a {
        Action::None | Action::Browse => {}
        Action::Edit => {
            // Its editor, if the keys moved onto the note a moment ago and it's still to come
            // (or waits for the card's cross-fade: the keys go in now).
            sync_editor_now();
            if let Some(e) = with(|l| l.editor)
                && !e.is_null() {
                    unsafe { SetFocus(e) };
                }
        }
        Action::New => new_here(),
        Action::Undo => app::undo(),
        Action::Settings => crate::win::ui::settings::show(),
        Action::Share(ids) => app::share(hwnd, &ids),
        Action::Reorder(ids) => {
            app::store_mut(|s| s.reorder(&ids));
            app::save();
            app::refresh();
        }
        Action::OpenHere(id) => {
            // Closing saves and flies the window home; on close, All Notes refreshes
            // and its inline editor opens on the note.
            if let Some(h) = app::ui(|u| u.notes.get(&id).copied()) {
                unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
            }
        }
        Action::ClearSearch => {
            if let Some(s) = with(|l| l.search) {
                unsafe { SetWindowTextW(s, wide("").as_ptr()) };
            }
        }
        Action::Focus(f) => focus_to(hwnd, f),
        Action::Open(id, from) => {
            if from.is_some() {
                app::open_note_from(id, from)
            } else {
                app::open_note(id)
            }
        }
        Action::Archive(ids, true) => app::archive(&ids),
        Action::Archive(ids, false) => app::restore(&ids),
        Action::Delete(ids) => {
            if !ids.is_empty() {
                app::delete(ids)
            }
        }
        Action::Color(id, c) => app::note_command(id, app::NC_COLOR0 + c as u32),
        // The editor stays while the menu is open, and is saved and closed for its command.
        Action::Menu(id) => app::note_menu_at_cursor(id, hwnd, close_editor),
        Action::Close => unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        },
        Action::Min => unsafe {
            ShowWindow(hwnd, SW_MINIMIZE);
        },
        Action::MaxToggle => unsafe {
            ShowWindow(hwnd, if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE });
        },
    }
}

/// Whether `e`, a control in the inline editor, is its text with the selection inside the
/// first paragraph (the title).
fn in_title(e: HWND) -> bool {
    use crate::win::editor::rich;
    let mut class = [0u16; 32];
    let n = unsafe { GetClassNameW(e, class.as_mut_ptr(), class.len() as i32) }.max(0) as usize;
    if !String::from_utf16_lossy(&class[..n]).eq_ignore_ascii_case(rich::CLASS) {
        return false;
    }
    rich::sel_in_title(e)
}

/// Whether a key pressed in the note being edited is All Notes' rather than the editor's:
/// Ctrl+Tab leaves the note, and so does Tab in its title, where a tab character has no
/// place (elsewhere in the text Tab is the editor's: lists, tables, code); and Ctrl+F goes
/// to the search box, as the box says it does, from the note too (it was the editor's, which
/// does nothing with it, so the caret kept blinking in the note). AltGr+F is a character.
fn editors_key_is_ours(vk: u16, ctrl: bool, alt: bool, in_title: impl FnOnce() -> bool) -> bool {
    match vk {
        VK_TAB => ctrl || in_title(),
        _ => ctrl && !alt && vk == b'F' as u16,
    }
}

/// Keys for the search box and list that must be seen before the EDIT consumes them,
/// and Ctrl+Tab and Ctrl+F out of the note editor (plain Tab there indents lists).
pub fn pre_translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let Some((hwnd, search, editor)) = with(|l| (l.hwnd, l.search, l.editor)) else { return false };
    let vk = msg.wParam as u16;
    let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
    let in_editor = !editor.is_null() && (msg.hwnd == editor || unsafe { IsChild(editor, msg.hwnd) } != 0);
    if in_editor {
        if !editors_key_is_ours(vk, ctrl, key_down(VK_MENU), || in_title(msg.hwnd)) {
            return false;
        }
    } else if msg.hwnd != hwnd && msg.hwnd != search {
        return false;
    }
    // Any key here takes a tooltip away, the note's own (a picture's name) too: the list
    // moving under it, or a search, leaves it standing for something no longer there.
    crate::win::ui::tooltip::leave(hwnd);
    // The window's own drawn controls have the keys, not the search field.
    let own = msg.hwnd == hwnd;
    let mut repaint = false;
    let action = with(|l| -> Option<Action> {
        if own && !ctrl && matches!(vk, VK_SPACE | VK_RETURN)
            && let Some(a) = activate(l)
        {
            return Some(a);
        }
        match vk {
            VK_TAB => Some(next_focus(l, shift).map_or(Action::None, Action::Focus)),
            VK_LEFT | VK_RIGHT if own && nudge(l, if vk == VK_LEFT { -1 } else { 1 }) => {
                repaint = true;
                Some(Action::None)
            }
            VK_DOWN => {
                move_selection(l, 1);
                Some(Action::Browse)
            }
            VK_UP => {
                move_selection(l, -1);
                Some(Action::Browse)
            }
            // The first or last note: from the list, and from the search field while it's
            // empty (with words in it, they move its caret).
            VK_HOME | VK_END if own || l.query.is_empty() => {
                let n = l.rows.len() as i32;
                move_selection(l, if vk == VK_HOME { -n } else { n });
                Some(Action::Browse)
            }
            // Into the note's text; in its own window when it has none here (several
            // selected, a narrow window, or it's open already).
            VK_RETURN => Some(if l.editor.is_null() && wanted_editor(l).is_none() { open_selected(l) } else { Action::Edit }),
            VK_ESCAPE => {
                if !l.query.is_empty() {
                    // Not here: the EDIT's change notification re-enters this window
                    // and must find it unborrowed, or the cleared query is lost.
                    Some(Action::ClearSearch)
                } else {
                    Some(Action::Close)
                }
            }
            // As in a note window: Ctrl+Shift+Del and Ctrl+Shift+A, from the search field too.
            VK_DELETE if own || (ctrl && shift) => Some(Action::Delete(l.sel.clone())),
            _ if ctrl && shift && vk == b'A' as u16 => Some(if l.sel.is_empty() { Action::None } else { click(l, Hit::Archive) }),
            // To the search box, its text selected; from outside this borrow, as the box's
            // focus notification (it draws as focused) comes back into this window.
            _ if ctrl && vk == b'F' as u16 => Some(Action::Focus(Focus::Search)),
            _ if ctrl && vk == b'A' as u16 && msg.hwnd == hwnd => {
                let first = l.rows.first().map(|r| r.id);
                l.sel = l.rows.iter().map(|r| r.id).filter(|x| Some(*x) != first).collect();
                if let Some(id) = first {
                    l.sel.push(id);
                }
                swap_detail(l);
                anim::wake(Client::List);
                Some(Action::None)
            }
            _ if ctrl && vk == b'N' as u16 => Some(Action::New),
            // While the Undo toast shows, Ctrl+Z here (not in the note editor, whose own
            // undo comes first) takes back the archive or delete, as its button does.
            _ if ctrl && !shift && vk == b'Z' as u16 && app::can_undo() => Some(Action::Undo),
            _ => None,
        }
    })
    .flatten();
    match action {
        Some(a) => {
            perform(hwnd, a);
            if repaint {
                paint_now(hwnd);
            }
            true
        }
        None => false,
    }
}

fn on_create(hwnd: HWND) {
    let s = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
    let fonts = Fonts::new(s);
    let search = unsafe {
        CreateWindowExW(0, wide("EDIT").as_ptr(), wide("").as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32, 0, 0, 10, 10, hwnd, ID_SEARCH as _, app::hinst(), null_mut())
    };
    unsafe {
        SendMessageW(search, WM_SETFONT, fonts.edit_gdi as usize, 1);
        SendMessageW(search, EM_SETCUEBANNER, 1, wide(t!("search_placeholder")).as_ptr() as isize);
        SendMessageW(search, windows_sys::Win32::UI::Controls::EM_SETLIMITTEXT, SEARCH_MAX, 0);
        let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        DwmExtendFrameIntoClientArea(hwnd, &m);
    }
    theme::set_dark_titlebar(hwnd, app::dark());
    theme::set_round_corners(hwnd, false);
    let mode = if START_ARCHIVED.with(|c| c.get()) { 1 } else { 0 };
    let now = Instant::now();
    let mut lib = Lib {
        hwnd,
        search,
        s,
        fonts,
        canvas: None,
        rows: Vec::new(),
        mode,
        query: String::new(),
        elsewhere: 0,
        sel: Vec::new(),
        anchor: None,
        kbd: Focus::List,
        swatch_k: 0,
        cues: false,
        hover: Hit::None,
        pressed: Hit::None,
        tracking: false,
        nc_hot: false,
        last_click: (now, 0),
        seg_x: Spring::unit(mode as f32, motion::MOMENTUM),
        scroll: Spring::px(0.0, (1.0, 0.24)),
        sel_y: Spring::px(0.0, (0.9, 0.3)),
        hover_y: Spring::px(0.0, (1.0, 0.16)),
        hover_a: Spring::unit(0.0, motion::SNAPPY),
        list_a: Spring::fade(0.0, motion::DEFAULT),
        detail_a: Spring::fade(0.0, motion::DEFAULT),
        bar_a: Spring::unit(0.0, motion::DEFAULT),
        last_scroll: now,
        empty_a: Spring::fade(1.0, motion::DEFAULT),
        card_was: (None, false),
        editor: null_mut(),
        editor_id: 0,
        drag: None,
        offs: std::collections::HashMap::new(),
        listed: (usize::MAX, String::new()),
        leaving: Vec::new(),
        arriving: std::collections::HashMap::new(),
        hold: 0.0,
        sel_from: 0,
        sel_k: Spring::fade(1.0, motion::DEFAULT),
        sel_last: None,
        old_detail: None,
        editor_pic: None,
        editor_first: None,
        fresh: false,
        old_list: None,
    };
    lib.list_a.to(1.0);
    LIB.with(|l| *l.borrow_mut() = Some(lib));
    unsafe { SetTimer(hwnd, T_TIMES, TIMES_MS, None) };
    with(|l| {
        rebuild(l);
        place_search(l);
    });
    sync_editor();
    anim::wake(Client::List);
}

/// The EDIT sits inside the drawn field, its text on the field's optical centre line.
fn place_search(l: &Lib) {
    let lay = layout(l);
    let s = l.s;
    let sf = lay.search;
    let f = &l.fonts.edit;
    let eh = f.ascent + f.descent;
    let base = (sf.y + sf.h / 2.0) as i32 + f.cap / 2;
    unsafe {
        MoveWindow(l.search, (sf.x + 36.0 * s) as i32, base - f.ascent, (sf.w - 36.0 * s - 84.0 * s).max(20.0) as i32, eh, 1);
    }
}

fn field_color(dark: bool) -> u32 {
    let p = pal(dark);
    mix(p.list, p.field.0, p.field.1)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                on_create(hwnd);
                return 0;
            }
            WM_NCCALCSIZE if wp != 0 => {
                if IsZoomed(hwnd) != 0 {
                    // A maximised borderless window overhangs the monitor by its frame.
                    let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                    let dpi = GetDpiForWindow(hwnd);
                    let f = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                    let r = &mut p.rgrc[0];
                    r.left += f;
                    r.right -= f;
                    r.top += f;
                    r.bottom -= f;
                }
                return 0;
            }
            WM_NCACTIVATE => return DefWindowProcW(hwnd, msg, wp, -1),
            WM_NCHITTEST => {
                let mut p = POINT { x: x_of(lp), y: y_of(lp) };
                ScreenToClient(hwnd, &mut p);
                let r = with(|l| {
                    let (w, h) = client_size(hwnd);
                    let b = (6.0 * l.s) as i32;
                    if IsZoomed(hwnd) == 0 {
                        let (le, ri, to, bo) = (p.x < b, p.x >= w - b, p.y < b, p.y >= h - b);
                        let edge = match (le, ri, to, bo) {
                            (true, _, true, _) => Some(HTTOPLEFT),
                            (_, true, true, _) => Some(HTTOPRIGHT),
                            (true, _, _, true) => Some(HTBOTTOMLEFT),
                            (_, true, _, true) => Some(HTBOTTOMRIGHT),
                            (true, ..) => Some(HTLEFT),
                            (_, true, ..) => Some(HTRIGHT),
                            (_, _, true, _) => Some(HTTOP),
                            (_, _, _, true) => Some(HTBOTTOM),
                            _ => None,
                        };
                        if let Some(e) = edge {
                            return e;
                        }
                    }
                    let lay = layout(l);
                    match hit_test(l, p.x, p.y) {
                        Hit::Max => HTMAXBUTTON, // lets Windows 11 show Snap Layouts on hover
                        Hit::None if (p.y as f32) < SEARCH_Y * l.s - 4.0 * l.s => HTCAPTION,
                        Hit::None if (p.x as f32) >= lay.detail_x && (p.y as f32) < lay.card.y => HTCAPTION,
                        _ => HTCLIENT,
                    }
                });
                return r.unwrap_or(HTCLIENT) as LRESULT;
            }
            WM_NCMOUSEMOVE => {
                let hot = wp as u32 == HTMAXBUTTON;
                let changed = with(|l| {
                    let before = l.hover;
                    if hot {
                        l.hover = Hit::Max;
                        if !l.nc_hot {
                            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                            TrackMouseEvent(&mut tme);
                            l.nc_hot = true;
                        }
                    } else if l.hover == Hit::Max {
                        l.hover = Hit::None;
                    }
                    before != l.hover
                });
                if changed == Some(true) {
                    paint_now(hwnd);
                }
                if hot {
                    return 0;
                }
            }
            0x02A2 /* WM_NCMOUSELEAVE */ => {
                with(|l| {
                    l.nc_hot = false;
                    if l.hover == Hit::Max {
                        l.hover = Hit::None;
                    }
                });
                paint_now(hwnd);
            }
            WM_NCLBUTTONDOWN if wp as u32 == HTMAXBUTTON => {
                with(|l| l.pressed = Hit::Max);
                return 0;
            }
            WM_NCLBUTTONUP if wp as u32 == HTMAXBUTTON => {
                with(|l| l.pressed = Hit::None);
                perform(hwnd, Action::MaxToggle);
                return 0;
            }
            WM_GETMINMAXINFO => {
                let s = with(|l| l.s).unwrap_or(1.0);
                let mm = &mut *(lp as *mut MINMAXINFO);
                mm.ptMinTrackSize = POINT { x: (420.0 * s) as i32, y: (420.0 * s) as i32 };
                return 0;
            }
            // Another app in front, or the window minimized: a tooltip of ours (a picture's
            // name in the note) goes, rather than stay over whatever is there now.
            WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => {
                crate::win::ui::tooltip::leave(hwnd);
            }
            WM_SIZE => {
                if wp as u32 == SIZE_MINIMIZED {
                    crate::win::ui::tooltip::leave(hwnd);
                }
                with(|l| {
                    place_search(l);
                    clamp_scroll(l);
                    if let Some(i) = primary_index(l) {
                        let lay = layout(l);
                        l.sel_y.snap(lay.list_top + i as f32 * lay.row_h);
                    }
                });
                paint_now(hwnd);
                sync_editor();
                return 0;
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                render(dc);
                EndPaint(hwnd, &ps);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_CTLCOLOREDIT => {
                let dark = app::dark();
                let rgb = field_color(dark);
                let (mut brush, had) = EDIT_BRUSH.with(|b| b.get());
                if had != rgb || brush.is_null() {
                    if !brush.is_null() {
                        DeleteObject(brush);
                    }
                    brush = CreateSolidBrush(cref(rgb));
                    EDIT_BRUSH.with(|b| b.set((brush, rgb)));
                }
                SetTextColor(wp as HDC, cref(pal(dark).text));
                SetBkColor(wp as HDC, cref(rgb));
                return brush as LRESULT;
            }
            WM_COMMAND => {
                let (id, code) = (wp & 0xFFFF, ((wp >> 16) & 0xFFFF) as u32);
                if id == ID_SEARCH {
                    if code == EN_CHANGE {
                        // All of it: the field holds up to SEARCH_MAX characters.
                        let n = GetWindowTextLengthW(lp as HWND).max(0) as usize + 1;
                        let mut buf = vec![0u16; n];
                        GetWindowTextW(lp as HWND, buf.as_mut_ptr(), n as i32);
                        let q = from_wide(&buf);
                        with(|l| {
                            // The list fades in as a search starts or ends, not again with
                            // every letter: each fade redraws the window many times over.
                            let started = l.query.is_empty() != q.is_empty();
                            l.query = q;
                            l.scroll.snap(0.0);
                            if started {
                                l.list_a.snap(0.55);
                                l.list_a.to(1.0);
                            }
                            // Typing selects the best match; clearing the search keeps
                            // the note you found, scrolled into view.
                            let cleared = l.query.is_empty();
                            let before = l.sel.last().copied();
                            if !cleared {
                                l.sel.clear();
                            }
                            rebuild(l);
                            // Still the best match: its card stays as it is.
                            if l.sel.last().copied() == before {
                                l.detail_a.snap(1.0);
                            }
                            if let Some(i) = primary_index(l).filter(|_| cleared) {
                                let lay = layout(l);
                                l.sel_y.snap(lay.list_top + i as f32 * lay.row_h);
                                ensure_visible(l, i);
                            }
                        });
                        // While typing, the best match changes with each letter: its card
                        // shows the quick preview, and the editor comes once typing pauses
                        // (making one per letter made every keystroke lag).
                        if with(|l| wanted_editor(l).map(|w| w.0) != Some(l.editor_id)).unwrap_or(false) {
                            close_editor();
                        }
                        paint_now(hwnd);
                        SetTimer(hwnd, T_SYNC, 250, None);
                    } else {
                        // Focus in or out: the field drawn as it is now (`draw` asks the
                        // system which).
                        paint_now(hwnd);
                        sync_editor();
                    }
                }
                return 0;
            }
            WM_TIMER if wp == T_SYNC => {
                KillTimer(hwnd, T_SYNC);
                sync_editor();
                return 0;
            }
            WM_TIMER if wp == T_TIMES => {
                // Redrawn only when a row's time reads differently now.
                let changed = with(|l| {
                    let mut changed = false;
                    for r in &mut l.rows {
                        let now = relative_time(r.updated);
                        if now != r.time {
                            r.time = now;
                            changed = true;
                        }
                    }
                    changed
                });
                if changed == Some(true) {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                return 0;
            }
            WM_SETFOCUS => {
                // Back from the search field or the note (Esc there): the list has the keys,
                // or with no list, the search field.
                let empty = with(|l| {
                    if matches!(l.kbd, Focus::Search | Focus::Editor) {
                        l.kbd = Focus::List;
                    }
                    (l.rows.is_empty() && l.kbd == Focus::List).then_some(l.search)
                });
                if let Some(Some(search)) = empty {
                    SetFocus(search);
                }
                paint_now(hwnd);
                return 0;
            }
            WM_KILLFOCUS => {
                paint_now(hwnd);
                return 0;
            }
            WM_PARENTNOTIFY if matches!((wp & 0xFFFF) as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN) => {
                // A click in the search field or the note hides the focus ring, as one here does.
                with(|l| l.cues = false);
            }
            WM_CHAR => {
                // Typing while the list has focus goes to the search field.
                let c = wp as u32;
                if ((0x20..0x7F).contains(&c) || c > 0x9F)
                    && let Some(search) = with(|l| l.search) {
                        SetFocus(search);
                        SendMessageW(search, WM_CHAR, wp, lp);
                    }
                return 0;
            }
            WM_MOUSEMOVE => {
                let (x, y) = (x_of(lp), y_of(lp));
                if with(|l| l.drag.is_some()).unwrap_or(false) {
                    with(|l| drag_to(l, y));
                    return 0;
                }
                let changed = with(|l| {
                    if !l.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        l.tracking = true;
                    }
                    let hit = hit_test(l, x, y);
                    header_tip(l, hit);
                    if hit == l.hover {
                        return false;
                    }
                    l.hover = hit;
                    if let Hit::Row(i) = hit {
                        let lay = layout(l);
                        let target = lay.list_top + i as f32 * lay.row_h;
                        if l.hover_a.value < 0.05 {
                            l.hover_y.snap(target);
                        } else {
                            l.hover_y.to(target);
                        }
                        l.hover_a.to(1.0);
                    } else {
                        l.hover_a.to(0.0);
                    }
                    anim::wake(Client::List);
                    true
                });
                let _ = changed;
                return 0;
            }
            WM_MOUSELEAVE => {
                crate::win::ui::tooltip::leave(hwnd);
                with(|l| {
                    l.tracking = false;
                    if l.hover != Hit::Max {
                        l.hover = Hit::None;
                    }
                    l.hover_a.to(0.0);
                });
                anim::wake(Client::List);
                return 0;
            }
            WM_LBUTTONDOWN => {
                crate::win::ui::tooltip::mute();
                let (x, y) = (x_of(lp), y_of(lp));
                let action = with(|l| {
                    let hit = hit_test(l, x, y);
                    l.pressed = hit;
                    // A click hides the focus ring, and the keys go back to the list (or
                    // the segments, when they were clicked): never to a button out of sight.
                    // A row clicked with Ctrl or Shift, adding to the selection, is work the
                    // keys share: the ring stays, on the row clicked.
                    if !(matches!(hit, Hit::Row(_)) && (key_down(VK_CONTROL) || key_down(VK_SHIFT))) {
                        l.cues = false;
                    }
                    l.kbd = if matches!(hit, Hit::Segment(_)) { Focus::Segments } else { Focus::List };
                    match hit {
                        Hit::Search => {
                            SetFocus(l.search);
                            Action::None
                        }
                        Hit::Segment(i) => {
                            set_mode(l, i); // responds on press
                            Action::None
                        }
                        Hit::Elsewhere => {
                            set_mode(l, if l.mode == 1 { 0 } else { 1 });
                            Action::None
                        }
                        Hit::Row(i) => {
                            // Selection responds on press; a quick second press opens the note
                            // in its own window (the card beside the list already edits it).
                            let id = l.rows[i].id;
                            let dbl = l.last_click.1 == id && l.last_click.0.elapsed().as_millis() < GetDoubleClickTime() as u128;
                            l.last_click = (Instant::now(), id);
                            let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
                            select_row(l, i, ctrl, shift);
                            SetFocus(hwnd);
                            if can_drag(l) && !ctrl && !shift && !dbl {
                                let lay = layout(l);
                                let grab = y as f32 - (lay.list_top + i as f32 * lay.row_h - l.scroll.value);
                                l.drag = Some(ListDrag { id, from: i, to: i, grab, start_y: y, active: false, y: i as f32 * lay.row_h });
                                SetCapture(hwnd);
                            }
                            if dbl {
                                l.last_click.1 = 0;
                                open_selected(l)
                            } else {
                                Action::None
                            }
                        }
                        _ => {
                            SetCapture(hwnd);
                            Action::None
                        }
                    }
                });
                paint_now(hwnd);
                if let Some(a) = action {
                    perform(hwnd, a);
                }
                return 0;
            }
            WM_LBUTTONUP => {
                let (x, y) = (x_of(lp), y_of(lp));
                let action = with(|l| {
                    if l.drag.as_ref().is_some_and(|dr| dr.active) {
                        l.pressed = Hit::None;
                        return drop_drag(l);
                    }
                    l.drag = None;
                    let hit = hit_test(l, x, y);
                    let pressed = std::mem::replace(&mut l.pressed, Hit::None);
                    if hit == pressed { click(l, hit) } else { Action::None }
                });
                // After the drop: losing capture cancels a drag still in progress.
                ReleaseCapture();
                paint_now(hwnd);
                if let Some(a) = action {
                    perform(hwnd, a);
                }
                return 0;
            }
            WM_CAPTURECHANGED => {
                // Something else took the mouse mid-drag (a dialog, Alt+Tab): put rows back.
                with(cancel_drag);
                return 0;
            }
            WM_RBUTTONUP => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|l| {
                    l.cues = false;
                    l.kbd = Focus::List;
                });
                let action = with(|l| match hit_test(l, x, y) {
                    Hit::Row(i) => {
                        let id = l.rows[i].id;
                        if !l.sel.contains(&id) {
                            select_row(l, i, false, false);
                        }
                        Action::Menu(id)
                    }
                    _ => Action::None,
                });
                // The newly selected note's editor under the menu, not its plain preview.
                sync_editor();
                paint_now(hwnd);
                if let Some(a) = action {
                    perform(hwnd, a);
                }
                return 0;
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as f32;
                let mut p = POINT { x: x_of(lp), y: y_of(lp) };
                ScreenToClient(hwnd, &mut p);
                with(|l| {
                    let lay = layout(l);
                    if (p.x as f32) < lay.list_w {
                        let max = max_scroll(l, &lay);
                        let t = (l.scroll.target - delta / 120.0 * lay.row_h * 1.25).clamp(0.0, max);
                        l.scroll.to(t);
                        l.last_scroll = Instant::now();
                        l.bar_a.to(1.0);
                        l.hover_a.to(0.0);
                        l.hover = Hit::None;
                        anim::wake(Client::List);
                    }
                });
                return 0;
            }
            WM_DPICHANGED => {
                with(|l| {
                    l.s = (wp & 0xFFFF) as f32 / 96.0;
                    l.fonts = Fonts::new(l.s);
                    SendMessageW(l.search, WM_SETFONT, l.fonts.edit_gdi as usize, 1);
                });
                let r = &*(lp as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                return 0;
            }
            // Moved or sized: it opens there again.
            WM_EXITSIZEMOVE => {
                save_rect(hwnd);
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                save_rect(hwnd);
                crate::win::ui::tooltip::leave(hwnd);
                // The detail card's pixels (one to several MB) go with the window.
                CARD_CACHE.with(|c| c.borrow_mut().take());
                LIB.with(|l| {
                    if let Ok(mut l) = l.try_borrow_mut() {
                        l.take();
                    }
                });
                let (brush, _) = EDIT_BRUSH.with(|b| b.replace((null_mut(), u32::MAX)));
                if !brush.is_null() {
                    DeleteObject(brush);
                }
                app::schedule_trim();
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::theme::contrast;

    /// In the note being edited, Ctrl+F goes to the search box (the editor did nothing
    /// with it), as do Ctrl+Tab, and Tab in the title; other keys, AltGr+F and plain Tab in
    /// the text are the editor's.
    #[test]
    fn ctrl_f_searches_from_the_note_being_edited() {
        let f = b'F' as u16;
        assert!(editors_key_is_ours(f, true, false, || false));
        assert!(!editors_key_is_ours(f, true, true, || false), "AltGr+F types a character");
        assert!(!editors_key_is_ours(f, false, false, || true), "F is a letter");
        assert!(!editors_key_is_ours(b'A' as u16, true, false, || false), "Ctrl+A selects the note's text");
        assert!(editors_key_is_ours(VK_TAB, true, false, || false));
        assert!(editors_key_is_ours(VK_TAB, false, false, || true));
        assert!(!editors_key_is_ours(VK_TAB, false, false, || false), "Tab in the text indents");
    }

    /// The preview finds where the editor sets a note's lines: the title at the top, the
    /// body below it, one-line paragraphs a reading line and its space after apart, more
    /// at a larger scale.
    #[test]
    fn the_preview_paces_its_lines_as_the_editor_does() {
        unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide("Msftedit.dll").as_ptr()) };
        crate::win::editor::rich::set_typography(0, 1);
        let (title, first, step) = editor::editor_pace(1.0).expect("measured");
        assert!(title.abs() < 1.0, "{title}");
        assert!(first > 20.0 && first < 60.0, "{first}");
        assert!(step > 18.0 && step < 40.0, "{step}");
        let (_, first2, step2) = editor::editor_pace(2.0).expect("measured");
        assert!((first2 - 2.0 * first).abs() < 3.0 && (step2 - 2.0 * step).abs() < 3.0, "{first2} {step2}");
    }

    /// All Notes opens where it was left, kept wholly on the screen and no larger than it;
    /// a rect smaller than the window can be is not taken.
    #[test]
    fn the_window_opens_where_it_was_left_on_screen() {
        let r = |l, t, rr, b| RECT { left: l, top: t, right: rr, bottom: b };
        let work = r(0, 0, 1920, 1040);
        assert_eq!(on_screen(r(1100, 200, 2060, 820), work, 420).map(corners), Some((960, 200, 1920, 820)));
        assert_eq!(on_screen(r(300, 100, 1260, 720), work, 420).map(corners), Some((300, 100, 1260, 720)));
        assert_eq!(on_screen(r(-500, -50, 2500, 1500), work, 420).map(corners), Some(corners(work)));
        assert_eq!(on_screen(r(10, 10, 110, 110), work, 420).map(corners), None);
    }

    #[test]
    fn quiet_text_still_reads() {
        for dark in [false, true] {
            let p = pal(dark);
            // The search field's hint on the field, secondary text on the list.
            assert!(contrast(p.ter, field_color(dark)) >= 4.5, "hint, dark: {dark}");
            assert!(contrast(p.sub, p.list) >= 4.5, "secondary, dark: {dark}");
        }
    }

    /// 500 notes of about a kilobyte each, with the RTF RichEdit writes for them.
    fn sample_notes(n: usize) -> Vec<crate::common::store::Note> {
        use crate::win::editor::rich;
        let ed = unsafe {
            windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 300, 200, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(ed);
        let notes = (0..n)
            .map(|i| {
                let mut md = format!("Trip plan {i}\n- [ ] passport and tickets\n- [x] book the hotel near the station\n- charger, adapter\n  - the long cable\n");
                for k in 0..8 {
                    md.push_str(&format!("Paragraph {k}: we leave early on Friday and come back on Sunday evening, unless the release checklist slips again.\n"));
                }
                md.push_str("See [the booking](https://example.com/booking) before Thursday.");
                rich::load_markdown(ed, &md);
                let t = crate::common::util::now();
                crate::common::store::Note { id: i as u64 + 1, rtf: rich::get_rtf(ed), text: rich::to_markdown(ed, false), created: t, updated: t - i as i64 * 3600, order: i as i64, ..Default::default() }
            })
            .collect();
        unsafe { DestroyWindow(ed) };
        notes
    }

    /// Rows that take the last list's title and preview come out as rows made anew, at
    /// every keystroke of a search and back, and a note edited since gets its own.
    #[test]
    fn rows_taken_again_are_as_made() {
        crate::common::i18n::init("en");
        let mut notes: Vec<crate::common::store::Note> = (0..60)
            .map(|i| {
                let text = format!("{} plan {i}\n- [ ] book the {} hotel\nrelease notes {}\n", ["Trip", "Work", "Café"][i % 3], ["station", "airport"][i % 2], i * 7);
                crate::common::store::Note { id: i as u64 + 1, text, updated: 1_000 + i as i64, ..Default::default() }
            })
            .collect();
        let row = |r: &Row| (r.id, r.title.clone(), r.body.clone(), r.time.clone(), r.score, r.hits.clone(), r.snippet.clone(), r.snippet_hits.clone());
        let mut last: std::collections::HashMap<u64, Row> = notes.iter().filter_map(|n| row_of(n, &crate::common::search::Query::new(""), false)).map(|r| (r.id, r)).collect();
        for (k, typed) in ["r", "re", "rel", "rele", "rel", "", "c", "caf", "café", "x", ""].into_iter().enumerate() {
            if k == 7 {
                // Edited while searching: its row is made from its new text.
                notes[3].text = "Café edited\nnew body".into();
                notes[3].updated += 1;
            }
            let q = crate::common::search::Query::new(typed);
            let searching = !typed.trim().is_empty();
            let fresh: Vec<_> = notes.iter().filter_map(|n| row_of(n, &q, searching)).map(|r| row(&r)).collect();
            let kept: Vec<Row> = notes.iter().filter_map(|n| row_again(n, &q, searching, last.remove(&n.id))).collect();
            assert_eq!(kept.iter().map(row).collect::<Vec<_>>(), fresh, "typed {typed:?}");
            last = kept.into_iter().map(|r| (r.id, r)).collect();
        }
    }

    /// Timing (run with --ignored --nocapture): what each keystroke of a search costs over
    /// 500 notes of about a kilobyte, mixed words and a few accents: all the rows, and the
    /// scoring alone.
    #[test]
    #[ignore]
    fn probe_search_keystrokes() {
        crate::common::i18n::init("en");
        const WORDS: [&str; 24] = [
            "release", "checklist", "passport", "hotel", "station", "Friday", "café", "résumé", "meeting", "budget", "groceries", "garden", "invoice", "flight", "dentist", "report", "staging", "deploy", "birthday",
            "recipe", "relay", "chapter", "relevant", "check",
        ];
        let mut seed = 7u32;
        let mut word = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            WORDS[(seed >> 16) as usize % WORDS.len()]
        };
        let notes: Vec<crate::common::store::Note> = (0..500)
            .map(|i| {
                let mut text = format!("{} {} {i}\n", word(), word());
                for line in 0..14 {
                    let marker = ["- [ ] ", "- ", "", "1. "][line % 4];
                    let words: Vec<&str> = (0..10).map(|_| word()).collect();
                    text.push_str(&format!("{marker}{}\n", words.join(" ")));
                }
                crate::common::store::Note { id: i as u64 + 1, text, ..Default::default() }
            })
            .collect();
        let typed = "relaese chk";
        let (mut rows, mut again, mut scoring) = (std::time::Duration::ZERO, std::time::Duration::ZERO, std::time::Duration::ZERO);
        let prepared: Vec<(String, String)> = notes.iter().map(|n| (without_objects(title_of(&n.text)), search_rest(&n.text))).collect();
        let mut found = 0;
        // The list as it was before the search: its rows are taken again.
        let mut last: std::collections::HashMap<u64, Row> = notes.iter().filter_map(|n| row_of(n, &crate::common::search::Query::new(""), false)).map(|r| (r.id, r)).collect();
        for k in 1..=typed.len() {
            let q = crate::common::search::Query::new(&typed[..k]);
            let t0 = Instant::now();
            found += std::hint::black_box(notes.iter().filter_map(|n| row_of(n, &q, true)).collect::<Vec<_>>()).len();
            rows += t0.elapsed();
            let t0 = Instant::now();
            let kept: Vec<Row> = notes.iter().filter_map(|n| row_again(n, &q, true, last.remove(&n.id))).collect();
            again += t0.elapsed();
            last = kept.into_iter().map(|r| (r.id, r)).collect();
            let t0 = Instant::now();
            for (title, rest) in &prepared {
                std::hint::black_box(q.score(title, rest));
            }
            scoring += t0.elapsed();
        }
        let n = typed.len() as u32;
        eprintln!("PROBE search over 500 notes, a keystroke: rows made anew {:?}, rows with the last list's titles and previews {:?}, scoring alone {:?} ({found} rows over {n} keystrokes)", rows / n, again / n, scoring / n);
    }

    /// Timing (run with --ignored --nocapture): what one pause in typing costs with 500
    /// notes: the note's RTF made text, notes.json written, and the deck's and All Notes'
    /// texts made again for every note (as each pause once did) or for the edited one.
    #[test]
    #[ignore]
    fn probe_typing_pause() {
        use crate::win::editor::rich;
        crate::common::i18n::init("en");
        let notes = sample_notes(500);
        let ed = unsafe { CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 300, 200, null_mut(), null_mut(), null_mut(), null_mut()) };
        rich::setup(ed);
        let runs = 20;
        let t0 = Instant::now();
        for _ in 0..runs {
            rich::set_rtf(ed, &notes[250].rtf);
            std::hint::black_box(rich::to_markdown(ed, false));
        }
        let to_text = t0.elapsed() / runs;
        unsafe { DestroyWindow(ed) };

        let dir = std::env::temp_dir().join(format!("peekaboo-probe-typing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = crate::common::store::Store::load(dir.clone());
        store.d.notes = notes.clone();
        store.save().unwrap();
        let t0 = Instant::now();
        for k in 0..runs {
            store.d.notes[250].updated += k as i64;
            store.save().unwrap();
        }
        let save = t0.elapsed() / runs;
        // Of which making the JSON, laid out as the file has it and in one line.
        let t0 = Instant::now();
        for _ in 0..runs {
            serde_json::to_writer_pretty(std::io::sink(), &store.d).unwrap();
        }
        let pretty = t0.elapsed() / runs;
        let t0 = Instant::now();
        for _ in 0..runs {
            serde_json::to_writer(std::io::sink(), &store.d).unwrap();
        }
        let compact = t0.elapsed() / runs;
        let size = std::fs::metadata(dir.join("notes.json")).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_dir_all(&dir);

        let q = crate::common::search::Query::new("");
        let t0 = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(notes.iter().filter_map(|n| row_of(n, &q, false)).collect::<Vec<_>>());
        }
        let rows_all = t0.elapsed() / runs;
        let t0 = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(row_of(&notes[250], &q, false));
        }
        let rows_one = t0.elapsed() / runs;
        let deck = |n: &crate::common::store::Note| (without_objects(title_of(&n.text)), without_objects(body_of(&n.text)));
        let t0 = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(notes.iter().map(deck).collect::<Vec<_>>());
        }
        let deck_all = t0.elapsed() / runs;
        let t0 = Instant::now();
        for _ in 0..runs {
            std::hint::black_box(notes.iter().take(8).map(deck).collect::<Vec<_>>());
        }
        let deck_shown = t0.elapsed() / runs;
        eprintln!(
            "PROBE 500 notes, notes.json {} KB: RTF to text {to_text:?}, save {save:?} (JSON {pretty:?}, {compact:?} in one line), All Notes rows: every note {rows_all:?}, the edited one {rows_one:?}; deck texts: every note {deck_all:?}, the 8 shown {deck_shown:?}",
            size / 1024
        );
    }
}
