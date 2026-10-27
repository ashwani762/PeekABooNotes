// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Popover menus: the app's own context and tray menus.
//!
//! Drawn in the same material and type as the rest of PeekABoo Notes (native Win32 menus can't be
//! styled to match). Opens with a 120 ms fade-and-settle from the pointer, highlights on
//! hover instantly, confirms a choice with a quick blink and fades out.
//! `show` runs a nested message loop, like `TrackPopupMenu`, and returns the command id
//! (0 if dismissed). Keyboard: arrows (Up and Down between rows, Left and Right along
//! the colour and tool rows), Enter or Space, Esc, and first-letter jumps.
//!
//! A submenu row (with a chevron) opens its menu beside it when the pointer rests on it,
//! when it's clicked, or with Right or Enter; Left or Esc closes it again. The menus open at
//! once share the one nested loop, and the first holds the mouse for all of them, so the
//! pointer is followed across them without any window of theirs taking the keys.
//!
//! Dismissal fades too, and lets go of the mouse at once, so whatever is under the
//! pointer responds while the menu is still disappearing. A caller can also pass a
//! `leave` test: when the pointer moves somewhere it says the menu no longer belongs
//! (e.g. onto a different deck card), the menu fades away by itself.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{DT_CENTER, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, FW_SEMIBOLD};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::win::render::anim::{self, Client, Spring, clamp01};
use crate::win::app;
use crate::win::render::gfx::{Canvas, Font, Shape};
use crate::win::theme;
use crate::win::util::{WM_MOUSELEAVE, mix, rect, wide, x_of, y_of};

const CLASS: &str = "PeekABooMenu";

#[derive(Clone, Copy)]
pub enum ToolStyle {
    Bold,
    Italic,
    Underline,
    Strike,
    Plain,
    /// A glyph from the icon font.
    Icon,
}

pub struct Tool {
    pub id: u32,
    pub glyph: &'static str,
    pub style: ToolStyle,
    /// The tooltip: what it does, and its shortcut ("" for none).
    pub label: &'static str,
    pub key: String,
}

pub enum Item {
    /// A row of colour swatches; choosing one returns `base + colour index`.
    Swatches { base: u32, current: Option<u8> },
    /// A row of compact icon buttons (text formatting).
    Tools(Vec<Tool>),
    /// A `disabled` row is greyed out: no highlight, and neither the mouse nor the keys pick it.
    Action { id: u32, label: String, key: String, icon: &'static str, checked: bool, danger: bool, disabled: bool },
    /// A row that opens a menu of `items` beside it. They're built only when it opens, so a
    /// menu nobody looks into costs nothing to fill; their ids come back from `show` like
    /// any others, so they mustn't clash with the rest of the menu's.
    Submenu { label: String, icon: &'static str, items: Box<dyn Fn() -> Vec<Item>>, disabled: bool },
    Separator,
}

impl Item {
    /// The same row, greyed out unless `on`.
    pub fn enabled(mut self, on: bool) -> Item {
        if let Item::Action { disabled, .. } | Item::Submenu { disabled, .. } = &mut self {
            *disabled = !on;
        }
        self
    }
}

pub fn action(id: u32, label: &str, key: &str, icon: &'static str) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon, checked: false, danger: false, disabled: false }
}

pub fn check(id: u32, label: &str, key: &str, checked: bool) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon: "", checked, danger: false, disabled: false }
}

pub fn danger(id: u32, label: &str, key: &str, icon: &'static str) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon, checked: false, danger: true, disabled: false }
}

pub fn submenu(label: &str, icon: &'static str, items: impl Fn() -> Vec<Item> + 'static) -> Item {
    Item::Submenu { label: label.into(), icon, items: Box::new(items), disabled: false }
}

#[derive(Clone, Copy, PartialEq)]
enum Target {
    None,
    Row(usize),
    Swatch(usize, usize), // (item, swatch position in display order)
    Tool(usize, usize),
}

struct Fonts {
    label: Font,
    key: Font,
    icon: Font,
    /// The submenu rows' chevron: smaller than the icons, as Windows draws it.
    chevron: Font,
    tools: [Font; 5],
}

/// One menu window: the one `show` opened, or a submenu beside it.
struct Menu {
    hwnd: HWND,
    items: Vec<Item>,
    s: f32,
    fonts: Fonts,
    w: i32,
    h: i32,
    margin: i32,
    /// The window's top-left on screen; the card starts `margin` in from it.
    origin: (i32, i32),
    /// The side it opens from (a unit step in x, y): the settle starts that way.
    from: (f32, f32),
    hot: Target,
    fade: Spring,
    canvas: Option<Canvas>,
    has_icons: bool,
    /// It opened leftwards (right-aligned, or turned back at the screen's edge), so its
    /// submenus do too: away from what the caller kept it clear of, such as the deck.
    leftward: bool,
    /// A submenu's row in the menu before it, the one that opened it.
    parent_row: usize,
}

impl Menu {
    /// Whether screen point `p` is on the card.
    fn holds(&self, p: POINT) -> bool {
        let (x, y) = (p.x - self.origin.0 - self.margin, p.y - self.origin.1 - self.margin);
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }

    /// What's under screen point `p`.
    fn target(&self, p: POINT) -> Target {
        target_at(self, p.x - self.origin.0, p.y - self.origin.1)
    }
}

#[derive(Clone, Copy)]
struct Blink {
    cmd: u32,
    t0: Instant,
    /// The menu it's in, and where.
    level: usize,
    at: Target,
}

/// The menus showing: the one `show` opened first, then each open submenu, beside the one
/// before it.
struct Open {
    menus: Vec<Menu>,
    /// The menu the keys act on.
    keys: usize,
    blink: Option<Blink>,
    result: Option<u32>,
    /// Fading out after a dismissal; input is ignored and passes through.
    closing: bool,
    /// Where the pointer last was (screen coordinates).
    pointer: (i32, i32),
    /// What the pointer resting on a row will do to the submenus once `T_SUB` fires, and
    /// in which menu.
    pending: Option<(usize, Hover)>,
}

/// Tells the menu whether the pointer, outside it, has moved somewhere it no longer belongs.
type LeaveTest = Box<dyn Fn(POINT) -> bool>;

thread_local! {
    static MENU: RefCell<Option<Open>> = const { RefCell::new(None) };
    static LEAVE: RefCell<Option<LeaveTest>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Open) -> R) -> Option<R> {
    MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.as_mut().map(f)))
}

/// The newest menu's window (null with none up), whose display its frames keep to.
pub fn window() -> HWND {
    MENU.with(|m| m.try_borrow().ok().and_then(|m| m.as_ref().and_then(|o| o.menus.last().map(|m| m.hwnd)))).unwrap_or(null_mut())
}

pub fn is_menu(h: HWND) -> bool {
    !h.is_null() && with(|o| o.menus.iter().any(|m| m.hwnd == h)).unwrap_or(false)
}

// Metrics (device-independent px).
const PAD: f32 = 5.0;
const ROW: f32 = 28.0;
const SEP: f32 = 9.0;
const STRIP: f32 = 40.0;
const SW: f32 = 16.0;
const SW_GAP: f32 = 7.0;
const TOOL: f32 = 30.0;
/// How far a submenu's card overlaps its menu's edge, so that there's no gap between the
/// two where the pointer, crossing over, would be on neither.
const OVERLAP: f32 = 3.0;
const CHEVRON: &str = "\u{E76C}";
const CHEVRON_LEFT: &str = "\u{E76B}";

