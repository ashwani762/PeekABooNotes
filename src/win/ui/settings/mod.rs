// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings: a small System Settings-style window.
//!
//! A sidebar of coloured icon tiles picks a page; each page is a column of inset groups
//! whose rows carry switches, segmented controls, swatches, buttons or keycaps. Every
//! control animates from its current value (switch knobs, segment pills, the swatch ring,
//! the sidebar selection) and pages cross-fade with a short rise.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::MARGINS;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme::{self, motion};
use crate::win::util::{WM_MOUSELEAVE, data_dir, mix, pt_in, rect, wide, x_of, y_of};

mod pages;
mod layout;
mod draw;
mod kbd;
use pages::*;
use layout::*;
use draw::*;
use kbd::*;

const CLASS: &str = "PeekABooSettings";
const W: f32 = 720.0;
const H: f32 = 600.0;
const SIDE_W: f32 = 212.0;
const SIDE_TOP: f32 = 66.0;
const SIDE_ROW: f32 = 34.0;
const PAD: f32 = 28.0;
const PAGE_BASE: f32 = 50.0;
const GROUP_TOP: f32 = 74.0;
const ROW_H: f32 = 46.0;
const ROW_SUB_H: f32 = 58.0;
const KEY_ROW_H: f32 = 38.0;
const PREVIEW_H: f32 = 150.0;
const HEADER_H: f32 = 26.0;
const FOOTER_H: f32 = 28.0;
const GROUP_GAP: f32 = 18.0;
const INSET: f32 = 14.0;

/// (title string key, tile glyph, tile colour)
const PAGES: [(&str, &str, u32); 5] = [
    ("settings_page_general", "\u{E713}", 0x8E8E93),
    ("settings_page_notes", "\u{E70B}", 0xF5B300),
    ("settings_page_deck", "\u{E90D}", 0xFF9F0A),
    ("settings_page_shortcuts", "\u{E765}", 0x5E5CE6),
    ("settings_page_data", "\u{E8B7}", 0x0A84FF),
];

const CARDS: [u8; 3] = [5, 8, 12];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Key {
    Startup,
    Theme,
    Language,
    NoteFont,
    NoteSize,
    NewColor,
    ReduceMotion,
    HideUnpinned,
    ShowDeck,
    Edge,
    Cards,
    NewAt,
    Fullscreen,
    Folder,
    Import,
    Export,
    DeleteAll,
    Mcp,
    McpSetup,
}

enum Ctl {
    Toggle,
    Seg(Vec<&'static str>),
    Swatches,
    Button(&'static str),
    /// A pop-up button showing the current choice; clicking lists the options.
    Choice,
    /// Keycaps; lowercase names ("ctrl") are shown in this language's key names.
    Keys(&'static [&'static str]),
    /// A sample note in the chosen type and colour.
    Preview,
}

struct Row {
    key: Option<Key>,
    label: &'static str,
    sub: Option<String>,
    ctl: Ctl,
}

struct Group {
    header: Option<&'static str>,
    rows: Vec<Row>,
    footer: Option<String>,
}

fn row(key: Key, label: &'static str, ctl: Ctl) -> Row {
    Row { key: Some(key), label, sub: None, ctl }
}

fn row_sub(key: Key, label: &'static str, sub: &str, ctl: Ctl) -> Row {
    Row { key: Some(key), label, sub: Some(sub.into()), ctl }
}

fn keys(label: &'static str, k: &'static [&'static str]) -> Row {
    Row { key: None, label, sub: None, ctl: Ctl::Keys(k) }
}

struct Pal {
    bg: u32,
    side: u32,
    group: u32,
    line: u32,
    text: u32,
    sub: u32,
    accent: u32,
    track: u32,
    field: (u32, f32),
    pill: u32,
    hover: (u32, f32),
    /// Destructive buttons, and their text at rest (deeper red in light mode, for contrast).
    danger: (u32, u32),
}

fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            bg: 0x1C1C1E,
            side: 0x262628,
            group: 0x2C2C2E,
            line: 0x3A3A3C,
            text: 0xF5F5F7,
            sub: 0x98989D,
            accent: 0x0A84FF,
            track: 0x48484A,
            field: (0x767680, 0.24),
            pill: 0x636366,
            hover: (0xFFFFFF, 0.06),
            danger: (0xFF453A, 0xFF453A),
        }
    } else {
        Pal {
            bg: 0xF5F5F7,
            side: 0xEBEBEE,
            group: 0xFFFFFF,
            line: 0xE5E5EA,
            text: 0x1D1D1F,
            sub: 0x6E6E73,
            accent: 0x007AFF,
            track: 0xE3E3E8,
            field: (0x767680, 0.12),
            pill: 0xFFFFFF,
            hover: (0x000000, 0.045),
            danger: (0xFF3B30, 0xD70015),
        }
    }
}

