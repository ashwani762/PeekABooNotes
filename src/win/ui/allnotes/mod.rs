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

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::common::store::{body_of, strip_marker, title_of};
use crate::win::theme::{self, motion};
use crate::common::i18n::time::{edited_time, relative_time};
use crate::win::util::{WM_MOUSELEAVE, cref, from_wide, key_down, mix, rect, wide, x_of, y_of};

mod editor;
mod list;
mod layout;
mod draw;
use editor::*;
use list::*;
use layout::*;
use draw::*;

const CLASS: &str = "PeekABooLibrary";
const ID_SEARCH: usize = 10;
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
    Row(usize),
    Archive,
    Delete,
    Swatch(usize),
    Card,
    /// Bring the selected note back from its own window to edit it here.
    OpenHere,
}

struct Row {
    id: u64,
    title: String,
    off_deck: bool,
    order: i64,
    body: String,
    time: String,
    created: i64,
    updated: i64,
    color: u8,
    archived: bool,
    /// While searching: how well it matched, the matched parts of the title (char
    /// ranges), and a piece of the body around a match the title doesn't show.
    score: i32,
    hits: Vec<(usize, usize)>,
    snippet: Option<String>,
}

struct Pal {
    window: u32,
    list: u32,
    line: u32,
    text: u32,
    sub: u32,
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
            ter: 0x6C6C70,
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
            ter: 0xA1A1A6,
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
    seg_on: Font,
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
            seg_on: f(13.0, FW_SEMIBOLD, theme::FACE_TEXT),
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
    sel: Vec<u64>, // the last entry is the primary selection
    anchor: Option<u64>,
    focus_search: bool,
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
    edit_brush: HBRUSH,
    edit_brush_rgb: u32,
    /// Inline editor hosted in the detail card (a child note window) and its note.
    editor: HWND,
    editor_id: u64,
    /// A row being dragged to a new place (Notes view only).
    drag: Option<ListDrag>,
    /// Rows' visual offsets while they make room for a dragged row, by note id.
    offs: std::collections::HashMap<u64, Spring>,
}

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
}

fn with<R>(f: impl FnOnce(&mut Lib) -> R) -> Option<R> {
    LIB.with(|l| l.try_borrow_mut().ok().and_then(|mut l| l.as_mut().map(f)))
}

/// The inline editor, when it's showing note `id`.
pub fn editor_for(id: u64) -> Option<HWND> {
    with(|l| (l.editor_id == id && !l.editor.is_null()).then_some(l.editor)).flatten()
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
    let (work, s) = crate::win::ui::deck::monitor_geo();
    let (w, h) = ((960.0 * s) as i32, (620.0 * s) as i32);
    let x = work.left + ((work.right - work.left) - w) / 2;
    let y = work.top + ((work.bottom - work.top) - h) / 2;
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            wide(CLASS).as_ptr(),
            wide(&t!("window_all_notes", t!("app_name"))).as_ptr(),
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

pub fn refresh() {
    let h = with(|l| {
        rebuild(l);
        l.hwnd
    });
    if let Some(h) = h {
        paint_now(h);
        sync_editor();
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
        unsafe { SetFocus(e) };
    }
}

/// Note typography changed: restyle the inline editor, if there is one.
pub fn retheme_editor() {
    with(|l| l.fonts = Fonts::new(l.s));
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
        SetWindowTextW(h, wide(&t!("window_all_notes", t!("app_name"))).as_ptr());
        SendMessageW(search, EM_SETCUEBANNER, 1, wide(t!("search_placeholder")).as_ptr() as isize);
    }
    refresh();
}

pub fn retheme() {
    let Some((h, search)) = with(|l| (l.hwnd, l.search)) else { return };
    theme::set_dark_titlebar(h, app::dark());
    // The inline editor's paper, the card's cached pixels and the search field (its own
    // control, which only repaints when told) all follow the new theme.
    retheme_editor();
    unsafe {
        InvalidateRect(search, std::ptr::null(), 1);
        InvalidateRect(h, std::ptr::null(), 0);
    }
    paint_now(h);
}