/// The timer for opening or closing a submenu after the pointer has rested a moment.
const T_SUB: usize = 1;
/// How long the pointer rests on a submenu row before it opens: long enough that passing
/// over the row on the way somewhere else doesn't flash it open.
const OPEN_MS: u32 = 250;
/// How long the pointer rests on another row before an open submenu closes, so that
/// cutting across a corner of the menu on the way into the submenu doesn't lose it.
const CLOSE_MS: u32 = 300;

fn item_h(it: &Item) -> f32 {
    match it {
        Item::Swatches { .. } | Item::Tools(_) => STRIP,
        Item::Action { .. } | Item::Submenu { .. } => ROW,
        Item::Separator => SEP,
    }
}

fn pal(dark: bool) -> (u32, u32, u32, u32, u32) {
    // (surface, edge, text, secondary, highlight); the secondary (shortcuts, icons) reads
    // at 4.5:1 on the surface.
    if dark { (0x2C2C2E, 0xFFFFFF, 0xF2F2F7, 0x98989D, 0x0A84FF) } else { (0xF7F7F9, 0x000000, 0x1D1D1F, 0x6E6E73, 0x0A84FF) }
}

fn fonts(s: f32) -> Fonts {
    let f = |px: f32, w: u32, face: &str| Font::new(px * s, w, face);
    let icon_face = crate::win::ui::note::icon_face();
    Fonts {
        label: f(13.0, FW_NORMAL, theme::FACE_TEXT),
        key: f(12.0, FW_NORMAL, theme::FACE_TEXT),
        icon: f(14.0, FW_NORMAL, icon_face),
        chevron: f(10.0, FW_NORMAL, icon_face),
        tools: [
            f(14.0, FW_BOLD, theme::FACE_TEXT),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, true, false, false),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, false, true, false),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, false, false, true),
            f(14.0, FW_SEMIBOLD, theme::FACE_TEXT),
        ],
    }
}

/// The card's size for `items`, in pixels: sized to the content. Labels always leave room
/// for an icon / checkmark column, so checked and unchecked rows line up.
fn measure(items: &[Item], fonts: &Fonts, s: f32) -> (i32, i32) {
    let mut w = 0.0f32;
    let mut h = PAD * 2.0;
    for it in items {
        h += item_h(it);
        let need = match it {
            Item::Action { label, key, .. } => {
                let lw = fonts.label.measure(label).0 as f32 / s;
                let kw = if key.is_empty() { 0.0 } else { fonts.key.measure(key).0 as f32 / s + 28.0 };
                40.0 + lw + kw + 18.0
            }
            Item::Submenu { label, .. } => {
                let lw = fonts.label.measure(label).0 as f32 / s;
                40.0 + lw + fonts.chevron.measure(CHEVRON).0 as f32 / s + 28.0 + 18.0
            }
            Item::Swatches { .. } => 2.0 * 14.0 + theme::ORDER.len() as f32 * SW + (theme::ORDER.len() - 1) as f32 * SW_GAP,
            Item::Tools(t) => 2.0 * 14.0 + t.len() as f32 * TOOL + (t.len().saturating_sub(1)) as f32 * 4.0,
            Item::Separator => 0.0,
        };
        w = w.max(need);
    }
    ((w.max(220.0) * s).round() as i32, (h * s).round() as i32)
}

/// A menu window (hidden) with its top-left at `x`, `y`. A submenu's never takes the keys
/// or becomes the active window: the first menu keeps both.
fn create(x: i32, y: i32, w: i32, h: i32, sub: bool) -> HWND {
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | if sub { WS_EX_NOACTIVATE } else { 0 };
    let hwnd = unsafe { CreateWindowExW(ex, wide(CLASS).as_ptr(), wide("Menu").as_ptr(), WS_POPUP, x, y, w, h, null_mut(), null_mut(), app::hinst(), null_mut()) };
    crate::win::util::no_transitions(hwnd);
    hwnd
}

/// Tell Windows a menu popup opened or closed, as its own menus do (`TrackPopupMenu`
/// raises these). The taskbar's hidden-icons flyout stays open while the menu of one of its
/// icons is up only when it hears this; without it, the flyout closed as soon as the
/// pointer went from it into the menu. Screen readers announce the menu from it too.
fn announce(hwnd: HWND, open: bool) {
    const OBJID_CLIENT: i32 = -4;
    const CHILDID_SELF: i32 = 0;
    let event = if open { EVENT_SYSTEM_MENUPOPUPSTART } else { EVENT_SYSTEM_MENUPOPUPEND };
    unsafe { windows_sys::Win32::UI::Accessibility::NotifyWinEvent(event, hwnd, OBJID_CLIENT, CHILDID_SELF) };
}

/// Show a menu at `at` (screen coordinates). Returns the chosen command id, 0 if dismissed.
pub fn show(items: Vec<Item>, at: POINT) -> u32 {
    show_with(items, at, false, None)
}

/// `show`, optionally right-aligned to `at` (opening leftwards), with a `leave` test
/// called with the pointer's screen position whenever it is outside the menu.
pub fn show_with(items: Vec<Item>, at: POINT, align_right: bool, leave: Option<Box<dyn Fn(POINT) -> bool>>) -> u32 {
    if with(|_| ()).is_some() {
        return 0; // one menu at a time
    }
    LEAVE.with(|l| *l.borrow_mut() = leave);
    let (work, s) = crate::win::ui::screen::at(at);
    let fonts = fonts(s);
    let (w, h) = measure(&items, &fonts, s);
    let margin = (22.0 * s) as i32;
    // The card and its shadow on the pointer's monitor.
    let work = crate::win::ui::screen::inset(work, margin);
    // Keep on screen: open down-right of the pointer, flipping when needed.
    let mut x = if align_right { at.x - w } else { at.x };
    let mut y = at.y;
    let flip_y = y + h > work.bottom;
    let leftward = align_right || x + w > work.right;
    if x + w > work.right {
        x -= w;
    }
    if flip_y {
        y -= h;
    }
    x = x.clamp(work.left, (work.right - w).max(work.left));
    y = y.clamp(work.top, (work.bottom - h).max(work.top));
    let hwnd = create(x - margin, y - margin, w + 2 * margin, h + 2 * margin, false);
    let mut fade = Spring::fade(0.0, (1.0, 0.12));
    fade.to(1.0);
    let menu = Menu {
        hwnd,
        items,
        s,
        fonts,
        w,
        h,
        margin,
        origin: (x - margin, y - margin),
        // Settle toward the pointer's side: down from above it, or up when flipped above it.
        from: (0.0, if flip_y { 1.0 } else { -1.0 }),
        hot: Target::None,
        fade,
        canvas: None,
        has_icons: true,
        leftward,
        parent_row: 0,
    };
    MENU.with(|m| {
        *m.borrow_mut() = Some(Open { menus: vec![menu], keys: 0, blink: None, result: None, closing: false, pointer: (i32::MIN, i32::MIN), pending: None })
    });
    with(|o| draw(o, 0));
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        announce(hwnd, true);
        SetForegroundWindow(hwnd);
        SetCapture(hwnd);
    }
    anim::wake(Client::Menu);
    // Nested loop until a choice is made or the menu is dismissed.
    loop {
        if with(|o| o.result.is_some()).unwrap_or(true) {
            break;
        }
        if !app::pump_once() {
            break;
        }
    }
    let result = MENU.with(|m| m.borrow_mut().take()).map(|o| {
        unsafe {
            ReleaseCapture();
            KillTimer(hwnd, T_SUB);
            for m in o.menus.iter().rev() {
                announce(m.hwnd, false);
                DestroyWindow(m.hwnd);
            }
        }
        o.result.unwrap_or(0)
    });
    LEAVE.with(|l| l.borrow_mut().take());
    result.unwrap_or(0)
}