struct Fonts {
    title: Font,
    label: Font,
    sub: Font,
    header: Font,
    seg: Font,
    seg_on: Font,
    button: Font,
    key: Font,
    side: Font,
    side_title: Font,
    tile: Font,
    chrome: Font,
    result_glyph: Font,
    about: Font,
    about_sub: Font,
}

impl Fonts {
    fn new(s: f32) -> Fonts {
        let f = |px: f32, w: u32, face: &str| Font::new(px * s, w, face);
        let icons = crate::win::ui::note::icon_face();
        Fonts {
            title: f(22.0, FW_SEMIBOLD, theme::FACE_DISPLAY),
            label: f(13.5, FW_NORMAL, theme::FACE_TEXT),
            sub: f(12.0, FW_NORMAL, theme::FACE_TEXT),
            header: f(12.5, FW_SEMIBOLD, theme::FACE_TEXT),
            seg: f(12.5, FW_NORMAL, theme::FACE_TEXT),
            seg_on: f(12.5, FW_SEMIBOLD, theme::FACE_TEXT),
            button: f(12.5, FW_SEMIBOLD, theme::FACE_TEXT),
            key: f(11.5, FW_SEMIBOLD, theme::FACE_SMALL),
            side: f(13.5, FW_NORMAL, theme::FACE_TEXT),
            side_title: f(17.0, FW_SEMIBOLD, theme::FACE_DISPLAY),
            tile: f(13.0, FW_NORMAL, icons),
            chrome: f(10.0, FW_NORMAL, icons),
            result_glyph: f(12.0, FW_BOLD, icons),
            about: f(13.0, FW_SEMIBOLD, theme::FACE_TEXT),
            about_sub: f(11.5, FW_NORMAL, theme::FACE_TEXT),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Close,
    Page(usize),
    /// A control, with the segment / swatch index it was hit at (0 otherwise).
    Ctl(Key, usize),
}

/// Where the keyboard is: the page list, or the control of one setting.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Focus {
    Pages,
    Ctl(Key),
}

struct Win {
    hwnd: HWND,
    s: f32,
    fonts: Fonts,
    canvas: Option<Canvas>,
    page: usize,
    hover: Hit,
    pressed: Hit,
    /// The control with the keys, and the swatch it's on when that's the swatches.
    kbd: Focus,
    swatch_k: usize,
    /// The focus ring shows: the keyboard moved focus since the last click.
    cues: bool,
    tracking: bool,
    startup: bool,
    side_y: Spring,
    hover_y: Spring,
    hover_a: Spring,
    page_a: Spring,
    press: Spring,
    /// Each control's displayed value (switch position, segment or swatch index).
    knobs: HashMap<Key, Spring>,
    /// The last import / export result shown on its button: (button, success, text).
    result: Option<(Key, bool, String)>,
    result_a: Spring,
    /// How far the page is scrolled down (a long page, like Shortcuts).
    scroll: Spring,
}

/// How long a button shows its result before turning back.
const T_RESULT: usize = 7;
const RESULT_MS: u32 = 2600;

thread_local! {
    /// A result whose time ran out while another window was in front.
    static RESULT_WAITS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Win) -> R) -> Option<R> {
    WIN.with(|w| w.try_borrow_mut().ok().and_then(|mut w| w.as_mut().map(f)))
}

