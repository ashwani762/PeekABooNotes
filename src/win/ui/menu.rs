// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Popover menus: the app's own context and tray menus.
//!
//! Drawn in the same material and type as the rest of PeekABoo Notes (native Win32 menus can't be
//! styled to match). Opens with a 120 ms fade-and-settle from the pointer, highlights on
//! hover instantly, confirms a choice with a quick blink and fades out.
//! `show` runs a nested message loop, like `TrackPopupMenu`, and returns the command id
//! (0 if dismissed). Keyboard: arrows, Enter, Esc, and first-letter jumps.
//!
//! Dismissal fades too, and lets go of the mouse at once, so whatever is under the
//! pointer responds while the menu is still disappearing. A caller can also pass a
//! `leave` test: when the pointer moves somewhere it says the menu no longer belongs
//! (e.g. onto a different deck card), the menu fades away by itself.

use std::cell::RefCell;
use std::ptr::null_mut;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_CENTER, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FW_BOLD, FW_NORMAL, FW_SEMIBOLD, GetMonitorInfoW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
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
}

pub enum Item {
    /// A row of colour swatches; choosing one returns `base + colour index`.
    Swatches { base: u32, current: Option<u8> },
    /// A row of compact icon buttons (text formatting).
    Tools(Vec<Tool>),
    Action { id: u32, label: String, key: String, icon: &'static str, checked: bool, danger: bool },
    Separator,
}

pub fn action(id: u32, label: &str, key: &str, icon: &'static str) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon, checked: false, danger: false }
}

pub fn check(id: u32, label: &str, key: &str, checked: bool) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon: "", checked, danger: false }
}

pub fn danger(id: u32, label: &str, key: &str, icon: &'static str) -> Item {
    Item::Action { id, label: label.into(), key: key.into(), icon, checked: false, danger: true }
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
    tools: [Font; 5],
}

struct Menu {
    hwnd: HWND,
    items: Vec<Item>,
    s: f32,
    fonts: Fonts,
    w: i32,
    h: i32,
    margin: i32,
    origin: (i32, i32),
    flip_y: bool,
    hot: Target,
    fade: Spring,
    blink: Option<(u32, Instant, Target)>,
    result: Option<u32>,
    canvas: Option<Canvas>,
    has_icons: bool,
    /// Fading out after a dismissal; input is ignored and passes through.
    closing: bool,
}

/// Tells the menu whether the pointer, outside it, has moved somewhere it no longer belongs.
type LeaveTest = Box<dyn Fn(POINT) -> bool>;

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
    static LEAVE: RefCell<Option<LeaveTest>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Menu) -> R) -> Option<R> {
    MENU.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.as_mut().map(f)))
}

pub fn is_menu(h: HWND) -> bool {
    !h.is_null() && with(|m| m.hwnd == h).unwrap_or(false)
}

// Metrics (device-independent px).
const PAD: f32 = 5.0;
const ROW: f32 = 28.0;
const SEP: f32 = 9.0;
const STRIP: f32 = 40.0;
const SW: f32 = 16.0;
const SW_GAP: f32 = 7.0;
const TOOL: f32 = 30.0;

fn item_h(it: &Item) -> f32 {
    match it {
        Item::Swatches { .. } | Item::Tools(_) => STRIP,
        Item::Action { .. } => ROW,
        Item::Separator => SEP,
    }
}