pub fn frame(dt: f32) -> bool {
    with(|o| {
        let mut moving = false;
        for m in &mut o.menus {
            moving |= m.fade.step(dt);
        }
        if o.closing {
            draw_all(o);
            if o.menus[0].fade.value < 0.03 || !moving {
                o.result = Some(0);
                return false;
            }
            return true;
        }
        // Confirmation blink, then close.
        if let Some(b) = o.blink {
            let ms = b.t0.elapsed().as_millis();
            if ms > 190 {
                o.result = Some(b.cmd);
                return false;
            }
            if ms > 110 && o.menus[0].fade.target > 0.0 {
                for m in &mut o.menus {
                    m.fade.retarget(0.0, (1.0, 0.1));
                    m.fade.launch(0.0);
                }
            }
            draw_all(o);
            return true;
        }
        draw_all(o);
        moving
    })
    .unwrap_or(false)
}

fn layout_rows(m: &Menu) -> Vec<(f32, f32)> {
    let s = m.s;
    let mut y = PAD * s;
    m.items
        .iter()
        .map(|it| {
            let h = item_h(it) * s;
            let r = (y, h);
            y += h;
            r
        })
        .collect()
}

fn swatch_x(m: &Menu, k: usize) -> f32 {
    let s = m.s;
    let n = theme::ORDER.len() as f32;
    let total = n * SW + (n - 1.0) * SW_GAP;
    let x0 = (m.w as f32 / s - total) / 2.0;
    (x0 + k as f32 * (SW + SW_GAP)) * s
}

fn tool_x(m: &Menu, n: usize, k: usize) -> f32 {
    let s = m.s;
    let total = n as f32 * TOOL + (n.saturating_sub(1)) as f32 * 4.0;
    let x0 = (m.w as f32 / s - total) / 2.0;
    (x0 + k as f32 * (TOOL + 4.0)) * s
}

/// What's at (`x`, `y`) in the menu's window.
fn target_at(m: &Menu, x: i32, y: i32) -> Target {
    let (fx, fy) = ((x - m.margin) as f32, (y - m.margin) as f32);
    if fx < 0.0 || fy < 0.0 || fx >= m.w as f32 || fy >= m.h as f32 {
        return Target::None;
    }
    let s = m.s;
    for (i, (ry, rh)) in layout_rows(m).into_iter().enumerate() {
        if fy < ry || fy >= ry + rh {
            continue;
        }
        return match &m.items[i] {
            Item::Action { disabled: false, .. } | Item::Submenu { disabled: false, .. } => Target::Row(i),
            Item::Action { .. } | Item::Submenu { .. } => Target::None,
            Item::Swatches { .. } => (0..theme::ORDER.len())
                .find(|&k| {
                    let x0 = swatch_x(m, k) - 3.0 * s;
                    fx >= x0 && fx < x0 + (SW + 6.0) * s
                })
                .map(|k| Target::Swatch(i, k))
                .unwrap_or(Target::None),
            Item::Tools(t) => (0..t.len())
                .find(|&k| {
                    let x0 = tool_x(m, t.len(), k);
                    fx >= x0 && fx < x0 + TOOL * s
                })
                .map(|k| Target::Tool(i, k))
                .unwrap_or(Target::None),
            Item::Separator => Target::None,
        };
    }
    Target::None
}

fn command_of(m: &Menu, t: Target) -> Option<u32> {
    match t {
        Target::Row(i) => match &m.items[i] {
            Item::Action { id, .. } => Some(*id),
            _ => None,
        },
        Target::Swatch(i, k) => match &m.items[i] {
            Item::Swatches { base, .. } => Some(base + theme::ORDER[k] as u32),
            _ => None,
        },
        Target::Tool(i, k) => match &m.items[i] {
            Item::Tools(t) => t.get(k).map(|t| t.id),
            _ => None,
        },
        Target::None => None,
    }
}

/// The row `t` is, when it's a submenu row that can open.
fn opens(m: &Menu, t: Target) -> Option<usize> {
    match t {
        Target::Row(i) if matches!(m.items.get(i), Some(Item::Submenu { disabled: false, .. })) => Some(i),
        _ => None,
    }
}

/// `t` in menu `level` was picked.
fn choose(o: &mut Open, level: usize, t: Target) {
    if o.blink.is_some() {
        return;
    }
    if let Some(cmd) = o.menus.get(level).and_then(|m| command_of(m, t)) {
        o.blink = Some(Blink { cmd, t0: Instant::now(), level, at: t });
        anim::wake(Client::Menu);
    }
}

/// A swatch says which colour it is, a tool what it does and its shortcut: under the
/// pointer, or (`by_key`) when the keyboard reaches it.
fn tip(m: &Menu, t: Target, by_key: bool) {
    use crate::win::ui::tooltip;
    let s = m.s;
    let row = |i: usize| layout_rows(m).get(i).copied();
    let (r, label, key) = match (t, item_of(m, t)) {
        (Target::Swatch(i, k), Some(Item::Swatches { .. })) => {
            let Some((ry, rh)) = row(i) else { return };
            let d = SW * s;
            (rect(m.margin + swatch_x(m, k) as i32, m.margin + (ry + (rh - d) / 2.0) as i32, d as i32, d as i32), theme::color_name(theme::ORDER[k]), "")
        }
        (Target::Tool(i, k), Some(Item::Tools(tools))) if tools.get(k).is_some_and(|t| !t.label.is_empty()) => {
            let Some((ry, rh)) = row(i) else { return };
            let d = TOOL * s;
            (rect(m.margin + tool_x(m, tools.len(), k) as i32, m.margin + (ry + (rh - d) / 2.0) as i32, d as i32, d as i32), tools[k].label, tools[k].key.as_str())
        }
        _ => {
            tooltip::leave(m.hwnd);
            return;
        }
    };
    if by_key {
        tooltip::focus(m.hwnd, r, label, key);
    } else {
        tooltip::hover(m.hwnd, r, label, key);
    }
}

/// The item a target sits in.
fn item_of(m: &Menu, t: Target) -> Option<&Item> {
    match t {
        Target::Row(i) | Target::Swatch(i, _) | Target::Tool(i, _) => m.items.get(i),
        Target::None => None,
    }
}