pub fn hwnd() -> HWND {
    with(|w| w.hwnd).unwrap_or(null_mut())
}

// ---------------------------------------------------------------- values

fn value(w: &Win, k: Key) -> usize {
    if k == Key::Startup {
        return w.startup as usize;
    }
    app::store(|s| {
        let st = &s.d.settings;
        match k {
            Key::Theme => (st.theme as usize).min(2),
            Key::NoteFont => (st.note_font as usize).min(3),
            Key::NoteSize => (st.note_size as usize).min(2),
            Key::NewColor => st.new_color.and_then(|c| theme::ORDER.iter().position(|&o| o == c)).map_or(0, |p| p + 1),
            Key::ReduceMotion => st.reduce_motion as usize,
            Key::HideUnpinned => !st.keep_open as usize,
            Key::Mcp => st.mcp as usize,
            Key::ShowDeck => !st.deck_hidden as usize,
            Key::Edge => !st.edge_left as usize,
            Key::Cards => CARDS.iter().position(|&c| c == st.deck_max).unwrap_or(1),
            Key::NewAt => !st.new_on_top as usize,
            Key::Fullscreen => st.over_fullscreen as usize,
            _ => 0,
        }
    })
}

/// The language button's text: the chosen language's own name, or "Same as Windows".
fn language_label() -> String {
    let tag = app::store(|s| s.d.settings.language.clone());
    if tag.is_empty() {
        return t!("language_follow_windows").to_string();
    }
    crate::common::i18n::languages().into_iter().find(|l| l.tag.eq_ignore_ascii_case(&tag)).map(|l| l.name).unwrap_or(tag)
}

/// Pop up the languages under the pointer; choosing one switches the whole interface.
fn choose_language() {
    let current = app::store(|s| s.d.settings.language.clone());
    let langs = crate::common::i18n::languages();
    let mut items = vec![crate::win::ui::menu::check(1, t!("language_follow_windows"), "", current.is_empty()), crate::win::ui::menu::Item::Separator];
    for (i, l) in langs.iter().enumerate() {
        items.push(crate::win::ui::menu::check(100 + i as u32, &l.name, "", l.tag.eq_ignore_ascii_case(&current)));
    }
    // Drop the list from under the button, right-aligned to it, so it never covers it.
    let anchor = with(|w| {
        let groups = page(w.page);
        let sh = place(w, &groups).into_iter().find_map(|it| match it {
            Item::Row(pl) if matches!(pl.row.ctl, Ctl::Choice) => Some(choice_shape(w, &pl.r)),
            _ => None,
        })?;
        let mut p = POINT { x: (sh.x + sh.w) as i32, y: (sh.y + sh.h + 4.0 * w.s) as i32 };
        unsafe { ClientToScreen(w.hwnd, &mut p) };
        Some(p)
    })
    .flatten();
    let (p, right) = match anchor {
        Some(p) => (p, true),
        None => {
            let mut p = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut p) };
            (p, false)
        }
    };
    let tag = match crate::win::ui::menu::show_with(items, p, right, None) {
        0 => return,
        1 => String::new(),
        n => match langs.get((n - 100) as usize) {
            Some(l) => l.tag.clone(),
            None => return,
        },
    };
    app::update_settings(|s| s.language = tag);
    app::apply_language();
}

/// Turn button `k` into a tick and a count (or a cross) for a moment. Cancelling the
/// dialog shows nothing.
fn show_result(hwnd: HWND, k: Key, outcome: app::Outcome, plural: &str) {
    let Some(r) = outcome else { return };
    let text = match r {
        Ok(n) => crate::common::i18n::plural(plural, n as i64, &[&n]),
        Err(()) => t!("result_failed").to_string(),
    };
    show_text(hwnd, k, r.is_ok(), text);
}