/// Note `id` changed colour: if the inline editor shows it, its paper follows.
pub fn recolor(id: u64) {
    if editor_for(id).is_some() {
        retheme_editor();
    }
}

pub fn frame(dt: f32) -> bool {
    let r = with(|l| {
        if l.last_scroll.elapsed().as_secs_f32() > 0.9 {
            l.bar_a.to(0.0);
        }
        let mut moving = false;
        for sp in [&mut l.seg_x, &mut l.scroll, &mut l.sel_y, &mut l.hover_y, &mut l.hover_a, &mut l.list_a, &mut l.detail_a, &mut l.bar_a] {
            moving |= sp.step(dt);
        }
        for sp in l.offs.values_mut() {
            moving |= sp.step(dt);
        }
        (moving || l.bar_a.value > 0.01, l.hwnd)
    });
    match r {
        Some((moving, h)) => {
            paint_now(h);
            moving
        }
        None => false,
    }
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
    Reorder(Vec<u64>),
    Close,
    Min,
    MaxToggle,
}

fn perform(hwnd: HWND, a: Action) {
    if matches!(a, Action::Open(..) | Action::Archive(..) | Action::Delete(_) | Action::Color(..) | Action::Share(_)) {
        close_editor();
    }
    perform_inner(hwnd, a);
    sync_editor();
}

