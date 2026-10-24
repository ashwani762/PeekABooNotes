// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! A floating note window: a coloured, borderless frame hosting a RichEdit control.

use std::cell::OnceCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::{MARGINS, NMHDR};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::Spring;
use crate::win::app::{self, *};
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::editor::rich::{self, CFM_BOLD, CFM_ITALIC, CFM_STRIKEOUT, CFM_UNDERLINE, PFN_ARABIC, PFN_BULLET};
use crate::common::store::title_of;
use crate::win::theme;
use crate::win::util::{WM_MOUSELEAVE, key_down, mix, now, rect, wide, x_of, y_of};

mod attach;
pub use attach::adopt_files;
mod chrome;
mod commands;
mod lists;
mod input;
mod scrollbar;
use chrome::*;
use commands::*;
use lists::*;
use input::*;

const CLASS: &str = "PeekABooNote";
const T_SAVE: usize = 1;
const T_HIDE: usize = 3;
/// Keeps the footer's "Edited 2 minutes ago" current.
const T_AGE: usize = 4;
/// Ends the footer's "This note is full".
const T_FULL: usize = 5;
const FULL_MS: u32 = 4000;
const EN_MAXTEXT: u32 = 0x0501;
const SAVE_DELAY_MS: u32 = 250;
pub const WM_NOTE_CMD: u32 = WM_APP + 40;
const WM_RESTYLE: u32 = WM_APP + 41;

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    None,
    /// Done: archive the note.
    Done,
    Delete,
    Pin,
    Color,
    More,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum AutoKind {
    List,
    Task,
}

/// Remembers an automatic list conversion so Backspace can undo it.
struct Auto {
    kind: AutoKind,
    typed: String,
    pos: i32,
}

struct NoteWin {
    id: u64,
    edit: HWND,
    s: f32,
    hot: Btn,
    pressed: Btn,
    tracking: bool,
    auto: Option<Auto>,
    dirty: bool,
    loading: bool,
    discard: bool,
    /// When typing last ran into the note limit (the footer says so for a moment).
    full_at: Option<std::time::Instant>,
    swallow: bool,
    icons: Option<Font>,
    meta: Option<Font>,
    /// Header controls and footer fade in while the note is active or hovered.
    ctl: Spring,
    hot_a: Spring,
    active: bool,
    hover: bool,
    /// Hosted inside All Notes' detail card (child window, no chrome of its own).
    embedded: bool,
    /// The scroll bar (see `scrollbar`): shown, widened, pointer on it, held by a recent
    /// scroll, mid-drag (pointer y and scroll at the grab), last (scroll, content) seen.
    bar: Spring,
    bar_wide: Spring,
    bar_hot: bool,
    bar_hold: bool,
    bar_drag: Option<(i32, i32)>,
    bar_seen: Option<(i32, i32)>,
    /// Wheel scrolling glides to its target instead of jumping.
    glide: Spring,
}

fn state(h: HWND) -> Option<&'static mut NoteWin> {
    let p = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as *mut NoteWin;
    if p.is_null() { None } else { Some(unsafe { &mut *p }) }
}

thread_local! {
    static ICON_FACE: OnceCell<&'static str> = const { OnceCell::new() };
}

/// Segoe Fluent Icons on Windows 11, Segoe MDL2 Assets on Windows 10.
pub fn icon_face() -> &'static str {
    ICON_FACE.with(|c| {
        *c.get_or_init(|| unsafe {
            let f = gfx_font(12, "Segoe Fluent Icons");
            let dc = CreateCompatibleDC(null_mut());
            let old = SelectObject(dc, f);
            let mut buf = [0u16; 64];
            GetTextFaceW(dc, 64, buf.as_mut_ptr());
            SelectObject(dc, old);
            DeleteDC(dc);
            DeleteObject(f);
            if crate::win::util::from_wide(&buf).eq_ignore_ascii_case("Segoe Fluent Icons") {
                "Segoe Fluent Icons"
            } else {
                "Segoe MDL2 Assets"
            }
        })
    })
}

fn gfx_font(px: i32, face: &str) -> HFONT {
    let f = wide(face);
    unsafe {
        CreateFontW(-px, 0, 0, 0, FW_NORMAL as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_TT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, 0, f.as_ptr())
    }
}

pub fn register() {
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
}