/// Answer in row `k`'s button for a moment: a tick (or a cross) and `text`.
fn show_text(hwnd: HWND, k: Key, ok: bool, text: String) {
    RESULT_WAITS.with(|r| r.set(false));
    with(|w| {
        w.result = Some((k, ok, text));
        w.result_a.to(1.0);
    });
    anim::wake(Client::Settings);
    unsafe { SetTimer(hwnd, T_RESULT, RESULT_MS, None) };
}

/// Carry out a control change. Runs outside the window's borrow: it calls back into
/// the app, which may retheme or refresh this window.
fn apply(hwnd: HWND, k: Key, v: usize) {
    match k {
        Key::Language => choose_language(),
        Key::Startup => app::set_startup(v == 1),
        Key::Theme => {
            app::update_settings(|s| s.theme = v as u8);
            app::apply_theme();
        }
        Key::NoteFont => {
            app::update_settings(|s| s.note_font = v as u8);
            app::apply_typography();
        }
        Key::NoteSize => {
            app::update_settings(|s| s.note_size = v as u8);
            app::apply_typography();
        }
        Key::NewColor => app::update_settings(|s| s.new_color = v.checked_sub(1).map(|i| theme::ORDER[i])),
        Key::ReduceMotion => {
            app::update_settings(|s| s.reduce_motion = v == 1);
            app::apply_motion();
        }
        Key::HideUnpinned => app::update_settings(|s| s.keep_open = v == 0),
        Key::ShowDeck => app::update_settings(|s| s.deck_hidden = v == 0),
        Key::Edge => app::update_settings(|s| s.edge_left = v == 0),
        Key::Cards => app::update_settings(|s| s.deck_max = CARDS[v.min(2)]),
        Key::NewAt => app::update_settings(|s| s.new_on_top = v == 0),
        Key::Fullscreen => app::update_settings(|s| s.over_fullscreen = v == 1),
        Key::Folder => app::open_notes_folder(),
        Key::Import => show_result(hwnd, k, app::import(hwnd), "result_imported"),
        Key::Export => show_result(hwnd, k, app::export_all(hwnd), "result_exported"),
        Key::DeleteAll => app::delete_all(hwnd),
        Key::Mcp => {
            app::update_settings(|s| s.mcp = v == 1);
            app::set_mcp_enabled(v == 1);
        }
        Key::McpSetup => {
            // Which app? Each wants its own form of the same setup.
            let items = app::MCP_CLIENTS.iter().enumerate().map(|(i, name)| crate::win::ui::menu::action(i as u32 + 1, name, "", "")).collect();
            let mut p = POINT { x: 0, y: 0 };
            match with(|w| menu_anchor(w, k)).flatten() {
                Some(at) => p = at,
                None => unsafe {
                    GetCursorPos(&mut p);
                },
            }
            let choice = app::popup(items, p.x, p.y, hwnd);
            if choice > 0 {
                let i = choice as usize - 1;
                let name = app::MCP_CLIENTS[i];
                let (ok, text) = match app::mcp_connect(i) {
                    app::McpConnect::Done => (true, t!("result_connected", name)),
                    app::McpConnect::Copied => (true, crate::common::i18n::plural("result_copied", 1, &[&1])),
                    app::McpConnect::Missing => (true, t!("result_not_installed").to_string()),
                    app::McpConnect::Failed => (false, t!("result_failed").to_string()),
                };
                show_text(hwnd, k, ok, text);
            }
        }
    }
}

/// Set `k` to `v`, from a click or a key: the control moves first so it answers at once,
/// then the work is done.
fn change(hwnd: HWND, k: Key, v: usize) {
    with(|w| {
        if let Some(sp) = w.knobs.get_mut(&k) {
            sp.to(v as f32);
        }
    });
    apply(hwnd, k, v);
    refresh();
}

/// Re-read every value (a setting may have changed from the tray or a hotkey).
pub fn refresh() {
    let h = with(|w| {
        w.startup = app::startup_enabled();
        sync_knobs(w);
        w.hwnd
    });
    if h.is_some() {
        anim::wake(Client::Settings);
    }
}

