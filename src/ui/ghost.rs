//! Ghost transitions: the card that travels between the deck and a note window.
//!
//! Opening, the note's card grows from where it was (its tab, the plus button, the pill)
//! into the window's frame; the real window is revealed underneath on arrival and the
//! ghost dissolves. Closing runs the same path in reverse, so a note always returns to
//! where it came from (spatial consistency).

use std::cell::RefCell;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::render::anim::{self, Client, Spring, clamp01};
use crate::app;
use crate::render::gfx::{Canvas, Font, Shape};
use crate::theme;
use crate::platform::util::{rect, wide};

const CLASS: &str = "PeekabooGhost";

pub enum Kind {
    /// Grow into a window; `on_arrive` reveals it.
    Open,
    /// Return home (tab / pill).
    Close,
    /// Sink and dissolve in place (delete, archive).
    Dissolve,
}

struct Ghost {
    hwnd: HWND,
    kind: Kind,
    origin: (i32, i32),
    canvas: Canvas,
    x: Spring,
    y: Spring,
    w: Spring,
    h: Spring,
    r: Spring,
    fade: Spring,
    start_area: f32,
    end_area: f32,
    color: u8,
    title: String,
    body: String,
    s: f32,
    title_font: Font,
    body_font: Font,
    on_arrive: Option<Box<dyn FnOnce()>>,
    arrived: bool,
}

thread_local! {
    static GHOSTS: RefCell<Vec<Ghost>> = const { RefCell::new(Vec::new()) };
}

fn union(a: &RECT, b: &RECT, pad: i32) -> RECT {
    RECT { left: a.left.min(b.left) - pad, top: a.top.min(b.top) - pad, right: a.right.max(b.right) + pad, bottom: a.bottom.max(b.bottom) + pad }
}

/// Fly a card from `from` to `to` (screen rects with corner radii).
#[allow(clippy::too_many_arguments)]
pub fn fly(kind: Kind, from: (RECT, f32), to: (RECT, f32), color: u8, title: &str, body: &str, on_arrive: Option<Box<dyn FnOnce()>>) {
    if anim::reduced() {
        if let Some(f) = on_arrive {
            f();
        }
        return;
    }
    let (_, s) = crate::ui::deck::monitor_geo();
    let pad = (48.0 * s) as i32;
    let b = union(&from.0, &to.0, pad);
    app::register_class(CLASS, Some(wndproc), IDC_ARROW);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            wide(CLASS).as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            b.left,
            b.top,
            b.right - b.left,
            b.bottom - b.top,
            null_mut(),
            null_mut(),
            app::hinst(),
            null_mut(),
        )
    };
    let params = match kind {
        Kind::Open => (0.86, 0.38),
        Kind::Close => (1.0, 0.3),
        Kind::Dissolve => (1.0, 0.26),
    };
    let lx = |r: &RECT| (r.left - b.left) as f32;
    let ly = |r: &RECT| (r.top - b.top) as f32;
    let (f, t) = (&from.0, &to.0);
    let mut g = Ghost {
        hwnd,
        kind,
        origin: (b.left, b.top),
        canvas: Canvas::new(b.right - b.left, b.bottom - b.top),
        x: Spring::px(lx(f), params),
        y: Spring::px(ly(f), params),
        w: Spring::px((f.right - f.left) as f32, params),
        h: Spring::px((f.bottom - f.top) as f32, params),
        r: Spring::px(from.1, params),
        fade: Spring::unit(1.0, theme::motion::SNAPPY),
        start_area: ((f.right - f.left) * (f.bottom - f.top)) as f32,
        end_area: ((t.right - t.left) * (t.bottom - t.top)) as f32,
        color,
        title: title.to_string(),
        body: body.chars().take(900).collect(),
        s,
        // The note's own typography, so the flight matches the window it becomes.
        title_font: crate::editor::rich::preview_fonts(s).0,
        body_font: crate::editor::rich::preview_fonts(s).1,
        on_arrive,
        arrived: false,
    };
    g.x.to(lx(t));
    g.y.to(ly(t));
    g.w.to((t.right - t.left) as f32);
    g.h.to((t.bottom - t.top) as f32);
    g.r.to(to.1);
    render(&mut g);
    unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    GHOSTS.with(|gs| gs.borrow_mut().push(g));
    anim::wake(Client::Ghost);
}