/// Create a note window. With `show` false it stays hidden until `reveal` (used while the
/// ghost transition flies the card into place).
pub fn create(id: u64, r: RECT, show: bool) -> HWND {
    let pinned = app::store(|s| s.note(id).map(|n| n.pinned)).unwrap_or(false);
    let st = Box::new(NoteWin {
        id,
        edit: null_mut(),
        s: 1.0,
        hot: Btn::None,
        pressed: Btn::None,
        tracking: false,
        auto: None,
        dirty: false,
        loading: true,
        discard: false,
        full_at: None,
        swallow: false,
        icons: None,
        meta: None,
        ctl: Spring::unit(1.0, theme::motion::DEFAULT),
        hot_a: Spring::unit(0.0, theme::motion::SNAPPY),
        active: true,
        hover: false,
        embedded: false,
        bar: Spring::unit(0.0, theme::motion::DEFAULT),
        bar_wide: Spring::unit(0.0, theme::motion::SNAPPY),
        bar_hot: false,
        bar_hold: false,
        bar_drag: None,
        bar_seen: None,
        glide: Spring::px(0.0, theme::motion::SNAPPY),
    });
    // Hidden-until-arrival windows are created layered at alpha 0 so they can take focus
    // now (while we still hold the user's input) and become visible when the ghost lands.
    let ex = WS_EX_TOOLWINDOW | if pinned { WS_EX_TOPMOST } else { 0 } | if show { 0 } else { WS_EX_LAYERED };
    let style = WS_POPUP | WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    let h = unsafe {
        CreateWindowExW(
            ex,
            wide(CLASS).as_ptr(),
            wide(t!("note_untitled")).as_ptr(),
            style,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            null_mut(),
            null_mut(),
            app::hinst(),
            Box::into_raw(st) as _,
        )
    };
    if h.is_null() {
        return h;
    }
    unsafe {
        // Our own transitions replace the system's show/hide animation.
        let off: i32 = 1;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(h, 3 /* DWMWA_TRANSITIONS_FORCEDISABLED */, &off as *const i32 as _, 4);
        SetWindowPos(h, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
    // Dropping the system frame just grew the text area to its real size: place the text
    // again for it, before the opening card takes its picture of the window.
    open_at_place(h);
    if show {
        reveal(h);
    } else {
        unsafe {
            SetLayeredWindowAttributes(h, 0, 0, LWA_ALPHA);
            ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
        }
    }
    h
}

/// An editor for note `id` hosted inside `parent` (All Notes' detail card) at `r`
/// (parent client coordinates). Same editing behaviour as a note window, no chrome.
pub fn create_embedded(id: u64, parent: HWND, r: RECT) -> HWND {
    let st = Box::new(NoteWin {
        id,
        edit: null_mut(),
        s: 1.0,
        hot: Btn::None,
        pressed: Btn::None,
        tracking: false,
        auto: None,
        dirty: false,
        loading: true,
        discard: false,
        full_at: None,
        swallow: false,
        icons: None,
        meta: None,
        ctl: Spring::unit(0.0, theme::motion::DEFAULT),
        hot_a: Spring::unit(0.0, theme::motion::SNAPPY),
        active: false,
        hover: false,
        embedded: true,
        bar: Spring::unit(0.0, theme::motion::DEFAULT),
        bar_wide: Spring::unit(0.0, theme::motion::SNAPPY),
        bar_hot: false,
        bar_hold: false,
        bar_drag: None,
        bar_seen: None,
        glide: Spring::px(0.0, theme::motion::SNAPPY),
    });
    let h = unsafe {
        CreateWindowExW(
            0,
            wide(CLASS).as_ptr(),
            wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            r.left,
            r.top,
            (r.right - r.left).max(1),
            (r.bottom - r.top).max(1),
            parent,
            null_mut(),
            app::hinst(),
            Box::into_raw(st) as _,
        )
    };
    scrollbar::hint(h);
    h
}

pub fn is_embedded(h: HWND) -> bool {
    state(h).is_some_and(|s| s.embedded)
}

/// Make the window visible (and focused, if it isn't already).
pub fn reveal(h: HWND) {
    unsafe {
        if IsWindow(h) == 0 {
            return;
        }
        // The window has its final size only now (it was created at a default one). Done
        // while it's still invisible, so the text never shows a jump.
        open_at_place(h);
        scrollbar::hint(h);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_LAYERED != 0 {
            SetWindowLongPtrW(h, GWL_EXSTYLE, (ex & !WS_EX_LAYERED) as isize);
            SetWindowPos(h, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            // Paint everything now, in one go: out of the layered surface the window shows
            // black wherever it hasn't painted yet (the footer, if only the text had).
            RedrawWindow(h, std::ptr::null(), null_mut(), RDW_ERASE | RDW_INVALIDATE | RDW_FRAME | RDW_ALLCHILDREN | RDW_UPDATENOW);
        }
        ShowWindow(h, SW_SHOW);
        if GetForegroundWindow() != h {
            SetForegroundWindow(h);
        }
    }
}

/// Where you are in the text: the caret, the first character in view, and how far its
/// line is scrolled past the top (the first line is often only partly in view).
fn view_of(edit: HWND) -> [i32; 3] {
    const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
    const EM_LINEINDEX: u32 = 0x00BB;
    let line = rich::send(edit, EM_GETFIRSTVISIBLELINE, 0, 0);
    let top = rich::send(edit, EM_LINEINDEX, line as usize, 0).max(0) as i32;
    let cut = -rich::pos_from_char(edit, top).y;
    [rich::sel(edit).1.max(0), top, cut.max(0)]
}

/// Back where you left the note: the caret where it was, and the line that was at the top
/// at the top again (a character, not pixels, so it holds after a resize or a new text
/// size). A note never opened before waits at the end, where you'd carry on writing.
/// Loading and styling moved the selection around at the editor's earlier sizes and left
/// it scrolled anywhere, so the scroll is set outright, with drawing held until it's final.
fn open_at_place(h: HWND) {
    let Some(st) = state(h) else { return };
    let edit = st.edit;
    // A window reopens where it was left; All Notes' card always shows a note from its title.
    let view = if st.embedded { None } else { app::store(|s| s.note(st.id).and_then(|n| n.view)) };
    let len = rich::text(edit).len() as i32;
    unsafe { SendMessageW(edit, WM_SETREDRAW, 0, 0) };
    match view {
        Some([caret, top, cut]) if (0..=len).contains(&caret) && (0..=len).contains(&top) => {
            rich::set_sel(edit, caret, caret);
            let y = rich::pos_from_char(edit, top).y + rich::scroll_y(edit) + cut;
            rich::set_scroll_y(edit, y.min(scrollbar::max_scroll(edit)));
            scrollbar::keep_caret_in_view(edit);
        }
        // Never opened here before: from the top, title first.
        _ => {
            rich::set_sel(edit, 0, 0);
            rich::set_scroll_y(edit, 0);
        }
    }
    unsafe {
        SendMessageW(edit, WM_SETREDRAW, 1, 0);
        InvalidateRect(edit, std::ptr::null(), 1);
    }
}

/// Current look of a note for a ghost: (screen rect, colour, what it shows).
fn snapshot(h: HWND) -> Option<(RECT, u8, crate::win::ui::ghost::Content)> {
    let st = state(h)?;
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(h, &mut r) };
    let (color, text) = app::store(|s| s.note(st.id).map(|n| (n.color, n.text.clone())))?;
    Some((r, color, flight_content(h, &text)))
}

/// What the card flying to or from this window shows: the title and the top of the text,
/// or, when the text is scrolled, the lines from the first one wholly in view, placed where
/// the window has them, so the card and the window hand over without a jump.
pub fn flight_content(h: HWND, text: &str) -> crate::win::ui::ghost::Content {
    let image = if state(h).is_some_and(|st| !st.embedded) { crate::win::render::gfx::capture(h) } else { None };
    let whole = crate::win::ui::ghost::Content { title: title_of(text), body: crate::common::store::body_of(text), body_top: None, image, from_image: None };
    if whole.image.is_some() {
        return whole;
    }
    let Some(st) = state(h) else { return whole };
    if st.embedded || rich::scroll_y(st.edit) <= 0 {
        return whole;
    }
    let t = rich::text(st.edit);
    // `para` counts lines of `text`: a quote, code block or table is as many as it's
    // written in, and is stepped over whole.
    let (mut cp, mut para) = (0usize, 0usize);
    while rich::pos_from_char(st.edit, cp as i32).y < 0 {
        if t.get(cp) == Some(&rich::ROW_START)
            && let Some((_, lines, end)) = rich::light_block(st.edit, &t, cp)
        {
            cp = end;
            para += lines;
            continue;
        }
        let gap = cp >= 2 && t[cp - 2] == rich::ROW_END && t.get(cp + 1) == Some(&rich::ROW_START);
        let Some(i) = t[cp..].iter().position(|&c| c == rich::CR) else { return whole };
        cp += i + 1;
        if !gap {
            para += 1;
        }
        if cp >= t.len() {
            return whole;
        }
    }
    let y = rich::pos_from_char(st.edit, cp as i32).y as f32;
    let body_top = header_h(st.s) as f32 + 2.0 * st.s + y;
    crate::win::ui::ghost::Content { title: String::new(), body: crate::common::store::body_from(text, para), body_top: Some(body_top), image: None, from_image: None }
}

/// Ghost the note back to its tab (or the pill) as the window disappears.
fn fly_home(h: HWND) {
    let Some(st) = state(h) else { return };
    let id = st.id;
    let Some((r, color, content)) = snapshot(h) else { return };
    if content.title == t!("note_untitled") && content.body.is_empty() {
        return; // an empty note simply vanishes
    }
    let home = crate::win::ui::deck::home_rect(id);
    crate::win::ui::ghost::fly(crate::win::ui::ghost::Kind::Close, (r, 8.0 * st.s), home, color, content, None);
}

/// Sink and dissolve in place (archive / delete).
pub fn dissolve(h: HWND) {
    let Some(st) = state(h) else { return };
    let Some((r, color, content)) = snapshot(h) else { return };
    let (w, hh) = (r.right - r.left, r.bottom - r.top);
    let to = RECT { left: r.left + w / 20, top: r.top + hh / 12, right: r.right - w / 20, bottom: r.bottom - hh / 40 };
    crate::win::ui::ghost::fly(crate::win::ui::ghost::Kind::Dissolve, (r, 8.0 * st.s), (to, 8.0 * st.s), color, content, None);
}

/// Fade header controls for every note that is mid-transition.
pub fn frame(dt: f32) -> bool {
    let mut wins: Vec<HWND> = app::ui(|u| u.notes.values().copied().collect());
    // All Notes' editor scrolls and shows its bar like a note window.
    let inline = crate::win::ui::allnotes::editor();
    if !inline.is_null() {
        wins.push(inline);
    }
    let mut moving = false;
    for h in wins {
        if let Some(st) = state(h) {
            let a = st.ctl.step(dt);
            let b = st.hot_a.step(dt);
            let c = st.bar.step(dt) | st.bar_wide.step(dt);
            if scrollbar::step(st, dt) {
                moving = true;
            }
            if a || b || c {
                moving = true;
                unsafe { InvalidateRect(h, std::ptr::null(), 0) };
            }
        }
    }
    crate::win::editor::checks::frame(dt) || moving
}

fn set_controls(h: HWND) {
    if let Some(st) = state(h) {
        st.ctl.to(if st.active || st.hover { 1.0 } else { 0.0 });
        crate::win::render::anim::wake(crate::win::render::anim::Client::Notes);
    }
}

/// The pointer is over some part of the note (its header, its text or its frame):
/// show the header controls.
pub(super) fn pointer_in(h: HWND) {
    if let Some(st) = state(h)
        && !st.hover
    {
        st.hover = true;
        set_controls(h);
    }
}

/// The pointer left one part of the note: hide the controls only if it left the note.
pub(super) fn pointer_out(h: HWND) {
    let mut p = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(h, &mut r);
    }
    if crate::win::util::pt_in(&r, p.x, p.y) {
        return;
    }
    if let Some(st) = state(h) {
        st.hover = false;
        set_controls(h);
    }
}

pub fn id_of(h: HWND) -> Option<u64> {
    state(h).map(|s| s.id)
}

/// Persist pending edits now.
pub fn flush(h: HWND) {
    let Some(st) = state(h) else { return };
    unsafe { KillTimer(h, T_SAVE) };
    let (id, edit) = (st.id, st.edit);
    // The place to reopen at is the note window's: the card in All Notes leaves it be.
    let view = if st.embedded { app::store(|s| s.note(id).and_then(|n| n.view)) } else { Some(view_of(edit)) };
    if !st.dirty {
        // Only moved around: keep the place (not the "edited" time) when it changed.
        if app::store(|s| s.note(id).map(|n| n.view)) != Some(view) {
            app::store_mut(|s| {
                if let Some(n) = s.note_mut(id) {
                    n.view = view;
                }
            });
            app::save();
        }
        return;
    }
    st.dirty = false;
    let rtf = rich::get_rtf(edit);
    let text = app::rtf_to_text(&rtf);
    let title = title_of(&text);
    app::store_mut(|s| {
        if let Some(n) = s.note_mut(id) {
            n.rtf = rtf;
            n.text = text;
            n.updated = now();
            n.view = view;
        }
    });
    app::save();
    unsafe {
        SetWindowTextW(h, wide(&title).as_ptr());
        InvalidateRect(h, std::ptr::null(), 0); // the footer's edited time
    }
    app::refresh();
}

/// Close without saving (the note is being deleted or archived by the caller).
pub fn discard(h: HWND) {
    if let Some(st) = state(h) {
        st.discard = true;
    }
    unsafe { DestroyWindow(h) };
}

pub fn retheme(h: HWND) {
    apply_theme(h);
}

fn mark_dirty(h: HWND) {
    if let Some(st) = state(h)
        && !st.loading {
            st.dirty = true;
            unsafe {
                SetTimer(h, T_SAVE, SAVE_DELAY_MS, None);
                PostMessageW(h, WM_RESTYLE, 0, 0);
            }
        }
}

/// An emoji variation selector got into the editor directly (the emoji panel puts text
/// in without a key message). RichEdit would stop drawing list bullets for good, so
/// reload the content with the selector as a hidden marker (see `rich::selectors`),
/// keeping the caret and scroll.
fn unselect(h: HWND) {
    let Some(st) = state(h) else { return };
    let old = st.edit;
    let (a, b) = rich::sel(old);
    let y = rich::scroll_y(old);
    let focused = unsafe { GetFocus() } == old;
    let rtf = rich::get_rtf(old);
    // The damage stays with the control, whatever is loaded into it: a fresh one.
    let edit = new_editor(h);
    st.loading = true;
    st.edit = edit;
    rich::set_rtf(edit, &rtf);
    apply_theme(h);
    connect_editor(h, edit);
    layout(h);
    rich::set_sel(edit, a, b);
    rich::set_scroll_y(edit, y);
    st.loading = false;
    st.dirty = true;
    unsafe {
        DestroyWindow(old);
        if focused {
            SetFocus(edit);
        }
    }
    mark_dirty(h);
}

/// Show note `id`'s stored content again wherever it's open (it was changed elsewhere,
/// by an assistant say), keeping the place in it as far as the new content allows.
pub fn reload(id: u64) {
    let open = [app::ui(|u| u.notes.get(&id).copied()), crate::win::ui::allnotes::editor_for(id)];
    let (rtf, text) = app::store(|s| s.note(id).map(|n| (n.rtf.clone(), n.text.clone()))).unwrap_or_default();
    for h in open.into_iter().flatten() {
        let Some(st) = state(h) else { continue };
        let (a, _) = rich::sel(st.edit);
        let y = rich::scroll_y(st.edit);
        st.loading = true;
        if rtf.is_empty() {
            rich::load_markdown(st.edit, &text);
        } else {
            rich::set_rtf(st.edit, &rtf);
        }
        st.dirty = false;
        apply_theme(h);
        let len = rich::text(st.edit).len() as i32;
        rich::set_sel(st.edit, a.min(len), a.min(len));
        rich::set_scroll_y(st.edit, y);
        st.loading = false;
        unsafe { SetWindowTextW(h, wide(&title_of(&text)).as_ptr()) };
    }
}

fn apply_theme(h: HWND) {
    let Some(st) = state(h) else { return };
    let dark = app::dark();
    let color = app::store(|s| s.note(st.id).map(|n| n.color)).unwrap_or(0);
    let bg = theme::paper(color, dark);
    let was = st.loading;
    st.loading = true;
    rich::bulk(st.edit, || {
        rich::apply_base_format(st.edit, theme::PAPER_TEXT, bg);
        rich::style_title(st.edit);
        rich::set_box_colors(st.edit, bg, theme::paper_subtext(color, dark), theme::PAPER_TEXT);
        crate::win::editor::checks::set_style(st.edit, theme::paper_ink(color, dark), bg, st.s);
        rich::style_boxes(st.edit);
        rich::style_bullets(st.edit);
        rich::set_block_colors(st.edit, bg, theme::PAPER_TEXT, dark);
        rich::style_blocks(st.edit);
        rich::color_named_links(st.edit);
    });
    st.loading = was;
    if !st.embedded {
        theme::set_border_color(h, mix(theme::color(color).ink, bg, 0.78));
        theme::set_dark_titlebar(h, false);
    }
    unsafe { InvalidateRect(h, std::ptr::null(), 1) };
}

/// The RichEdit for note window `h`, set up but empty and not yet listened to.
fn new_editor(h: HWND) -> HWND {
    let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_NOHIDESEL) as u32;
    let edit = unsafe { CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), style, 0, 0, 10, 10, h, 100 as _, app::hinst(), null_mut()) };
    rich::setup(edit);
    edit
}