fn sync_knobs(w: &mut Win) {
    let keys: Vec<Key> = page(w.page).iter().flat_map(|g| g.rows.iter().filter_map(|r| r.key)).collect();
    for k in keys {
        let v = value(w, k) as f32;
        w.knobs.entry(k).or_insert_with(|| Spring::unit(v, (0.86, 0.3))).to(v);
    }
}

pub fn retheme() {
    if let Some(h) = with(|w| w.hwnd) {
        theme::set_dark_titlebar(h, app::dark());
        paint_now(h);
    }
}

/// The interface language changed: new title, then repaint (all other text is drawn
/// from the string tables on every paint).
pub fn relabel() {
    if let Some(h) = with(|w| w.hwnd) {
        unsafe { SetWindowTextW(h, wide(&t!("window_settings", t!("app_name"))).as_ptr()) };
        paint_now(h);
    }
}

// ---------------------------------------------------------------- window

pub fn show() {
    if let Some(h) = with(|w| w.hwnd) {
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
        return;
    }
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let (work, s) = crate::win::ui::deck::monitor_geo();
    let (w, h) = ((W * s) as i32, (H * s) as i32);
    let x = work.left + ((work.right - work.left) - w) / 2;
    let y = work.top + ((work.bottom - work.top) - h) / 2;
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            wide(CLASS).as_ptr(),
            wide(&t!("window_settings", t!("app_name"))).as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
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

fn on_create(hwnd: HWND) {
    let s = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
    unsafe {
        let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        DwmExtendFrameIntoClientArea(hwnd, &m);
    }
    theme::set_dark_titlebar(hwnd, app::dark());
    theme::set_round_corners(hwnd, false);
    let mut w = Win {
        hwnd,
        s,
        fonts: Fonts::new(s),
        canvas: None,
        page: 0,
        hover: Hit::None,
        pressed: Hit::None,
        kbd: Focus::Pages,
        swatch_k: 0,
        cues: false,
        tracking: false,
        startup: app::startup_enabled(),
        side_y: Spring::px(0.0, motion::MOMENTUM),
        hover_y: Spring::px(0.0, (1.0, 0.16)),
        hover_a: Spring::unit(0.0, motion::SNAPPY),
        page_a: Spring::unit(1.0, motion::DEFAULT),
        press: Spring::unit(0.0, motion::SNAPPY),
        result: None,
        result_a: Spring::unit(0.0, motion::DEFAULT),
        knobs: HashMap::new(),
        scroll: Spring::px(0.0, motion::SNAPPY),
    };
    sync_knobs(&mut w);
    WIN.with(|c| *c.borrow_mut() = Some(w));
}

fn set_page(w: &mut Win, i: usize) {
    let i = i.min(PAGES.len() - 1);
    if i == w.page {
        return;
    }
    w.page = i;
    w.scroll.snap(0.0);
    w.side_y.to(i as f32 * SIDE_ROW * w.s);
    // Controls on the new page appear at their values; only the page itself moves.
    w.knobs.clear();
    sync_knobs(w);
    w.page_a.snap(0.0);
    w.page_a.to(1.0);
    anim::wake(Client::Settings);
}

pub fn frame(dt: f32) -> bool {
    let r = with(|w| {
        let mut moving = false;
        for sp in [&mut w.side_y, &mut w.hover_y, &mut w.hover_a, &mut w.page_a, &mut w.press, &mut w.result_a, &mut w.scroll] {
            moving |= sp.step(dt);
        }
        for sp in w.knobs.values_mut() {
            moving |= sp.step(dt);
        }
        (moving, w.hwnd)
    });
    match r {
        Some((moving, h)) => {
            paint_now(h);
            moving
        }
        None => false,
    }
}

// ---------------------------------------------------------------- layout

/// One laid-out row and its rectangle.
struct Placed<'a> {
    row: &'a Row,
    r: RECT,
    first: bool,
}