/// 0 at the start of the flight, 1 on arrival (by size, so it is robust to overshoot).
fn progress(g: &Ghost) -> f32 {
    let area = g.w.value * g.h.value;
    if (g.end_area - g.start_area).abs() < 1.0 {
        let total = (g.x.target - g.x.value).abs() + (g.y.target - g.y.value).abs();
        return 1.0 - clamp01(total / 200.0);
    }
    clamp01((area - g.start_area) / (g.end_area - g.start_area))
}

fn render(g: &mut Ghost) {
    let dark = app::dark();
    let s = g.s;
    let p = progress(g);
    let alpha = match g.kind {
        Kind::Open => g.fade.value,
        Kind::Close => 1.0 - clamp01((p - 0.7) / 0.3),
        Kind::Dissolve => 1.0 - p,
    };
    let c = &mut g.canvas;
    c.clear(0);
    let sh = Shape::new(g.x.value, g.y.value, g.w.value.max(1.0), g.h.value.max(1.0), g.r.value.max(0.0));
    c.card(&sh, theme::paper(g.color, dark), alpha, 0x000000, 0.06 * alpha, 1.6 * alpha, s);
    c.begin_batch();
    // Content sits where the note window draws it; the growing card reveals it.
    let content = match g.kind {
        Kind::Open => alpha * clamp01((p - 0.25) / 0.6),
        _ => alpha * (1.0 - clamp01(p / 0.6)),
    };
    if content > 0.01 && sh.w > 40.0 * s && sh.h > 30.0 * s {
        let (x0, y0) = (sh.x + 18.0 * s, sh.y + 42.0 * s);
        let right = sh.x + sh.w - 16.0 * s;
        let bottom = sh.y + sh.h - 12.0 * s;
        if right > x0 && bottom > y0 {
            let tr = rect(x0 as i32, y0 as i32, (right - x0) as i32, (26.0 * s) as i32);
            let clip_title = rect(tr.left, tr.top, tr.right - tr.left, ((bottom - y0) as i32).min(tr.bottom - tr.top));
            c.text(&g.title, &g.title_font, clip_title, theme::PAPER_TEXT, content, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
            let by = y0 + 32.0 * s;
            if bottom > by && !g.body.is_empty() {
                let br = rect(x0 as i32, by as i32, (right - x0) as i32, (bottom - by) as i32);
                crate::ui::draw_body(c, &g.body, &g.body_font, br, g.color, dark, content, s);
            }
        }
    }
    c.flush();
    c.present(g.hwnd, g.origin.0, g.origin.1);
}

pub fn frame(dt: f32) -> bool {
    let mut arrivals: Vec<Box<dyn FnOnce()>> = Vec::new();
    let alive = GHOSTS.with(|gs| {
        let mut gs = gs.borrow_mut();
        for g in gs.iter_mut() {
            let mut moving = false;
            for sp in [&mut g.x, &mut g.y, &mut g.w, &mut g.h, &mut g.r, &mut g.fade] {
                moving |= sp.step(dt);
            }
            let near = (g.w.target - g.w.value).abs() < 1.5 && (g.h.target - g.h.value).abs() < 1.5 && (g.x.target - g.x.value).abs() < 1.5 && (g.y.target - g.y.value).abs() < 1.5;
            if !g.arrived && (near || !moving) {
                g.arrived = true;
                if let Some(f) = g.on_arrive.take() {
                    arrivals.push(f);
                }
                match g.kind {
                    Kind::Open => g.fade.to(0.0),
                    _ => g.fade.snap(0.0),
                }
            }
            render(g);
        }
        // Retire ghosts that have arrived and faded.
        gs.retain(|g| {
            let done = g.arrived && g.fade.settled() && g.fade.value <= 0.01;
            if done {
                unsafe { DestroyWindow(g.hwnd) };
            }
            !done
        });
        !gs.is_empty()
    });
    for f in arrivals {
        f();
    }
    alive
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}
