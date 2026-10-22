//! Windows and surfaces: the edge deck, note windows, All Notes, Settings, menus, toasts.

pub mod allnotes;
pub mod deck;
pub mod ghost;
pub mod menu;
pub mod note;
pub mod settings;
pub mod toast;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{DT_END_ELLIPSIS, DT_LEFT, DT_WORDBREAK};

use crate::render::gfx::{Canvas, Font, Shape};

/// A note's preview body (from `store::body_of`) in `r`, on paper `color`: list markers
/// hang in their own column so wrapped lines line up under the text, nested items step
/// in, checklist items get the editor's circles (ticked ones filled, their text struck
/// and dimmed), and the last line that fits ends in an ellipsis.
#[allow(clippy::too_many_arguments)]
pub fn draw_body(c: &mut Canvas, body: &str, f: &Font, r: RECT, color: u8, dark: bool, alpha: f32, s: f32) {
    let paper = crate::theme::paper(color, dark);
    let ink = crate::theme::paper_ink(color, dark);
    let text_rgb = crate::theme::paper_subtext(color, dark);
    let done_rgb = crate::platform::util::mix(text_rgb, paper, 0.3);
    let struck = f.struck();
    let line_h = (f.ascent + f.descent).max(1);
    let step = (16.0 * s) as i32;
    // Circle size and column, in proportion to the editor's (diameter ~1.45 cap heights).
    let d = (f.cap as f32 * 1.45).round();
    let box_w = (d + 6.0 * s) as i32;
    let mut y = r.top;
    for l in body.lines() {
        let room = (r.bottom - y) / line_h * line_h;
        if room < line_h {
            break;
        }
        let t = l.trim_start_matches(' ');
        let x = r.left + ((l.len() - t.len()) / 2) as i32 * step;
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
        match task {
            Some(done) => {
                let (cx, cy) = (x as f32 + d / 2.0, (y + f.ascent) as f32 - f.cap as f32 * 0.5);
                let circle = Shape::new(cx - d / 2.0, cy - d / 2.0, d, d, d / 2.0);
                if done {
                    c.shape(&circle, ink, alpha, 1.0, 0.0);
                    crate::editor::checks::check(c, cx, cy, d, 0xFFFFFF, alpha, s);
                } else {
                    c.shape(&circle, crate::platform::util::mix(ink, paper, 0.42), alpha, 1.0, 1.3 * s);
                }
            }
            None if !mark.is_empty() => {
                c.line(mark, f, x, y + f.ascent, tx - x, None, text_rgb, alpha, DT_LEFT);
            }
            None => {}
        }
        let (font, rgb) = if task == Some(true) { (&struck, done_rgb) } else { (f, text_rgb) };
        let h = font.wrapped_height(text, w);
        c.text(text, font, crate::platform::util::rect(tx, y, w, h.min(room)), rgb, alpha, DT_LEFT | DT_WORDBREAK | DT_END_ELLIPSIS);
        y += h.max(line_h);
    }
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