/// Fade out without a choice. The mouse is released straight away and the windows turn
/// click-through, so the pointer is already back with whatever lies beneath.
fn dismiss(o: &mut Open) {
    crate::win::ui::tooltip::mute();
    if o.result.is_some() || o.blink.is_some() || o.closing {
        return;
    }
    o.closing = true;
    o.pending = None;
    let root = o.menus[0].hwnd;
    for m in &mut o.menus {
        m.hot = Target::None;
        // It starts going at once, from the frame after the click.
        m.fade.retarget(0.0, (1.0, 0.12));
        m.fade.launch(0.0);
        unsafe {
            let ex = GetWindowLongW(m.hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(m.hwnd, GWL_EXSTYLE, (ex | WS_EX_TRANSPARENT) as i32);
        }
    }
    unsafe {
        KillTimer(root, T_SUB);
        if GetCapture() == root {
            ReleaseCapture();
        }
    }
    anim::wake(Client::Menu);
}

/// Redraw menu `level` as it stands.
fn draw(o: &mut Open, level: usize) {
    let blink = o.blink.filter(|b| b.level == level).map(|b| (b.t0, b.at));
    let open = o.menus.get(level + 1).map(|c| c.parent_row);
    let inner = o.keys > level;
    if let Some(m) = o.menus.get_mut(level) {
        render(m, blink, open, inner);
    }
}

fn draw_all(o: &mut Open) {
    for i in 0..o.menus.len() {
        draw(o, i);
    }
}

/// Draw menu `m`, with `blink` (when, and where) if a choice in it is blinking, and `open`
/// the row whose submenu is showing beside it; `inner` when the keys have gone into that
/// submenu.
fn render(m: &mut Menu, blink: Option<(Instant, Target)>, open: Option<usize>, inner: bool) {
    let dark = app::dark();
    let (surface, edge, text, sub, hi) = pal(dark);
    let s = m.s;
    let (cw, ch) = (m.w + 2 * m.margin, m.h + 2 * m.margin);
    if m.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
        m.canvas = Some(Canvas::new(cw, ch));
    }
    let mut c = m.canvas.take().unwrap();
    c.clear(0);
    let a = clamp01(m.fade.value);
    // Settle: a few pixels of travel toward the content, anchored at the side it opens from.
    // In whole pixels, so the card and its text move together: the text is put down on
    // whole pixels, and with the card between them the two parted by a fraction and met
    // again at the end, the text seeming to change its weight as the last pixel went.
    let settle = if anim::reduced() { 0.0 } else { ((1.0 - m.fade.value) * 6.0 * s).round() };
    let ox = m.margin as f32 + settle * m.from.0;
    let oy = m.margin as f32 + settle * m.from.1;
    let card = Shape::new(ox, oy, m.w as f32, m.h as f32, 12.0 * s);
    c.card(&card, surface, 1.0, edge, if dark { 0.14 } else { 0.10 }, 1.6, s);
    c.begin_batch();
    let blink_on = blink.map(|(t0, _)| (t0.elapsed().as_millis() / 55) % 2 == 1).unwrap_or(true);
    let blinking = |t: Target| blink.is_some_and(|(_, b)| b == t);
    let rows = layout_rows(m);
    // Interior is opaque: draw ClearType text, then restore alpha just there.
    for (i, it) in m.items.iter().enumerate() {
        let (ry, rh) = rows[i];
        let y = oy + ry;
        match it {
            Item::Separator => {
                c.rrect(ox + 12.0 * s, y + rh / 2.0, m.w as f32 - 24.0 * s, 1.0, 0.0, edge, if dark { 0.12 } else { 0.10 });
            }
            Item::Action { .. } | Item::Submenu { .. } => {
                let (label, key, icon, checked, danger, disabled, chevron) = match it {
                    Item::Action { label, key, icon, checked, danger, disabled, .. } => (label, key.as_str(), *icon, *checked, *danger, *disabled, false),
                    Item::Submenu { label, icon, disabled, .. } => (label, "", *icon, false, false, *disabled, true),
                    _ => continue,
                };
                let hot = m.hot == Target::Row(i) || blinking(Target::Row(i));
                // A row whose submenu is showing stays marked, softly once the keys or the
                // pointer have gone on into the submenu, so it's plain where it came from.
                let opened = open == Some(i);
                let lit = hot && blink_on && !(opened && inner);
                // Destructive rows stay red all the way: red label and icon, red highlight.
                let red = if dark { 0xFF453A } else { 0xE5352B };
                let hl_rgb = if danger { red } else { hi };
                let hl = Shape::new(ox + PAD * s, y, m.w as f32 - 2.0 * PAD * s, rh, 6.0 * s);
                if lit {
                    c.shape(&hl, hl_rgb, 1.0, 1.0, 0.0);
                } else if opened {
                    c.shape(&hl, edge, if dark { 0.12 } else { 0.08 }, 1.0, 0.0);
                }
                let off = mix(sub, surface, 0.45);
                let ink = if lit { 0xFFFFFF } else if disabled { off } else if danger { red } else { text };
                let ink2 = if lit { mix(hl_rgb, 0xFFFFFF, 0.8) } else if disabled { off } else if danger { red } else { sub };
                let mid = (y + rh / 2.0) as i32;
                if checked {
                    c.text_ct("\u{E73E}", &m.fonts.icon, rect((ox + 12.0 * s) as i32, y as i32, (20.0 * s) as i32, rh as i32), None, ink, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                } else if !icon.is_empty() {
                    c.text_ct(icon, &m.fonts.icon, rect((ox + 12.0 * s) as i32, y as i32, (20.0 * s) as i32, rh as i32), None, ink2, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                }
                let lx = (ox + if m.has_icons { 40.0 } else { 16.0 } * s) as i32;
                let base = mid + m.fonts.label.cap / 2;
                c.line_ct(label, &m.fonts.label, lx, base, m.w - (lx - ox as i32) - (16.0 * s) as i32, None, ink, DT_LEFT);
                // The chevron sits where a shortcut would, in the shortcut's colour.
                let right = (ox + m.w as f32 - 16.0 * s) as i32;
                if chevron {
                    // Pointing the way its submenu opens: left from a menu that opened
                    // leftwards (a note's ⋯ menu, at its top right).
                    let glyph = if m.leftward { CHEVRON_LEFT } else { CHEVRON };
                    c.text_ct(glyph, &m.fonts.chevron, rect(right - (20.0 * s) as i32, y as i32, (20.0 * s) as i32, rh as i32), None, ink2, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                } else if !key.is_empty() {
                    let kx = right - (160.0 * s) as i32;
                    c.line_ct(key, &m.fonts.key, kx, mid + m.fonts.key.cap / 2, (160.0 * s) as i32, None, ink2, DT_RIGHT);
                }
            }
            Item::Swatches { current, .. } => {
                for k in 0..theme::ORDER.len() {
                    let ci = theme::ORDER[k];
                    let hot = m.hot == Target::Swatch(i, k) || blinking(Target::Swatch(i, k));
                    let grow = if hot { 2.5 * s } else { 0.0 };
                    let d = SW * s + 2.0 * grow;
                    let sx = ox + swatch_x(m, k) - grow;
                    let sy = y + (rh - d) / 2.0;
                    let sh = Shape::new(sx, sy, d, d, d / 2.0);
                    c.shape(&sh, theme::color(ci).accent, 1.0, 1.0, 0.0);
                    c.shape(&sh, 0x000000, 0.14, 1.0, 1.0);
                    if *current == Some(ci) {
                        let ring = Shape::new(sx - 3.5 * s, sy - 3.5 * s, d + 7.0 * s, d + 7.0 * s, d / 2.0 + 3.5 * s);
                        c.shape(&ring, text, 0.6, 1.0, 1.5 * s);
                    }
                }
            }
            Item::Tools(tools) => {
                for (k, t) in tools.iter().enumerate() {
                    let tx = ox + tool_x(m, tools.len(), k);
                    let hot = m.hot == Target::Tool(i, k) || blinking(Target::Tool(i, k));
                    let r = Shape::new(tx, y + (rh - TOOL * s) / 2.0, TOOL * s, TOOL * s, 7.0 * s);
                    if hot && blink_on {
                        c.shape(&r, hi, 1.0, 1.0, 0.0);
                    } else {
                        c.shape(&r, edge, if dark { 0.08 } else { 0.05 }, 1.0, 0.0);
                    }
                    let f = match t.style {
                        ToolStyle::Bold => &m.fonts.tools[0],
                        ToolStyle::Italic => &m.fonts.tools[1],
                        ToolStyle::Underline => &m.fonts.tools[2],
                        ToolStyle::Strike => &m.fonts.tools[3],
                        ToolStyle::Plain => &m.fonts.tools[4],
                        ToolStyle::Icon => &m.fonts.icon,
                    };
                    let ink = if hot && blink_on { 0xFFFFFF } else { text };
                    c.text_ct(t.glyph, f, rect(r.x as i32, r.y as i32, r.w as i32, r.h as i32), None, ink, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                }
            }
        }
    }
    c.flush();
    // The card interior is opaque.
    c.make_opaque_rect(rect((ox + 4.0 * s) as i32, (oy + 4.0 * s) as i32, m.w - (8.0 * s) as i32, m.h - (8.0 * s) as i32));
    if a < 0.999 {
        c.scale_alpha(a);
    }
    c.present(m.hwnd, m.origin.0, m.origin.1);
    m.canvas = Some(c);
}

/// Where a submenu of `size` goes, as its card's top-left on screen: beside the parent
/// menu's card (`card`: its left and right edges), overlapping it by `overlap`, its first
/// row level with the parent's row at `row_top` (`inset` being the menu's inner padding).
/// It opens to the right, or (`leftward`) to the left, turning the other way when there's
/// no room on that side, and is lifted to stay within `work`. True when it went left.
fn beside(card: (i32, i32), row_top: i32, size: (i32, i32), inset: i32, overlap: i32, leftward: bool, work: RECT) -> (i32, i32, bool) {
    let (w, h) = size;
    let (right, left) = (card.1 - overlap, card.0 + overlap - w);
    let (fits_right, fits_left) = (right + w <= work.right, left >= work.left);
    let (x, went_left) = match (leftward, fits_left, fits_right) {
        (true, true, _) | (false, true, false) => (left, true),
        (_, _, true) => (right, false),
        // No room either side (a menu as wide as the screen): over the parent, on screen.
        _ => ((work.right - w).max(work.left), false),
    };
    let y = (row_top - inset).min(work.bottom - h).max(work.top);
    (x, y, went_left)
}

/// Close the submenus open beyond menu `level`; the keys come back to it if they were in one.
fn close_after(o: &mut Open, level: usize) {
    while o.menus.len() > level + 1 {
        if let Some(m) = o.menus.pop() {
            crate::win::ui::tooltip::leave(m.hwnd);
            announce(m.hwnd, false);
            unsafe { DestroyWindow(m.hwnd) };
        }
    }
    o.keys = o.keys.min(level);
    if o.pending.is_some_and(|(l, _)| l > level) {
        o.pending = None;
    }
}

/// Open row `row`'s submenu beside menu `level`, closing any other open there, and with
/// `keys` take the keyboard into it, at its first row.
fn open_sub(o: &mut Open, level: usize, row: usize, keys: bool) {
    o.pending = None;
    unsafe { KillTimer(o.menus[0].hwnd, T_SUB) };
    if o.menus.get(level + 1).is_none_or(|c| c.parent_row != row) {
        close_after(o, level);
        let Some(Item::Submenu { items, disabled: false, .. }) = o.menus[level].items.get(row) else { return };
        let items = items();
        if items.is_empty() {
            return;
        }
        let child = new_sub(&o.menus[level], row, items);
        let hwnd = child.hwnd;
        o.menus.push(child);
        o.menus[level].hot = Target::Row(row);
        draw(o, level + 1);
        unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        announce(hwnd, true);
        anim::wake(Client::Menu);
    }
    if keys {
        let c = &o.menus[level + 1];
        let rows: Vec<Reach> = c.items.iter().map(Reach::of).collect();
        let t = next_focus(&rows, None, Arrow::Down).map_or(Target::None, |f| target_of(c, f));
        o.keys = level + 1;
        o.menus[level + 1].hot = t;
        tip(&o.menus[level + 1], t, true);
    }
    draw(o, level);
    draw(o, level + 1);
}

/// The submenu of `items` for row `row` of `parent`: the same look and scale, placed beside
/// the row on the parent's monitor.
fn new_sub(parent: &Menu, row: usize, items: Vec<Item>) -> Menu {
    let s = parent.s;
    let fonts = fonts(s);
    let (w, h) = measure(&items, &fonts, s);
    let (work, _) = crate::win::ui::screen::of(parent.hwnd);
    let work = crate::win::ui::screen::inset(work, parent.margin);
    let left = parent.origin.0 + parent.margin;
    let row_top = parent.origin.1 + parent.margin + layout_rows(parent).get(row).map_or(0.0, |r| r.0) as i32;
    let (x, y, flipped) = beside((left, left + parent.w), row_top, (w, h), (PAD * s).round() as i32, (OVERLAP * s).round() as i32, parent.leftward, work);
    let margin = parent.margin;
    let hwnd = create(x - margin, y - margin, w + 2 * margin, h + 2 * margin, true);
    let mut fade = Spring::fade(0.0, (1.0, 0.12));
    fade.to(1.0);
    Menu {
        hwnd,
        items,
        s,
        fonts,
        w,
        h,
        margin,
        origin: (x - margin, y - margin),
        from: (if flipped { 1.0 } else { -1.0 }, 0.0),
        hot: Target::None,
        fade,
        canvas: None,
        has_icons: true,
        leftward: flipped,
        parent_row: row,
    }
}

/// What the pointer resting on a row does to the submenus, once it has rested a moment.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Hover {
    /// Leave what's pending be.
    Keep,
    /// Nothing is to happen.
    Cancel,
    /// Open this row's submenu.
    Open(usize),
    /// Close the submenu that's open.
    Close,
}

/// What resting on `row` (a row that can be picked, and whether it opens a submenu; None
/// for a gap, a separator or a greyed-out row) does, with `open` the row whose submenu is
/// showing. On the open row or in the gaps the submenu stays; on another row it closes,
/// so a pointer cutting across on its way into the submenu has a moment to get there.
fn hover_plan(row: Option<(usize, bool)>, open: Option<usize>) -> Hover {
    match (row, open) {
        (Some((r, true)), Some(o)) if r == o => Hover::Cancel,
        (Some((r, true)), _) => Hover::Open(r),
        (Some(_), Some(_)) => Hover::Close,
        (None, Some(_)) => Hover::Keep,
        (_, None) => Hover::Cancel,
    }
}

/// Act on a hover plan for menu `level`: a new open or close starts the timer over.
fn plan(o: &mut Open, level: usize, h: Hover) {
    let root = o.menus[0].hwnd;
    match h {
        Hover::Keep => {}
        Hover::Cancel => {
            if o.pending.take().is_some() {
                unsafe { KillTimer(root, T_SUB) };
            }
        }
        Hover::Open(_) | Hover::Close => {
            if o.pending != Some((level, h)) {
                o.pending = Some((level, h));
                let ms = if h == Hover::Close { CLOSE_MS } else { OPEN_MS };
                unsafe { SetTimer(root, T_SUB, ms, None) };
            }
        }
    }
}

/// The pointer moved to screen point `p`: highlight what's under it, and open or close
/// submenus after a moment. Returns `p` when it is outside every menu.
fn moved(o: &mut Open, p: POINT) -> Option<POINT> {
    // A move that isn't one (sent when a window appears under a still pointer, such as a
    // tooltip or a submenu) mustn't take the keyboard's highlight.
    if o.closing || o.pointer == (p.x, p.y) {
        return None;
    }
    o.pointer = (p.x, p.y);
    if o.blink.is_some() {
        return None;
    }
    let over = (0..o.menus.len()).rev().find(|&i| o.menus[i].holds(p));
    let mut changed = false;
    for i in 0..o.menus.len() {
        // A menu with a submenu open keeps its row marked, unless the pointer is on another.
        let open = o.menus.get(i + 1).map_or(Target::None, |c| Target::Row(c.parent_row));
        let t = match o.menus[i].target(p) {
            Target::None => open,
            t if over == Some(i) => t,
            _ => open,
        };
        if t != o.menus[i].hot {
            o.menus[i].hot = t;
            tip(&o.menus[i], t, false);
            changed = true;
        }
    }
    match over {
        Some(v) => {
            if o.keys != v {
                o.keys = v;
                changed = true;
            }
            let m = &o.menus[v];
            let row = match m.target(p) {
                t @ Target::Row(r) => Some((r, opens(m, t).is_some())),
                Target::Swatch(r, _) | Target::Tool(r, _) => Some((r, false)),
                Target::None => None,
            };
            let open = o.menus.get(v + 1).map(|c| c.parent_row);
            plan(o, v, hover_plan(row, open));
        }
        None => plan(o, 0, Hover::Cancel),
    }
    if changed {
        draw_all(o);
    }
    over.is_none().then_some(p)
}

/// What the keys can reach in a row: nothing, the row itself (a submenu row being one that
/// opens), or this many targets along it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Reach {
    Nothing,
    Row,
    Sub,
    Along(usize),
}

impl Reach {
    fn of(it: &Item) -> Reach {
        match it {
            Item::Action { disabled: false, .. } => Reach::Row,
            Item::Submenu { disabled: false, .. } => Reach::Sub,
            Item::Swatches { .. } => Reach::Along(theme::ORDER.len()),
            Item::Tools(t) if !t.is_empty() => Reach::Along(t.len()),
            _ => Reach::Nothing,
        }
    }

    fn width(self) -> usize {
        match self {
            Reach::Nothing => 0,
            Reach::Row | Reach::Sub => 1,
            Reach::Along(n) => n,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Arrow {
    Up,
    Down,
    Left,
    Right,
}

/// Where an arrow key takes the keyboard focus, as (row, column). Up and Down go to the
/// next row with something to reach, wrapping round, and keep the column if that row has
/// it (else start at its first). Left and Right go along a row, wrapping too. With no
/// focus yet, Down starts at the top and Up at the bottom.
fn next_focus(rows: &[Reach], at: Option<(usize, usize)>, key: Arrow) -> Option<(usize, usize)> {
    let n = rows.len();
    match key {
        Arrow::Left | Arrow::Right => {
            let (i, k) = at?;
            let w = rows.get(i)?.width();
            if w < 2 {
                return at;
            }
            Some((i, if key == Arrow::Right { (k + 1) % w } else { (k + w - 1) % w }))
        }
        Arrow::Up | Arrow::Down => {
            let down = key == Arrow::Down;
            // From nowhere, step as if from just past the far end.
            let from = at.map_or(if down { n.checked_sub(1)? } else { 0 }, |a| a.0);
            (1..=n).map(|d| if down { (from + d) % n } else { (from + n - d) % n }).find(|&i| rows[i].width() > 0).map(|i| {
                let k = at.map_or(0, |a| a.1);
                (i, if k < rows[i].width() { k } else { 0 })
            })
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Key {
    Arrow(Arrow),
    /// Enter or Space.
    Enter,
    Esc,
}

/// What a key does in the menu that has the keys.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    /// The focus moves here (None: nowhere).
    Focus(Option<(usize, usize)>),
    /// This row's submenu opens, if it isn't open already, and the keys go into it.
    Into(usize),
    /// The submenu open beside this menu closes; the keys stay here.
    CloseSub,
    /// This submenu closes and the keys go back to the row that opened it.
    Back,
    Choose,
    Dismiss,
}

/// What `key` does in a menu of `rows` with the focus `at`: `nested` when the menu is a
/// submenu, `sub_open` when one of its own is showing (the pointer opened it). Right and
/// Enter open a submenu row, Left (but along a colour or tool row) and Esc step back out
/// of a submenu, and Esc closes the innermost menu first.
fn key_step(rows: &[Reach], at: Option<(usize, usize)>, nested: bool, sub_open: bool, key: Key) -> Step {
    let reach = at.and_then(|(i, _)| rows.get(i).copied());
    if let Some((i, _)) = at
        && reach == Some(Reach::Sub)
        && matches!(key, Key::Arrow(Arrow::Right) | Key::Enter)
    {
        return Step::Into(i);
    }
    match key {
        Key::Arrow(Arrow::Left) if nested && !matches!(reach, Some(Reach::Along(_))) => Step::Back,
        Key::Arrow(a) => Step::Focus(next_focus(rows, at, a)),
        Key::Enter => Step::Choose,
        Key::Esc if sub_open => Step::CloseSub,
        Key::Esc if nested => Step::Back,
        Key::Esc => Step::Dismiss,
    }
}

fn focus_of(t: Target) -> Option<(usize, usize)> {
    match t {
        Target::Row(i) => Some((i, 0)),
        Target::Swatch(i, k) | Target::Tool(i, k) => Some((i, k)),
        Target::None => None,
    }
}

fn target_of(m: &Menu, (i, k): (usize, usize)) -> Target {
    match m.items.get(i) {
        Some(Item::Swatches { .. }) => Target::Swatch(i, k),
        Some(Item::Tools(_)) => Target::Tool(i, k),
        Some(_) => Target::Row(i),
        None => Target::None,
    }
}

/// The keyboard moved to `t` in menu `level`: highlight it as the pointer would, tooltip
/// included. A submenu open from another row closes.
fn focus(o: &mut Open, level: usize, t: Target) {
    if o.menus.get(level + 1).is_some_and(|c| Target::Row(c.parent_row) != t) {
        close_after(o, level);
    }
    let was = o.keys;
    o.keys = level;
    if t != o.menus[level].hot || was != level {
        o.menus[level].hot = t;
        draw_all(o);
        tip(&o.menus[level], t, true);
    }
}

/// A key in the menu that has the keys.
fn on_key(o: &mut Open, vk: u16) {
    if o.closing || o.blink.is_some() {
        return;
    }
    let level = o.keys.min(o.menus.len() - 1);
    let key = match vk {
        VK_UP => Key::Arrow(Arrow::Up),
        VK_DOWN => Key::Arrow(Arrow::Down),
        VK_LEFT => Key::Arrow(Arrow::Left),
        VK_RIGHT => Key::Arrow(Arrow::Right),
        VK_RETURN | VK_SPACE => Key::Enter,
        VK_ESCAPE => Key::Esc,
        // Letters come as characters, in the keyboard's own layout (`on_char`).
        _ => return,
    };
    // The keys take over from the pointer: nothing it set going happens now.
    plan(o, level, Hover::Cancel);
    let m = &o.menus[level];
    let rows: Vec<Reach> = m.items.iter().map(Reach::of).collect();
    match key_step(&rows, focus_of(m.hot), level > 0, o.menus.len() > level + 1, key) {
        Step::Focus(Some(f)) => {
            let t = target_of(m, f);
            focus(o, level, t);
        }
        Step::Focus(None) => {}
        Step::Into(row) => open_sub(o, level, row, true),
        Step::CloseSub => {
            close_after(o, level);
            draw(o, level);
        }
        Step::Back => {
            let row = o.menus[level].parent_row;
            close_after(o, level - 1);
            focus(o, level - 1, Target::Row(row));
            // That row had the focus all along, so `focus` saw no change: drawn again, it
            // turns from the soft "came from here" mark back to the full highlight.
            draw(o, level - 1);
        }
        Step::Choose => {
            crate::win::ui::tooltip::mute();
            let t = o.menus[level].hot;
            choose(o, level, t);
        }
        Step::Dismiss => dismiss(o),
    }
}

/// A typed character: the focus jumps to the next row that can be picked whose label
/// starts with it, from the row it's on, so typing it again goes on to the next.
fn on_char(o: &mut Open, ch: char) {
    if o.closing || o.blink.is_some() {
        return;
    }
    let level = o.keys.min(o.menus.len() - 1);
    let m = &o.menus[level];
    let rows: Vec<(usize, &str)> = m
        .items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match it {
            Item::Action { label, disabled: false, .. } | Item::Submenu { label, disabled: false, .. } => Some((i, label.as_str())),
            _ => None,
        })
        .collect();
    let at = match m.hot {
        Target::Row(i) => Some(i),
        _ => None,
    };
    if let Some(i) = crate::win::ui::focus::by_letter(&rows, at, ch) {
        plan(o, level, Hover::Cancel);
        focus(o, level, Target::Row(i));
    }
}

/// Window `h`'s point (`x`, `y`) on screen (mouse messages come to the first menu, which
/// holds the mouse, in its own coordinates).
fn on_screen(o: &Open, h: HWND, x: i32, y: i32) -> POINT {
    let (ox, oy) = o.menus.iter().find(|m| m.hwnd == h).map_or((0, 0), |m| m.origin);
    POINT { x: ox + x, y: oy + y }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                let (x, y) = (x_of(lp), y_of(lp));
                let outside = with(|o| {
                    let p = on_screen(o, h, x, y);
                    moved(o, p)
                })
                .flatten();
                // Outside the menus: ask the caller whether the pointer has moved on.
                if let Some(p) = outside
                    && LEAVE.with(|l| l.borrow().as_ref().is_some_and(|f| f(p))) {
                        with(dismiss);
                    }
                return 0;
            }
            WM_MOUSELEAVE => return 0,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|o| {
                    let p = on_screen(o, h, x, y);
                    if !o.closing && !o.menus.iter().any(|m| m.holds(p)) {
                        dismiss(o); // click outside (captured)
                    }
                });
                return 0;
            }
            WM_LBUTTONUP | WM_RBUTTONUP => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|o| {
                    if o.closing {
                        return;
                    }
                    let p = on_screen(o, h, x, y);
                    crate::win::ui::tooltip::mute();
                    let Some(level) = (0..o.menus.len()).rev().find(|&i| o.menus[i].holds(p)) else { return };
                    let t = o.menus[level].target(p);
                    // A click on a submenu row opens it there and then, without the wait.
                    match opens(&o.menus[level], t) {
                        Some(row) => {
                            o.keys = level;
                            open_sub(o, level, row, false);
                        }
                        None => choose(o, level, t),
                    }
                });
                return 0;
            }
            WM_KEYDOWN => {
                let vk = wp as u16;
                with(|o| on_key(o, vk));
                return 0;
            }
            WM_CHAR => {
                if let Some(ch) = char::from_u32(wp as u32) {
                    with(|o| on_char(o, ch));
                }
                return 0;
            }
            WM_TIMER if wp == T_SUB => {
                KillTimer(h, T_SUB);
                with(|o| {
                    if o.closing || o.blink.is_some() {
                        return;
                    }
                    match o.pending.take() {
                        Some((level, Hover::Open(row))) => open_sub(o, level, row, false),
                        Some((level, _)) => {
                            close_after(o, level);
                            draw(o, level);
                        }
                        None => {}
                    }
                });
                return 0;
            }
            WM_ACTIVATE => {
                crate::win::crash::trace(&format!("menu WM_ACTIVATE wp={wp:#x} other={lp:#x}"));
                if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                    with(dismiss);
                }
                return 0;
            }
            WM_CAPTURECHANGED => {
                crate::win::crash::trace(&format!("menu WM_CAPTURECHANGED new={lp:#x}"));
                // Another window took the mouse (e.g. a system dialog): close.
                if lp as HWND != h {
                    with(dismiss);
                }
                return 0;
            }
            // Only the first menu is ever activated; a submenu leaves the keys with it.
            WM_MOUSEACTIVATE => {
                let first = with(|o| o.menus[0].hwnd == h).unwrap_or(true);
                return (if first { MA_ACTIVATE } else { MA_NOACTIVATE }) as LRESULT;
            }
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Reach::{Along, Nothing, Row, Sub};

    // The edit menu's shape: two tool rows, a separator, actions.
    const EDIT: [Reach; 7] = [Along(7), Along(6), Nothing, Row, Row, Nothing, Row];

    #[test]
    fn shortcut_text_reads_on_the_menu() {
        for dark in [false, true] {
            let (surface, _, text, sub, _) = pal(dark);
            assert!(crate::common::theme::contrast(sub, surface) >= 4.5, "dark: {dark}");
            assert!(crate::common::theme::contrast(text, surface) >= 7.0);
        }
    }

    #[test]
    fn up_and_down_reach_every_row() {
        let rows = [Row, Nothing, Along(12), Nothing, Along(7), Row];
        assert_eq!(next_focus(&rows, None, Arrow::Down), Some((0, 0)));
        assert_eq!(next_focus(&rows, Some((0, 0)), Arrow::Down), Some((2, 0)));
        assert_eq!(next_focus(&rows, Some((2, 0)), Arrow::Down), Some((4, 0)));
        assert_eq!(next_focus(&rows, Some((4, 3)), Arrow::Down), Some((5, 0)));
        assert_eq!(next_focus(&rows, Some((5, 0)), Arrow::Down), Some((0, 0)), "wraps to the top");
        assert_eq!(next_focus(&rows, None, Arrow::Up), Some((5, 0)));
        assert_eq!(next_focus(&rows, Some((0, 0)), Arrow::Up), Some((5, 0)), "wraps to the bottom");
    }

    #[test]
    fn rows_keep_the_column_when_they_can() {
        assert_eq!(next_focus(&EDIT, Some((0, 4)), Arrow::Down), Some((1, 4)));
        assert_eq!(next_focus(&EDIT, Some((1, 4)), Arrow::Up), Some((0, 4)));
        // The second row is one shorter: past its end, start at its first.
        assert_eq!(next_focus(&EDIT, Some((0, 6)), Arrow::Down), Some((1, 0)));
        // Into an action row and back out: the column starts over.
        assert_eq!(next_focus(&EDIT, Some((1, 5)), Arrow::Down), Some((3, 0)));
        assert_eq!(next_focus(&EDIT, Some((3, 0)), Arrow::Up), Some((1, 0)));
    }

    #[test]
    fn left_and_right_go_along_a_row() {
        assert_eq!(next_focus(&EDIT, Some((0, 0)), Arrow::Right), Some((0, 1)));
        assert_eq!(next_focus(&EDIT, Some((0, 6)), Arrow::Right), Some((0, 0)), "wraps");
        assert_eq!(next_focus(&EDIT, Some((1, 0)), Arrow::Left), Some((1, 5)), "wraps");
        // Nothing to the side of an action row, or of no focus at all.
        assert_eq!(next_focus(&EDIT, Some((3, 0)), Arrow::Right), Some((3, 0)));
        assert_eq!(next_focus(&EDIT, None, Arrow::Left), None);
    }

    #[test]
    fn disabled_rows_are_skipped() {
        let items = [
            action(1, "Cut", "", "").enabled(false),
            action(2, "Copy", "", ""),
            Item::Separator,
            action(3, "Paste", "", "").enabled(false),
            Item::Tools(Vec::new()),
            submenu("More", "", Vec::new).enabled(false),
        ];
        let rows: Vec<Reach> = items.iter().map(Reach::of).collect();
        assert_eq!(rows, [Nothing, Row, Nothing, Nothing, Nothing, Nothing]);
        assert_eq!(next_focus(&rows, None, Arrow::Down), Some((1, 0)));
        assert_eq!(next_focus(&rows, None, Arrow::Up), Some((1, 0)));
        assert_eq!(next_focus(&rows, Some((1, 0)), Arrow::Down), Some((1, 0)), "the only one stays");
        assert_eq!(next_focus(&[Nothing, Nothing], None, Arrow::Down), None);
        assert_eq!(next_focus(&[], None, Arrow::Up), None);
    }

    #[test]
    fn swatches_reach_every_colour() {
        let rows = [Reach::of(&Item::Swatches { base: 0, current: None })];
        assert_eq!(rows, [Along(theme::ORDER.len())]);
        let last = theme::ORDER.len() - 1;
        assert_eq!(next_focus(&rows, Some((0, 0)), Arrow::Left), Some((0, last)));
    }

    // The note menu's shape: colours, a separator, a submenu row among actions.
    const NOTE: [Reach; 6] = [Along(8), Nothing, Row, Sub, Row, Row];

    #[test]
    fn submenu_rows_are_reached_like_actions() {
        assert_eq!(Reach::of(&submenu("Previous Versions", "", Vec::new)), Sub);
        assert_eq!(next_focus(&NOTE, Some((2, 0)), Arrow::Down), Some((3, 0)));
        assert_eq!(next_focus(&NOTE, Some((3, 0)), Arrow::Down), Some((4, 0)));
        assert_eq!(next_focus(&NOTE, Some((4, 0)), Arrow::Up), Some((3, 0)));
        // Up and Down pass over it as over any row, and the other rows' keys are as before.
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, false, Key::Arrow(Arrow::Down)), Step::Focus(Some((4, 0))));
        assert_eq!(key_step(&NOTE, Some((0, 2)), false, false, Key::Arrow(Arrow::Right)), Step::Focus(Some((0, 3))));
        assert_eq!(key_step(&NOTE, Some((2, 0)), false, false, Key::Arrow(Arrow::Right)), Step::Focus(Some((2, 0))));
        assert_eq!(key_step(&NOTE, Some((2, 0)), false, false, Key::Enter), Step::Choose);
    }

    #[test]
    fn right_and_enter_go_into_a_submenu() {
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, false, Key::Arrow(Arrow::Right)), Step::Into(3));
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, false, Key::Enter), Step::Into(3));
        // Already open under the pointer: the keys go in all the same.
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, true, Key::Arrow(Arrow::Right)), Step::Into(3));
        // A greyed-out submenu row can't be reached, so can't be opened.
        let off = [Row, Nothing];
        assert_eq!(key_step(&off, None, false, false, Key::Arrow(Arrow::Down)), Step::Focus(Some((0, 0))));
        assert_eq!(key_step(&off, Some((1, 0)), false, false, Key::Arrow(Arrow::Right)), Step::Focus(Some((1, 0))));
    }

    #[test]
    fn left_and_esc_come_back_out() {
        let versions = [Row, Row, Row];
        assert_eq!(key_step(&versions, Some((1, 0)), true, false, Key::Arrow(Arrow::Left)), Step::Back);
        assert_eq!(key_step(&versions, Some((1, 0)), true, false, Key::Esc), Step::Back);
        // Nothing focused yet in it (only "No earlier versions", greyed out): still back.
        assert_eq!(key_step(&[Nothing], None, true, false, Key::Arrow(Arrow::Left)), Step::Back);
        assert_eq!(key_step(&[Nothing], None, true, false, Key::Arrow(Arrow::Down)), Step::Focus(None));
        // Along a colour row in a submenu, Left still goes along it.
        assert_eq!(key_step(&[Along(4)], Some((0, 1)), true, false, Key::Arrow(Arrow::Left)), Step::Focus(Some((0, 0))));
        // In the first menu, Left is just Left; Esc closes the submenu the pointer opened
        // before it closes the menu.
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, false, Key::Arrow(Arrow::Left)), Step::Focus(Some((3, 0))));
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, true, Key::Esc), Step::CloseSub);
        assert_eq!(key_step(&NOTE, Some((3, 0)), false, false, Key::Esc), Step::Dismiss);
        assert_eq!(key_step(&versions, Some((0, 0)), true, false, Key::Enter), Step::Choose);
    }

    #[test]
    fn resting_on_rows_opens_and_closes_submenus() {
        // On a submenu row: it opens, unless it's the one open.
        assert_eq!(hover_plan(Some((3, true)), None), Hover::Open(3));
        assert_eq!(hover_plan(Some((3, true)), Some(3)), Hover::Cancel);
        assert_eq!(hover_plan(Some((5, true)), Some(3)), Hover::Open(5));
        // On another row the open one closes; with none open there's nothing to do.
        assert_eq!(hover_plan(Some((4, false)), Some(3)), Hover::Close);
        assert_eq!(hover_plan(Some((4, false)), None), Hover::Cancel);
        // In a gap or on a separator, whatever was coming still comes.
        assert_eq!(hover_plan(None, Some(3)), Hover::Keep);
        assert_eq!(hover_plan(None, None), Hover::Cancel);
    }

    const WORK: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1040 };

    #[test]
    fn a_submenu_opens_to_the_right_level_with_its_row() {
        // Over the parent's edge by the overlap, and lifted by the padding so its first row
        // is level with the parent's.
        let (x, y, left) = beside((100, 340), 300, (240, 200), 5, 3, false, WORK);
        assert_eq!((x, y, left), (337, 295, false));
    }

    #[test]
    fn a_submenu_flips_left_at_the_screen_edge() {
        assert_eq!(beside((1600, 1840), 300, (240, 200), 5, 3, false, WORK), (1363, 295, true));
        // Exactly fitting on the right still goes right.
        assert_eq!(beside((1443, 1683), 300, (240, 200), 5, 3, false, WORK), (1680, 295, false));
        // No room either side: kept on screen, over the parent.
        assert_eq!(beside((0, 1800), 300, (240, 200), 5, 3, false, WORK), (1680, 295, false));
    }

    #[test]
    fn a_menu_that_opened_leftwards_opens_its_submenus_leftwards() {
        assert_eq!(beside((600, 840), 300, (240, 200), 5, 3, true, WORK), (363, 295, true));
        // Unless there's no room on the left.
        assert_eq!(beside((100, 340), 300, (240, 200), 5, 3, true, WORK), (337, 295, false));
    }

    #[test]
    fn a_submenu_is_lifted_above_the_bottom() {
        assert_eq!(beside((100, 340), 960, (240, 200), 5, 3, false, WORK).1, 840);
        // Taller than the screen: its top stays on it.
        assert_eq!(beside((100, 340), 960, (240, 1200), 5, 3, false, WORK).1, 0);
        // Near the top, it keeps level with the row, but not above the work area.
        let top = RECT { top: 40, ..WORK };
        assert_eq!(beside((100, 340), 42, (240, 200), 5, 3, false, top).1, 40);
    }
}