/// Start listening to editor `edit` (once its content is in).
fn connect_editor(h: HWND, edit: HWND) {
    rich::send(edit, rich::EM_SETEVENTMASK, 0, (rich::ENM_CHANGE | rich::ENM_LINK) as isize);
    attach::hook(edit);
    unsafe { SetWindowSubclass(edit, Some(edit_proc), 1, h as usize) };
}

fn on_create(h: HWND) {
    let Some(st) = state(h) else { return };
    let dpi = unsafe { GetDpiForWindow(h) }.max(96);
    st.s = dpi as f32 / 96.0;
    st.icons = Some(Font::new(13.0 * st.s, FW_NORMAL, icon_face()));
    st.meta = Some(Font::new(11.5 * st.s, FW_NORMAL, theme::FACE_SMALL));
    unsafe { SetTimer(h, T_AGE, 30_000, None) };
    let edit = new_editor(h);
    st.edit = edit;
    let (rtf, text) = app::store(|s| s.note(st.id).map(|n| (n.rtf.clone(), n.text.clone()))).unwrap_or_default();
    // In bulk: a long note's thousands of format changes are laid out once, at the end.
    rich::bulk(edit, || {
        if !rtf.is_empty() {
            rich::set_rtf(edit, &rtf);
        } else if !text.is_empty() {
            rich::load_markdown(edit, &text);
        }
        // Addresses in notes saved before links were ours, or that came in from elsewhere.
        rich::link_addresses(edit, 0, usize::MAX);
        apply_theme(h);
    });
    rich::send(edit, rich::EM_EMPTYUNDOBUFFER, 0, 0);
    rich::limit_length(edit);
    connect_editor(h, edit);
    unsafe {
        // Files dropped on the frame (header, margins) too.
        windows_sys::Win32::UI::Shell::DragAcceptFiles(h, 1);
        SetWindowTextW(h, wide(&title_of(&text)).as_ptr());
        if !st.embedded {
            let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
            DwmExtendFrameIntoClientArea(h, &m);
        }
    }
    st.loading = false;
    layout(h);
    open_at_place(h);
    if !st.embedded {
        theme::set_round_corners(h, false);
        app::ui(|u| u.notes.insert(st.id, h));
        // All Notes swaps that note's card for a "Show Window" button.
        crate::win::ui::allnotes::refresh();
    }
}