enum Item<'a> {
    Header(&'static str, i32),
    Footer(&'a str, i32),
    Box(Shape),
    Row(Placed<'a>),
}

const TOGGLE: (f32, f32) = (40.0, 24.0);
const SWATCH: f32 = 16.0;
const SWATCH_GAP: f32 = 6.0;

// ---------------------------------------------------------------- painting

// ---------------------------------------------------------------- input

fn set_hover(w: &mut Win, hit: Hit) {
    if w.hover == hit {
        return;
    }
    w.hover = hit;
    match hit {
        Hit::Page(i) if i != w.page => {
            let y = i as f32 * SIDE_ROW * w.s;
            if w.hover_a.value < 0.05 {
                w.hover_y.snap(y);
            } else {
                w.hover_y.to(y);
            }
            w.hover_a.to(1.0);
        }
        _ => w.hover_a.to(0.0),
    }
    anim::wake(Client::Settings);
}

/// What a click on `hit` changes: (key, new value).
fn click(w: &mut Win, hit: Hit) -> Option<(Key, usize)> {
    match hit {
        Hit::Page(i) => {
            set_page(w, i);
            set_hover(w, Hit::None);
            None
        }
        Hit::Ctl(k, sub) => {
            let row_is_toggle = page(w.page).iter().flat_map(|g| g.rows.iter()).any(|r| r.key == Some(k) && matches!(r.ctl, Ctl::Toggle));
            let v = if row_is_toggle { 1 - value(w, k).min(1) } else { sub };
            Some((k, v))
        }
        _ => None,
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                on_create(hwnd);
                return 0;
            }
            WM_NCCALCSIZE if wp != 0 => return 0,
            WM_NCACTIVATE => return DefWindowProcW(hwnd, msg, wp, -1),
            WM_NCHITTEST => {
                let mut p = POINT { x: x_of(lp), y: y_of(lp) };
                ScreenToClient(hwnd, &mut p);
                let r = with(|w| match hit_test(w, p.x, p.y) {
                    Hit::None if (p.y as f32) < 44.0 * w.s => HTCAPTION,
                    Hit::None if (p.x as f32) >= SIDE_W * w.s && (p.y as f32) < header_band(w) => HTCAPTION,
                    _ => HTCLIENT,
                });
                return r.unwrap_or(HTCLIENT) as LRESULT;
            }
            WM_GETMINMAXINFO => {
                // A fixed-size panel: the frame is there only for the shadow and corners.
                let s = with(|w| w.s).unwrap_or(GetDpiForWindow(hwnd).max(96) as f32 / 96.0);
                let mm = &mut *(lp as *mut MINMAXINFO);
                let size = POINT { x: (W * s) as i32, y: (H * s) as i32 };
                mm.ptMinTrackSize = size;
                mm.ptMaxTrackSize = size;
                return 0;
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                BeginPaint(hwnd, &mut ps);
                EndPaint(hwnd, &ps);
                paint_now(hwnd);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_SIZE => {
                paint_now(hwnd);
                return 0;
            }
            WM_MOUSEWHEEL => {
                let notches = ((wp >> 16) as i16) as f32 / 120.0;
                with(|w| {
                    let max = max_scroll(w);
                    if max > 0.0 {
                        let to = (w.scroll.target - notches * 56.0 * w.s).clamp(0.0, max);
                        w.scroll.to(to);
                        anim::wake(Client::Settings);
                    }
                });
                return 0;
            }
            WM_MOUSEMOVE => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|w| {
                    if !w.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        w.tracking = true;
                    }
                    let hit = hit_test(w, x, y);
                    set_hover(w, hit);
                });
                return 0;
            }
            WM_MOUSELEAVE => {
                with(|w| {
                    w.tracking = false;
                    set_hover(w, Hit::None);
                });
                return 0;
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                let (x, y) = (x_of(lp), y_of(lp));
                let page_hit = with(|w| {
                    let hit = hit_test(w, x, y);
                    w.pressed = hit;
                    // A click hides the focus ring and gives the keys to what it hit.
                    w.cues = false;
                    match hit {
                        Hit::Page(_) => w.kbd = Focus::Pages,
                        Hit::Ctl(k, i) => {
                            w.kbd = Focus::Ctl(k);
                            w.swatch_k = i;
                        }
                        _ => {}
                    }
                    w.press.snap(0.0);
                    w.press.to(1.0);
                    anim::wake(Client::Settings);
                    // Pages switch on press: there's nothing to cancel.
                    if let Hit::Page(_) = hit {
                        click(w, hit);
                        true
                    } else {
                        false
                    }
                });
                if page_hit == Some(true) {
                    with(|w| w.pressed = Hit::None);
                }
                return 0;
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let (x, y) = (x_of(lp), y_of(lp));
                let act = with(|w| {
                    let hit = hit_test(w, x, y);
                    let was = std::mem::replace(&mut w.pressed, Hit::None);
                    w.press.to(0.0);
                    anim::wake(Client::Settings);
                    if hit != was {
                        return (None, false);
                    }
                    if hit == Hit::Close {
                        return (None, true);
                    }
                    (click(w, hit), false)
                });
                match act {
                    Some((_, true)) => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    Some((Some((k, v)), _)) => change(hwnd, k, v),
                    _ => {}
                }
                paint_now(hwnd);
                return 0;
            }
            WM_KEYDOWN => {
                let vk = wp as u16;
                match vk {
                    VK_ESCAPE => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    VK_TAB => {
                        with(|w| move_focus(w, crate::win::util::key_down(VK_SHIFT), true));
                    }
                    // In the page list they turn the page; on the page they go row to row.
                    VK_UP | VK_DOWN => {
                        with(|w| {
                            let back = vk == VK_UP;
                            if w.kbd == Focus::Pages {
                                let n = PAGES.len();
                                set_page(w, if back { (w.page + n - 1) % n } else { (w.page + 1) % n });
                            } else {
                                move_focus(w, back, false);
                            }
                        });
                    }
                    VK_LEFT | VK_RIGHT => {
                        if let Some(Some((k, v))) = with(|w| nudge(w, if vk == VK_LEFT { -1 } else { 1 })) {
                            change(hwnd, k, v);
                        }
                    }
                    VK_SPACE | VK_RETURN => {
                        if let Some(Some((k, v))) = with(activate) {
                            change(hwnd, k, v);
                            paint_now(hwnd);
                        }
                    }
                    _ if vk == b'W' as u16 && crate::win::util::key_down(VK_CONTROL) => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                    _ => {}
                }
                return 0;
            }
            // The ring shows only while this window has the keys.
            WM_SETFOCUS | WM_KILLFOCUS => {
                paint_now(hwnd);
                return 0;
            }
            WM_SETTINGCHANGE => {
                with(|w| w.startup = app::startup_enabled());
            }
            WM_DPICHANGED => {
                with(|w| {
                    w.s = (wp & 0xFFFF) as f32 / 96.0;
                    w.fonts = Fonts::new(w.s);
                    w.side_y.snap(w.page as f32 * SIDE_ROW * w.s);
                });
                let r = &*(lp as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                return 0;
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_TIMER if wp == T_RESULT => {
                KillTimer(hwnd, T_RESULT);
                // Export shows its folder on top: the ✓ waits (looking twice a second)
                // until Settings is back in front, then gets its full time there.
                let front = GetForegroundWindow() == hwnd;
                if !front {
                    RESULT_WAITS.with(|r| r.set(true));
                    SetTimer(hwnd, T_RESULT, 500, None);
                } else if RESULT_WAITS.with(|r| r.replace(false)) {
                    SetTimer(hwnd, T_RESULT, RESULT_MS, None);
                } else {
                    with(|w| w.result_a.to(0.0));
                    anim::wake(Client::Settings);
                }
                return 0;
            }
            WM_DESTROY => {
                WIN.with(|w| {
                    if let Ok(mut w) = w.try_borrow_mut() {
                        w.take();
                    }
                });
                draw::forget_preview();
                app::schedule_trim();
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}