fn pal(dark: bool) -> (u32, u32, u32, u32, u32) {
    // (surface, edge, text, secondary, highlight)
    if dark { (0x2C2C2E, 0xFFFFFF, 0xF2F2F7, 0x98989D, 0x0A84FF) } else { (0xF7F7F9, 0x000000, 0x1D1D1F, 0x86868B, 0x0A84FF) }
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
    let (s, work) = unsafe {
        let mon = MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (dx.max(96) as f32 / 96.0, mi.rcWork)
    };
    let f = |px: f32, w: u32, face: &str| Font::new(px * s, w, face);
    let icon_face = crate::win::ui::note::icon_face();
    let fonts = Fonts {
        label: f(13.0, FW_NORMAL, theme::FACE_TEXT),
        key: f(12.0, FW_NORMAL, theme::FACE_TEXT),
        icon: f(14.0, FW_NORMAL, icon_face),
        tools: [
            f(14.0, FW_BOLD, theme::FACE_TEXT),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, true, false, false),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, false, true, false),
            Font::new_styled(14.0 * s, FW_NORMAL, theme::FACE_TEXT, false, false, true),
            f(14.0, FW_SEMIBOLD, theme::FACE_TEXT),
        ],
    };
    // Size to content. Labels always leave room for an icon / checkmark column, so
    // checked and unchecked rows line up.
    let has_icons = true;
    let mut w = 0.0f32;
    let mut h = PAD * 2.0;
    for it in &items {
        h += item_h(it);
        let need = match it {
            Item::Action { label, key, .. } => {
                let lw = fonts.label.measure(label).0 as f32 / s;
                let kw = if key.is_empty() { 0.0 } else { fonts.key.measure(key).0 as f32 / s + 28.0 };
                40.0 + lw + kw + 18.0
            }
            Item::Swatches { .. } => 2.0 * 14.0 + theme::ORDER.len() as f32 * SW + (theme::ORDER.len() - 1) as f32 * SW_GAP,
            Item::Tools(t) => 2.0 * 14.0 + t.len() as f32 * TOOL + (t.len().saturating_sub(1)) as f32 * 4.0,
            Item::Separator => 0.0,
        };
        w = w.max(need);
    }
    let w = (w.max(220.0) * s).round() as i32;
    let h = (h * s).round() as i32;
    let margin = (22.0 * s) as i32;
    // Keep on screen: open down-right of the pointer, flipping when needed.
    let mut x = if align_right { at.x - w } else { at.x };
    let mut y = at.y;
    let flip_y = y + h > work.bottom;
    if x + w > work.right {
        x -= w;
    }
    if flip_y {
        y -= h;
    }
    x = x.clamp(work.left, (work.right - w).max(work.left));
    y = y.clamp(work.top, (work.bottom - h).max(work.top));
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            wide(CLASS).as_ptr(),
            wide("Menu").as_ptr(),
            WS_POPUP,
            x - margin,
            y - margin,
            w + 2 * margin,
            h + 2 * margin,
            null_mut(),
            null_mut(),
            app::hinst(),
            null_mut(),
        )
    };
    let mut fade = Spring::unit(0.0, (1.0, 0.12));
    fade.to(1.0);
    MENU.with(|m| {
        *m.borrow_mut() = Some(Menu {
            hwnd,
            items,
            s,
            fonts,
            w,
            h,
            margin,
            origin: (x - margin, y - margin),
            flip_y,
            hot: Target::None,
            fade,
            blink: None,
            result: None,
            canvas: None,
            has_icons,
            closing: false,
        })
    });
    with(render);
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetCapture(hwnd);
    }
    anim::wake(Client::Menu);
    // Nested loop until a choice is made or the menu is dismissed.
    loop {
        if with(|m| m.result.is_some()).unwrap_or(true) {
            break;
        }
        if !app::pump_once() {
            break;
        }
    }
    let result = MENU.with(|m| m.borrow_mut().take()).map(|m| {
        unsafe {
            ReleaseCapture();
            DestroyWindow(m.hwnd);
        }
        m.result.unwrap_or(0)
    });
    LEAVE.with(|l| l.borrow_mut().take());
    result.unwrap_or(0)
}