fn perform_inner(hwnd: HWND, a: Action) {
    match a {
        Action::None => {}
        Action::Edit => {
            if let Some(e) = with(|l| l.editor)
                && !e.is_null() {
                    unsafe { SetFocus(e) };
                }
        }
        Action::New => new_here(),
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
        Action::Open(id, from) => {
            if from.is_some() {
                app::open_note_from(id, from)
            } else {
                app::open_note(id)
            }
        }
        Action::Archive(ids, archived) => app::set_archived(&ids, archived),
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

/// Keys for the search box and list that must be seen before the EDIT consumes them.
pub fn pre_translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let Some((hwnd, search)) = with(|l| (l.hwnd, l.search)) else { return false };
    if msg.hwnd != hwnd && msg.hwnd != search {
        return false;
    }
    let vk = msg.wParam as u16;
    let ctrl = key_down(VK_CONTROL);
    let action = with(|l| -> Option<Action> {
        match vk {
            VK_DOWN => {
                move_selection(l, 1);
                Some(Action::None)
            }
            VK_UP => {
                move_selection(l, -1);
                Some(Action::None)
            }
            VK_RETURN => Some(if l.editor.is_null() { open_selected(l) } else { Action::Edit }),
            VK_ESCAPE => {
                if !l.query.is_empty() {
                    // Not here: the EDIT's change notification re-enters this window
                    // and must find it unborrowed, or the cleared query is lost.
                    Some(Action::ClearSearch)
                } else {
                    Some(Action::Close)
                }
            }
            VK_DELETE if msg.hwnd == hwnd => Some(Action::Delete(l.sel.clone())),
            VK_TAB => {
                unsafe { SetFocus(if msg.hwnd == search { hwnd } else { search }) };
                Some(Action::None)
            }
            _ if ctrl && vk == b'F' as u16 => {
                unsafe {
                    SetFocus(l.search);
                    SendMessageW(l.search, EM_SETSEL, 0, -1);
                }
                Some(Action::None)
            }
            _ if ctrl && vk == b'A' as u16 && msg.hwnd == hwnd => {
                let first = l.rows.first().map(|r| r.id);
                l.sel = l.rows.iter().map(|r| r.id).filter(|x| Some(*x) != first).collect();
                if let Some(id) = first {
                    l.sel.push(id);
                }
                l.detail_a.snap(0.0);
                l.detail_a.to(1.0);
                anim::wake(Client::List);
                Some(Action::None)
            }
            _ if ctrl && vk == b'N' as u16 => Some(Action::New),
            _ => None,
        }
    })
    .flatten();
    match action {
        Some(a) => {
            perform(hwnd, a);
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
        sel: Vec::new(),
        anchor: None,
        focus_search: false,
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
        list_a: Spring::unit(0.0, motion::DEFAULT),
        detail_a: Spring::unit(0.0, motion::DEFAULT),
        bar_a: Spring::unit(0.0, motion::DEFAULT),
        last_scroll: now,
        edit_brush: null_mut(),
        edit_brush_rgb: u32::MAX,
        editor: null_mut(),
        editor_id: 0,
        drag: None,
        offs: std::collections::HashMap::new(),
    };
    lib.list_a.to(1.0);
    LIB.with(|l| *l.borrow_mut() = Some(lib));
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
            WM_SIZE => {
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
                let brush = with(|l| {
                    if l.edit_brush_rgb != rgb {
                        if !l.edit_brush.is_null() {
                            DeleteObject(l.edit_brush);
                        }
                        l.edit_brush = CreateSolidBrush(cref(rgb));
                        l.edit_brush_rgb = rgb;
                    }
                    l.edit_brush
                });
                SetTextColor(wp as HDC, cref(pal(dark).text));
                SetBkColor(wp as HDC, cref(rgb));
                return brush.unwrap_or(null_mut()) as LRESULT;
            }
            WM_COMMAND => {
                let (id, code) = (wp & 0xFFFF, ((wp >> 16) & 0xFFFF) as u32);
                if id == ID_SEARCH {
                    match code {
                        EN_CHANGE => {
                            let mut buf = [0u16; 256];
                            GetWindowTextW(lp as HWND, buf.as_mut_ptr(), 256);
                            let q = from_wide(&buf);
                            with(|l| {
                                l.query = q;
                                l.scroll.snap(0.0);
                                l.list_a.snap(0.55);
                                l.list_a.to(1.0);
                                // Typing selects the best match; clearing the search keeps
                                // the note you found, scrolled into view.
                                let cleared = l.query.is_empty();
                                if !cleared {
                                    l.sel.clear();
                                }
                                rebuild(l);
                                if let Some(i) = primary_index(l).filter(|_| cleared) {
                                    let lay = layout(l);
                                    l.sel_y.snap(lay.list_top + i as f32 * lay.row_h);
                                    ensure_visible(l, i);
                                }
                            });
                        }
                        EN_SETFOCUS | EN_KILLFOCUS => {
                            with(|l| l.focus_search = code == EN_SETFOCUS);
                        }
                        _ => {}
                    }
                    paint_now(hwnd);
                    sync_editor();
                }
                return 0;
            }
            WM_SETFOCUS => {
                if let Some((search, empty)) = with(|l| (l.search, l.rows.is_empty()))
                    && empty {
                        SetFocus(search);
                    }
                return 0;
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
                    match hit {
                        Hit::Search => {
                            SetFocus(l.search);
                            Action::None
                        }
                        Hit::Segment(i) => {
                            set_mode(l, i); // responds on press
                            Action::None
                        }
                        Hit::Row(i) => {
                            // Selection responds on press; a quick second press opens.
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
                                if l.editor.is_null() { open_selected(l) } else { Action::Edit }
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
            WM_CLOSE => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                crate::win::ui::tooltip::leave(hwnd);
                // The detail card's pixels (one to several MB) go with the window.
                CARD_CACHE.with(|c| c.borrow_mut().take());
                LIB.with(|l| {
                    if let Ok(mut l) = l.try_borrow_mut()
                        && let Some(lib) = l.take()
                            && !lib.edit_brush.is_null() {
                                DeleteObject(lib.edit_brush);
                            }
                });
                app::schedule_trim();
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