fn on_destroy(h: HWND) {
    let Some(st) = state(h) else { return };
    let id = st.id;
    if st.embedded {
        flush(h);
        return;
    }
    if !st.discard {
        flush(h);
        let empty = app::store(|s| s.note(id).map(|n| n.text.trim().is_empty())).unwrap_or(false);
        if empty {
            app::store_mut(|s| s.remove(id));
            app::save();
        }
    }
    app::ui(|u| {
        if u.notes.get(&id) == Some(&h) {
            u.notes.remove(&id);
        }
    });
    app::refresh();
}

/// Open a link: a web address in the browser, an attachment with its own app.
pub(super) fn open_link(url: &str) {
    crate::win::ui::tooltip::mute();
    let target = match crate::common::attach::name_of(url) {
        Some(_) => match attach::file_of(url) {
            Some(p) => p.to_string_lossy().into_owned(),
            None => return,
        },
        None => url.to_string(),
    };
    unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(&target).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCCREATE => {
                let cs = &*(lp as *const CREATESTRUCTW);
                SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
            }
            WM_CREATE => {
                on_create(h);
                return 0;
            }
            WM_NCCALCSIZE if wp != 0 => return 0,
            WM_NCACTIVATE => return DefWindowProcW(h, msg, wp, -1),
            WM_NCHITTEST if is_embedded(h) => return HTCLIENT as LRESULT,
            WM_NCHITTEST => return hit_test(h, lp),
            WM_GETMINMAXINFO => {
                let s = state(h).map(|s| s.s).unwrap_or(1.0);
                let mm = &mut *(lp as *mut MINMAXINFO);
                mm.ptMinTrackSize = POINT { x: (220.0 * s) as i32, y: (150.0 * s) as i32 };
                return 0;
            }
            WM_SIZE => {
                layout(h);
                InvalidateRect(h, std::ptr::null(), 0);
                return 0;
            }
            WM_PAINT => {
                paint(h);
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_ACTIVATE => {
                if let Some(st) = state(h) {
                    // Our own popover menu doesn't count as leaving the note.
                    st.active = (wp & 0xFFFF) as u32 != WA_INACTIVE || crate::win::ui::menu::is_menu(lp as HWND);
                    if st.active {
                        KillTimer(h, T_HIDE);
                        SetFocus(st.edit);
                        let id = st.id;
                        if app::store(|s| s.d.settings.last_note) != Some(id) {
                            app::store_mut(|s| s.d.settings.last_note = Some(id));
                            app::save();
                        }
                    } else {
                        flush(h);
                        hide_later(h, lp as HWND);
                    }
                    set_controls(h);
                }
                return 0;
            }
            WM_NCMOUSEMOVE => {
                if let Some(st) = state(h)
                    && !st.hover {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: h, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        st.hover = true;
                        set_controls(h);
                    }
            }
            0x02A2 /* WM_NCMOUSELEAVE */ => pointer_out(h),
            WM_SETFOCUS => {
                if let Some(st) = state(h) {
                    SetFocus(st.edit);
                }
                return 0;
            }
            WM_COMMAND => {
                match ((wp >> 16) & 0xFFFF) as u32 {
                    rich::EN_CHANGE => mark_dirty(h),
                    // At the limit: the footer says so for a few seconds.
                    EN_MAXTEXT => {
                        if let Some(st) = state(h) {
                            st.full_at = Some(std::time::Instant::now());
                        }
                        SetTimer(h, T_FULL, FULL_MS, None);
                        InvalidateRect(h, std::ptr::null(), 0);
                    }
                    _ => {}
                }
                return 0;
            }
            WM_NOTIFY => {
                let hdr = &*(lp as *const NMHDR);
                if hdr.code == rich::EN_LINK {
                    let el = &*(lp as *const rich::EnLink);
                    let (m, r) = (el.msg, el.chrg);
                    if m == WM_LBUTTONUP
                        && let Some(st) = state(h) {
                            let url = rich::url_of(st.edit, r.min, r.max);
                            open_link(&url);
                        }
                }
                return 0;
            }
            WM_NOTE_CMD => {
                run_command(h, wp as u32);
                return 0;
            }
            attach::WM_ATTACH => {
                attach::flush_pending(h);
                return 0;
            }
            WM_DROPFILES => {
                attach::dropped_on_frame(h, wp);
                return 0;
            }
            WM_RESTYLE => {
                // Coalesce: one restyle per burst of edits.
                let mut m: MSG = std::mem::zeroed();
                while PeekMessageW(&mut m, h, WM_RESTYLE, WM_RESTYLE, PM_REMOVE) != 0 {}
                if let Some(st) = state(h) {
                    let was = st.loading;
                    st.loading = true;
                    rich::style_title(st.edit);
                    rich::color_named_links(st.edit);
                    rich::fix_caret_indent(st.edit);
                    if rich::has_selectors(&rich::text(st.edit)) {
                        unselect(h);
                    }
                    st.loading = was;
                    if rich::text(st.edit).iter().any(|&c| is_task(c)) {
                        InvalidateRect(st.edit, std::ptr::null(), 0);
                    }
                }
                return 0;
            }
            WM_TIMER if wp == T_SAVE => {
                flush(h);
                return 0;
            }
            WM_TIMER if wp == T_FULL => {
                KillTimer(h, T_FULL);
                if let Some(st) = state(h) {
                    st.full_at = None;
                }
                InvalidateRect(h, std::ptr::null(), 0);
                return 0;
            }
            WM_TIMER if wp == T_AGE => {
                if let Some(st) = state(h)
                    && !st.embedded
                {
                    InvalidateRect(h, std::ptr::null(), 0);
                }
                return 0;
            }
            WM_TIMER if wp == T_HIDE => {
                hide_tick(h);
                return 0;
            }
            WM_TIMER if wp == scrollbar::T_BAR => {
                scrollbar::timer(h);
                return 0;
            }
            WM_CAPTURECHANGED => scrollbar::cancel(h),
            // The wheel over the margins (the scroll bar's included) scrolls the text too.
            WM_MOUSEWHEEL => {
                scrollbar::wheel(h, (wp >> 16) as i16);
                return 0;
            }
            WM_MOUSEMOVE => {
                let on_bar = scrollbar::mouse_move(h, x_of(lp), y_of(lp));
                let b = if on_bar { Btn::None } else { btn_at(h, x_of(lp), y_of(lp)) };
                if let Some(st) = state(h) {
                    if !st.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        st.tracking = true;
                    }
                    pointer_in(h);
                    tip_for(h, b);
                    if st.hot != b {
                        if b != Btn::None {
                            st.hot = b;
                            st.hot_a.snap(0.0);
                        }
                        st.hot_a.to(if b != Btn::None { 1.0 } else { 0.0 });
                        crate::win::render::anim::wake(crate::win::render::anim::Client::Notes);
                    }
                }
                return 0;
            }
            WM_MOUSELEAVE => {
                if let Some(st) = state(h) {
                    st.tracking = false;
                    st.hot_a.to(0.0);
                }
                crate::win::ui::tooltip::leave(h);
                scrollbar::mouse_leave(h);
                pointer_out(h);
                return 0;
            }
            WM_LBUTTONDOWN => {
                crate::win::ui::tooltip::mute();
                if scrollbar::mouse_down(h, x_of(lp), y_of(lp)) {
                    return 0;
                }
                let b = btn_at(h, x_of(lp), y_of(lp));
                if let Some(st) = state(h) {
                    st.pressed = b;
                }
                return 0;
            }
            WM_LBUTTONUP => {
                if scrollbar::mouse_up(h) {
                    return 0;
                }
                let b = btn_at(h, x_of(lp), y_of(lp));
                let pressed = state(h).map(|s| std::mem::replace(&mut s.pressed, Btn::None));
                if pressed == Some(b) {
                    click(h, b);
                }
                return 0;
            }
            WM_NCRBUTTONUP if wp as u32 == HTCAPTION => {
                if let Some(id) = id_of(h) {
                    let cmd = app::popup(app::note_menu(id, true), x_of(lp), y_of(lp), h);
                    if cmd != 0 {
                        PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0);
                    }
                }
                return 0;
            }
            WM_EXITSIZEMOVE => {
                if let Some(st) = state(h) {
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(h, &mut r);
                    let id = st.id;
                    app::store_mut(|s| {
                        if let Some(n) = s.note_mut(id) {
                            n.rect = Some([r.left, r.top, r.right, r.bottom]);
                        }
                    });
                    app::save();
                }
                return 0;
            }
            WM_DPICHANGED => {
                if let Some(st) = state(h) {
                    st.s = ((wp & 0xFFFF) as f32 / 96.0).max(1.0);
                    st.icons = Some(Font::new(13.0 * st.s, FW_NORMAL, icon_face()));
                    st.meta = Some(Font::new(11.5 * st.s, FW_NORMAL, theme::FACE_SMALL));
                    let r = &*(lp as *const RECT);
                    SetWindowPos(h, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                    layout(h);
                }
                return 0;
            }
            WM_CLOSE => {
                flush(h);
                fly_home(h);
                DestroyWindow(h);
                return 0;
            }
            WM_DESTROY => {
                crate::win::ui::tooltip::leave(h);
                on_destroy(h);
                app::schedule_trim();
                return 0;
            }
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut NoteWin;
                SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