pub fn frame(dt: f32) -> bool {
    with(|m| {
        let moving = m.fade.step(dt);
        if m.closing {
            render(m);
            if m.fade.value < 0.03 || !moving {
                m.result = Some(0);
                return false;
            }
            return true;
        }
        // Confirmation blink, then close.
        if let Some((cmd, t0, _)) = m.blink {
            let ms = t0.elapsed().as_millis();
            if ms > 190 {
                m.result = Some(cmd);
                return false;
            }
            if ms > 110 && m.fade.target > 0.0 {
                m.fade.retarget(0.0, (1.0, 0.1));
            }
            render(m);
            return true;
        }
        render(m);
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
            Item::Action { .. } => Target::Row(i),
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

fn choose(m: &mut Menu, t: Target) {
    if m.blink.is_some() {
        return;
    }
    if let Some(cmd) = command_of(m, t) {
        m.blink = Some((cmd, Instant::now(), t));
        anim::wake(Client::Menu);
    }
}

/// Fade out without a choice. The mouse is released straight away and the window turns
/// click-through, so the pointer is already back with whatever lies beneath.
fn dismiss(m: &mut Menu) {
    if m.result.is_some() || m.blink.is_some() || m.closing {
        return;
    }
    m.closing = true;
    m.hot = Target::None;
    m.fade.retarget(0.0, (1.0, 0.12));
    unsafe {
        let ex = GetWindowLongW(m.hwnd, GWL_EXSTYLE) as u32;
        SetWindowLongW(m.hwnd, GWL_EXSTYLE, (ex | WS_EX_TRANSPARENT) as i32);
        if GetCapture() == m.hwnd {
            ReleaseCapture();
        }
    }
    anim::wake(Client::Menu);
}

fn render(m: &mut Menu) {
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
    // Settle: a few pixels of travel toward the content, anchored at the pointer side.
    let settle = (1.0 - m.fade.value) * 6.0 * s * if m.flip_y { 1.0 } else { -1.0 };
    let ox = m.margin as f32;
    let oy = m.margin as f32 + settle;
    let card = Shape::new(ox, oy, m.w as f32, m.h as f32, 12.0 * s);
    c.card(&card, surface, 1.0, edge, if dark { 0.14 } else { 0.10 }, 1.6, s);
    c.begin_batch();
    let blink_on = m.blink.map(|(_, t0, _)| (t0.elapsed().as_millis() / 55) % 2 == 1).unwrap_or(true);
    let rows = layout_rows(m);
    // Interior is opaque: draw ClearType text, then restore alpha just there.
    for (i, it) in m.items.iter().enumerate() {
        let (ry, rh) = rows[i];
        let y = oy + ry;
        match it {
            Item::Separator => {
                c.rrect(ox + 12.0 * s, y + rh / 2.0, m.w as f32 - 24.0 * s, 1.0, 0.0, edge, if dark { 0.12 } else { 0.10 });
            }
            Item::Action { label, key, icon, checked, danger, .. } => {
                let hot = m.hot == Target::Row(i) || m.blink.is_some_and(|(_, _, t)| t == Target::Row(i));
                let lit = hot && blink_on;
                // Destructive rows stay red all the way: red label and icon, red highlight.
                let red = if dark { 0xFF453A } else { 0xE5352B };
                let hl_rgb = if *danger { red } else { hi };
                if lit {
                    let hl = Shape::new(ox + PAD * s, y, m.w as f32 - 2.0 * PAD * s, rh, 6.0 * s);
                    c.shape(&hl, hl_rgb, 1.0, 1.0, 0.0);
                }
                let ink = if lit { 0xFFFFFF } else if *danger { red } else { text };
                let ink2 = if lit { mix(hl_rgb, 0xFFFFFF, 0.8) } else if *danger { red } else { sub };
                let mid = (y + rh / 2.0) as i32;
                if *checked {
                    c.text_ct("\u{E73E}", &m.fonts.icon, rect((ox + 12.0 * s) as i32, y as i32, (20.0 * s) as i32, rh as i32), None, ink, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                } else if !icon.is_empty() {
                    c.text_ct(icon, &m.fonts.icon, rect((ox + 12.0 * s) as i32, y as i32, (20.0 * s) as i32, rh as i32), None, ink2, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                }
                let lx = (ox + if m.has_icons { 40.0 } else { 16.0 } * s) as i32;
                let base = mid + m.fonts.label.cap / 2;
                c.line_ct(label, &m.fonts.label, lx, base, m.w - (lx - ox as i32) - (16.0 * s) as i32, None, ink, DT_LEFT);
                if !key.is_empty() {
                    let kx = (ox + m.w as f32 - 16.0 * s) as i32 - (160.0 * s) as i32;
                    c.line_ct(key, &m.fonts.key, kx, mid + m.fonts.key.cap / 2, (160.0 * s) as i32, None, ink2, DT_RIGHT);
                }
            }
            Item::Swatches { current, .. } => {
                for k in 0..theme::ORDER.len() {
                    let ci = theme::ORDER[k];
                    let hot = m.hot == Target::Swatch(i, k) || m.blink.is_some_and(|(_, _, t)| t == Target::Swatch(i, k));
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
                    let hot = m.hot == Target::Tool(i, k) || m.blink.is_some_and(|(_, _, b)| b == Target::Tool(i, k));
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

fn selectable(m: &Menu) -> Vec<usize> {
    m.items.iter().enumerate().filter(|(_, it)| matches!(it, Item::Action { .. })).map(|(i, _)| i).collect()
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                let (x, y) = (x_of(lp), y_of(lp));
                let outside = with(|m| {
                    if m.closing {
                        return None;
                    }
                    let t = target_at(m, x, y);
                    if t != m.hot && m.blink.is_none() {
                        m.hot = t;
                        render(m);
                    }
                    let inside = x >= m.margin && y >= m.margin && x < m.margin + m.w && y < m.margin + m.h;
                    (!inside && m.blink.is_none()).then_some(POINT { x: m.origin.0 + x, y: m.origin.1 + y })
                })
                .flatten();
                // Outside the menu: ask the caller whether the pointer has moved on.
                if let Some(p) = outside
                    && LEAVE.with(|l| l.borrow().as_ref().is_some_and(|f| f(p))) {
                        with(dismiss);
                    }
                return 0;
            }
            WM_MOUSELEAVE => return 0,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|m| {
                    if m.closing {
                        return;
                    }
                    if target_at(m, x, y) == Target::None {
                        let inside = x >= m.margin && y >= m.margin && x < m.margin + m.w && y < m.margin + m.h;
                        if !inside {
                            dismiss(m); // click outside (captured)
                        }
                    }
                });
                return 0;
            }
            WM_LBUTTONUP | WM_RBUTTONUP => {
                let (x, y) = (x_of(lp), y_of(lp));
                with(|m| {
                    if !m.closing {
                        let t = target_at(m, x, y);
                        choose(m, t);
                    }
                });
                return 0;
            }
            WM_KEYDOWN => {
                let vk = wp as u16;
                with(|m| {
                    let rows = selectable(m);
                    let cur = match m.hot {
                        Target::Row(i) => rows.iter().position(|r| *r == i),
                        _ => None,
                    };
                    match vk {
                        VK_ESCAPE => dismiss(m),
                        VK_DOWN | VK_UP if !rows.is_empty() => {
                            let n = rows.len() as i32;
                            let next = match cur {
                                Some(c) => (c as i32 + if vk == VK_DOWN { 1 } else { -1 }).rem_euclid(n),
                                None if vk == VK_DOWN => 0,
                                None => n - 1,
                            };
                            m.hot = Target::Row(rows[next as usize]);
                            render(m);
                        }
                        VK_RETURN | VK_SPACE => {
                            let t = m.hot;
                            choose(m, t);
                        }
                        _ => {
                            // First-letter jump.
                            let ch = (vk as u8 as char).to_ascii_lowercase();
                            if ch.is_ascii_alphanumeric() {
                                let found = rows.iter().copied().find(|&i| match &m.items[i] {
                                    Item::Action { label, .. } => label.to_lowercase().starts_with(ch),
                                    _ => false,
                                });
                                if let Some(i) = found {
                                    m.hot = Target::Row(i);
                                    render(m);
                                }
                            }
                        }
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
            WM_MOUSEACTIVATE => return MA_ACTIVATE as LRESULT,
            _ => {}
        }
        DefWindowProcW(h, msg, wp, lp)
    }
}
