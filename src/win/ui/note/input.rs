// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The editor control's subclass: keys, clicks and the checklist overlay hooks.

use super::*;

thread_local! {
    /// An attachment pressed on: (where, its file), until the pointer moves far enough
    /// to drag it out or comes back up to open it.
    static PRESS: std::cell::RefCell<Option<(POINT, std::path::PathBuf, String)>> = const { std::cell::RefCell::new(None) };
    /// What lies under the character the pointer was last over (see `Hover`).
    static HOVER: std::cell::RefCell<Option<Hover>> = const { std::cell::RefCell::new(None) };
}

/// Editor `e`'s DPI scale: the monitor its note is on.
fn scale(e: HWND) -> f32 {
    unsafe { GetDpiForWindow(e) }.max(96) as f32 / 96.0
}

/// About one body line's height in editor `e`, in pixels (the body size is in twips, 15
/// to a pixel at 96 DPI).
fn line_h(e: HWND) -> i32 {
    (rich::body_size() as f32 / 15.0 * 1.45 * scale(e)) as i32
}

/// A link: (start, end, address).
type Link = (i32, i32, String);

/// What lies around the character under the pointer, worked out for that character and
/// kept while the pointer stays on it: the checklist circles a point there can be on, and
/// the characters a link can be under it at (the one pointed at, then the one before),
/// with the area each covers, its link and that link's tip, each found when first asked
/// for. A move within the character only tests the point against these (each move read
/// the note's whole text four times, 2 MB in a long one, and asked RichEdit where a dozen
/// characters are). Any message to the editor other than the pointer's own may change
/// what's where, and lets it go (`hover_forget`).
struct Hover {
    e: isize,
    /// The point last asked about, and the character there.
    at: (i32, i32),
    cp: i32,
    /// Where the view was scrolled to, and how long the text was.
    view: (i32, usize),
    /// The paragraphs of the character and the one before it: all any of this looks at.
    t: rich::Span,
    /// Circles: (the box, its centre, how far around it counts).
    boxes: Vec<(i32, f32, f32, f32)>,
    /// The characters.
    chars: [Under; 2],
}

/// A character a link can be under the pointer at: the area it covers, its link and the
/// link's tip (anchor and label), each found when first asked for.
struct Under {
    c: i32,
    area: OnceCell<RECT>,
    link: OnceCell<Option<Link>>,
    tip: OnceCell<(RECT, String)>,
}

/// A message other than the pointer's came to editor `e`'s window: what `Hover` knows may
/// be out of date.
pub(super) fn hover_forget() {
    // Being worked out right now (its questions to RichEdit come back here): it's new.
    HOVER.with(|h| {
        if let Ok(mut h) = h.try_borrow_mut() {
            h.take();
        }
    });
}

/// Messages that only ask RichEdit something (where a character is, how long the text is,
/// what the selection or a format is): they change nothing under the pointer.
fn asks_only(msg: u32) -> bool {
    const WM_GETTEXT: u32 = 0x000D;
    const WM_GETTEXTLENGTH: u32 = 0x000E;
    const EM_GETSEL: u32 = 0x00B0;
    const EM_GETLINECOUNT: u32 = 0x00BA;
    const EM_LINEINDEX: u32 = 0x00BB;
    const EM_LINELENGTH: u32 = 0x00C1;
    const EM_POSFROMCHAR: u32 = 0x00D6;
    const EM_CHARFROMPOS: u32 = 0x00D7;
    const EM_EXGETSEL: u32 = 0x0434;
    const EM_EXLINEFROMCHAR: u32 = 0x0436;
    const EM_GETCHARFORMAT: u32 = 0x043A;
    const EM_GETEVENTMASK: u32 = 0x043B;
    const EM_GETOLEINTERFACE: u32 = 0x043C;
    const EM_GETPARAFORMAT: u32 = 0x043D;
    const EM_GETTEXTRANGE: u32 = 0x044B;
    const EM_GETTEXTLENGTHEX: u32 = 0x045F;
    const EM_GETTEXTEX: u32 = 0x045E;
    const EM_GETSCROLLPOS: u32 = 0x04DD;
    matches!(
        msg,
        WM_GETTEXT
            | WM_GETTEXTLENGTH
            | EM_GETSEL
            | EM_GETLINECOUNT
            | EM_LINEINDEX
            | EM_LINELENGTH
            | EM_POSFROMCHAR
            | EM_CHARFROMPOS
            | EM_EXGETSEL
            | EM_EXLINEFROMCHAR
            | EM_GETCHARFORMAT
            | EM_GETEVENTMASK
            | EM_GETOLEINTERFACE
            | EM_GETPARAFORMAT
            | EM_GETTEXTRANGE
            | EM_GETTEXTLENGTHEX
            | EM_GETTEXTEX
            | EM_GETSCROLLPOS
    )
}

/// Run `f` on what lies around the character under (`x`, `y`) in editor `e`. A move asks
/// two or three times about the same point (the checkbox for the ring and the cursor,
/// the link for its tip): only the first asks RichEdit what's there.
fn hover<R>(e: HWND, x: i32, y: i32, f: impl FnOnce(&Hover) -> R) -> R {
    HOVER.with(|h| {
        let mut h = h.borrow_mut();
        if h.as_ref().is_none_or(|hv| hv.e != e as isize || hv.at != (x, y)) {
            let cp = rich::char_from_pos(e, x, y);
            let view = (rich::scroll_y(e), rich::text_len(e));
            match h.as_mut() {
                Some(hv) if hv.e == e as isize && hv.cp == cp && hv.view == view => hv.at = (x, y),
                _ => *h = Some(Hover::new(e, (x, y), cp, view)),
            }
        }
        f(h.as_ref().unwrap())
    })
}

impl Hover {
    fn new(e: HWND, at: (i32, i32), cp: i32, view: (i32, usize)) -> Hover {
        let t = rich::paras_around(e, (cp - 1).max(0) as usize, (cp + 1).max(0) as usize);
        // The box of the line under the point: the circle is drawn left of the text, over
        // the glyph and the gap after it. A point left of a line can come back as the mark
        // ending the line above: the box after that mark counts too.
        let ps = (t.from + rich::para_start(&t.t, cp.max(0) - t.from as i32) as usize) as i32;
        let after = if t.get(cp.max(0) as usize) == Some(rich::CR) { cp + 1 } else { ps };
        let mut near = [ps, cp, cp - 1, after];
        near.sort_unstable();
        let boxes = near
            .into_iter()
            .enumerate()
            .filter(|&(i, c)| i == 0 || near[i - 1] != c)
            .map(|(_, c)| c)
            .filter(|&c| c >= 0 && t.get(c as usize).is_some_and(is_task) && (c == 0 || t.get(c as usize - 1) == Some(rich::CR)))
            .map(|c| {
                let (cx, cy, r) = crate::win::editor::checks::circle(e, c, t.get(c as usize + 1));
                (c, cx, cy, r)
            })
            .collect();
        let chars = [cp, cp - 1].map(|c| Under { c, area: OnceCell::new(), link: OnceCell::new(), tip: OnceCell::new() });
        Hover { e: e as isize, at, cp, view, t, boxes, chars }
    }

    /// The box whose circle (`x`, `y`) is on.
    fn box_at(&self, x: i32, y: i32) -> Option<i32> {
        self.boxes.iter().find(|&&(_, cx, cy, r)| (x as f32 - cx).abs() <= r && (y as f32 - cy).abs() <= r).map(|b| b.0)
    }

    /// Which of the characters (`chars`) (`x`, `y`) is on.
    fn char_at(&self, x: i32, y: i32) -> Option<usize> {
        let e = self.e as HWND;
        (0..2).find(|&k| {
            let c = self.chars[k].c;
            c >= 0 && {
                let r = self.chars[k].area.get_or_init(|| {
                    let lh = line_h(e);
                    let (p, q) = (rich::pos_from_char(e, c), rich::pos_from_char(e, c + 1));
                    let right = if q.y == p.y && q.x > p.x { q.x } else { p.x };
                    // A line is as tall as what's on it (a picture): down to where the next
                    // starts.
                    let next_line = self.t.t.get(c as usize - self.t.from..).and_then(|r| r.iter().position(|&ch| ch == 13)).map(|i| c + i as i32 + 1);
                    let bottom = next_line.filter(|&n| (n as usize) < self.t.len).map_or(p.y + lh, |n| rich::pos_from_char(e, n).y);
                    RECT { left: p.x, right, top: p.y, bottom: bottom.max(p.y + lh) }
                });
                x >= r.left && x < r.right && y >= r.top && y < r.bottom
            }
        })
    }

    /// The link under (`x`, `y`), and the character pointed at.
    fn link_at(&self, x: i32, y: i32) -> Option<(&Link, usize)> {
        let k = self.char_at(x, y)?;
        let (e, c) = (self.e as HWND, self.chars[k].c);
        let link = self.chars[k].link.get_or_init(|| rich::item_link_in(e, &self.t, c as usize).or_else(|| rich::link_at(e, c).filter(|l| c >= l.0 && c < l.1)));
        link.as_ref().map(|l| (l, k))
    }

    /// The tip for the link under (`x`, `y`): its anchor and its label.
    fn tip_at(&self, x: i32, y: i32) -> Option<&(RECT, String)> {
        let (link, k) = self.link_at(x, y)?;
        Some(self.chars[k].tip.get_or_init(|| tip(self.e as HWND, &self.t, link, self.chars[k].c)))
    }
}

/// The checkbox whose circle is at (`x`, `y`) in editor `e`.
pub(super) fn checkbox_at(e: HWND, x: i32, y: i32) -> Option<i32> {
    hover(e, x, y, |hv| hv.box_at(x, y))
}

/// `checkbox_at`, worked out afresh for this very point: what the cursor shows and what a
/// click acts on, never what was kept from an earlier move. (A pointer that jumped onto a
/// circle from the text kept the I-beam, and its click went into the text.)
pub(super) fn checkbox_now(e: HWND, x: i32, y: i32) -> Option<i32> {
    hover_forget();
    checkbox_at(e, x, y)
}

/// The link under the pointer: (start, end, address, the character pointed at). A
/// picture counts as its attachment's link.
pub(super) fn link_under(e: HWND, x: i32, y: i32) -> Option<(i32, i32, String, i32)> {
    hover(e, x, y, |hv| hv.link_at(x, y).map(|((a, b, url), k)| (*a, *b, url.clone(), hv.chars[k].c)))
}

/// Over a link, a tooltip shows where it goes.
fn link_tip(e: HWND, x: i32, y: i32) {
    use crate::win::ui::tooltip;
    hover(e, x, y, |hv| match hv.tip_at(x, y) {
        Some((anchor, label)) => tooltip::hover(e, *anchor, label, ""),
        None => tooltip::leave(e),
    });
}

/// The tooltip for `link` at character `cp`, which `t` holds the paragraph of: where it
/// goes, and what it says.
fn tip(e: HWND, t: &rich::Span, link: &Link, cp: i32) -> (RECT, String) {
    let (s, lh) = (scale(e), line_h(e));
    let (a, b) = (link.0, link.1);
    // Under the part of the link the pointer is on.
    let from = match (a as usize).checked_sub(t.from).zip((b as usize).checked_sub(t.from)).filter(|&(_, j)| j <= t.t.len()) {
        Some((i, j)) => t.from as i32 + rich::name_start(&t.t, i as i32, j as i32),
        None => a + rich::name_start(&rich::raw_text(e, a as usize, b as usize), 0, b - a),
    }
    .max(a);
    let p0 = rich::pos_from_char(e, from);
    let pe = rich::pos_from_char(e, b);
    let p = rich::pos_from_char(e, cp);
    let half = (20.0 * s) as i32;
    let (l, r) = if p0.y == pe.y { (p0.x, pe.x) } else { (p.x - half, p.x + half) };
    let anchor = crate::win::util::rect(l, p.y, (r - l).max(1), lh);
    // An attachment: its name, and what dragging does.
    if let Some(name) = crate::common::attach::name_of(&link.2) {
        // Under what the line shows. A picture sits on its line's baseline, so that's its
        // bottom edge; where the next line starts can be further down (a gap, an empty
        // line), which put the tip over the text below. A file's chip goes down to there.
        let anchor = if rich::item_link_in(e, t, cp as usize).is_some() {
            let (s0, s1) = item_shown_in(t, cp as usize);
            let (q0, q1) = (rich::pos_from_char(e, s0 as i32), rich::pos_from_char(e, s1 as i32));
            let next = t.t[cp as usize - t.from..].iter().position(|&c| c == 13).map(|i| cp + i as i32 + 1).filter(|&n| (n as usize) < t.len);
            let picture = t.get(s0) == Some(0xFFFC);
            let line_end = next.map_or(q0.y + lh, |n| rich::pos_from_char(e, n).y);
            let bottom = if picture { rich::baseline_at(e, s0 as i32).filter(|&b| b > q0.y).unwrap_or(line_end) } else { line_end }.min(q0.y + (600.0 * s) as i32);
            crate::win::util::rect(q0.x, q0.y, (q1.x - q0.x).max(1), (bottom - q0.y).max(lh))
        } else {
            anchor
        };
        return (anchor, name);
    }
    let url = link.2.as_str();
    let shown = url.strip_prefix("https://").or(url.strip_prefix("http://")).unwrap_or(url).trim_end_matches('/');
    let shown: String = if shown.chars().count() > 60 { shown.chars().take(59).chain(std::iter::once('\u{2026}')).collect() } else { shown.to_string() };
    (anchor, shown)
}

/// A picked picture: (editor, the selection, the picture's character, the size it's shown
/// at in twips, for its height).
type Picked = (isize, (i32, i32), i32, (i32, i32));

thread_local! {
    /// An attachment selected by a click on it: (editor, the selection, its picture's
    /// character, when it's a picture). RichEdit draws a selected picture as its colour
    /// negative, so while it's selected that way the selection is hidden and a frame is
    /// drawn round the picture instead, until the selection changes.
    static PICKED: std::cell::Cell<Option<Picked>> = const { std::cell::Cell::new(None) };
}

/// The picture picked in `e`, no longer kept.
fn take_picked(e: HWND) -> Option<Picked> {
    PICKED.with(|p| p.get().filter(|p| p.0 == e as isize).inspect(|_| p.set(None)))
}

/// Select `a..b`, an attachment's line, as a click on it does.
fn pick(e: HWND, a: i32, b: i32) {
    unpick(e);
    rich::set_sel(e, a, b);
    let Some(i) = rich::raw_text(e, a.max(0) as usize, b.max(0) as usize).iter().position(|&c| c == 0xFFFC) else { return };
    let obj = a + i as i32;
    // Read with the selection on the picture alone, then back on its line.
    let shown = rich::picture_shown(e, obj).unwrap_or((1, 1));
    rich::set_sel(e, a, b);
    // Held hidden: the changes that hide the selection while they work (restyling, a
    // menu asking the format) would otherwise show it again, as the negative.
    rich::hold_selection_hidden(e, true);
    PICKED.with(|p| p.set(Some((e as isize, (a, b), obj, shown))));
    // Framed as it's drawn (`paint_frame`).
    crate::win::editor::view::present(e);
}

/// The picture picked in `e` (if any) is no longer: its selection shows as RichEdit draws
/// it, and the frame goes.
fn unpick(e: HWND) {
    if take_picked(e).is_some() {
        rich::hold_selection_hidden(e, false);
        crate::win::editor::view::present(e);
    }
}

/// After message `msg` to `e` (which `drew` or not): the selection having moved on, a
/// picked picture is let go. (Its frame is drawn with the editor, `paint_frame`.)
fn picked_after(e: HWND, msg: u32, drew: bool) {
    let Some((_, at, _, _)) = PICKED.with(|p| p.get()).filter(|p| p.0 == e as isize) else { return };
    const EM_SETSEL: u32 = 0x00B1;
    const EM_EXSETSEL: u32 = 0x0437;
    if !drew && !matches!(msg, WM_KEYDOWN | WM_CHAR | WM_LBUTTONDOWN | WM_RBUTTONDOWN | EM_SETSEL | EM_EXSETSEL | WM_KILLFOCUS) {
        return;
    }
    if rich::sel(e) != at {
        unpick(e);
    } else if msg == WM_SETFOCUS {
        // Taking the focus back (after a menu, Settings, another window), RichEdit shows
        // the selection again, the picture as its negative: hidden again, and the picture
        // drawn afresh, framed.
        rich::send(e, rich::EM_HIDESELECTION, 0, 0);
        rich::send(e, rich::EM_HIDESELECTION, 1, 0);
        crate::win::editor::view::present(e);
    }
}

/// The bottom of a picture whose top is at `top`, `width` px wide and shown at `shown`
/// (twips): as tall as its shape makes it at that width.
fn picture_bottom(top: i32, width: i32, shown: (i32, i32)) -> i32 {
    top + ((width as i64 * shown.1.max(1) as i64 + shown.0.max(1) as i64 / 2) / shown.0.max(1) as i64) as i32
}

/// Where the frame round the picture at character `obj` in `e`, shown at `shown` (twips),
/// goes: around the picture as it's drawn.
fn picture_frame(e: HWND, obj: i32, shown: (i32, i32)) -> Option<RECT> {
    let (p0, p1) = (rich::pos_from_char(e, obj), rich::pos_from_char(e, obj + 1));
    if p1.y != p0.y || p1.x <= p0.x {
        return None;
    }
    // As tall as its shape at the width it's drawn: RichEdit sets a picture a little below
    // its line's baseline, so that left the frame's bottom edge 4 px up the picture.
    let bottom = picture_bottom(p0.y, p1.x - p0.x, shown);
    Some(RECT { left: p0.x, top: p0.y, right: p1.x, bottom })
}

/// The picked picture's frame, if editor `e` has one, onto `c`, which holds its client rows
/// from `top` on (`view::set_painter`).
pub(super) fn paint_frame(e: HWND, c: &mut crate::win::render::gfx::Canvas, top: i32) {
    let Some((_, _, obj, shown)) = PICKED.with(|p| p.get()).filter(|p| p.0 == e as isize) else { return };
    let Some(r) = picture_frame(e, obj, shown) else { return };
    let w = (2.0 * scale(e)).round().max(1.0) as i32;
    // The system's selection colour, as RGB.
    let bgr = unsafe { windows_sys::Win32::Graphics::Gdi::GetSysColor(windows_sys::Win32::Graphics::Gdi::COLOR_HIGHLIGHT) };
    let argb = 0xFF00_0000 | ((bgr & 0xFF) << 16) | (bgr & 0xFF00) | ((bgr >> 16) & 0xFF);
    for edge in [
        (r.left, r.top, r.right, r.top + w),
        (r.left, r.bottom - w, r.right, r.bottom),
        (r.left, r.top, r.left + w, r.bottom),
        (r.right - w, r.top, r.right, r.bottom),
    ] {
        c.fill_raw(edge.0, edge.1 - top, edge.2, edge.3 - top, argb);
    }
}

/// `rich::item_shown` for character `cp`, which `t` holds the paragraph of.
fn item_shown_in(t: &rich::Span, cp: usize) -> (usize, usize) {
    let (a, b) = rich::item_shown(&t.t, cp - t.from);
    (t.from + a, t.from + b)
}

/// The text menu (formatting and editing) at screen point `p`, or under the caret when
/// `p` is (-1, -1), as the keyboard asks for it.
fn text_menu(h: HWND, e: HWND, mut p: POINT) {
    if id_of(h).is_none() {
        return;
    }
    if p.x == -1 && p.y == -1 {
        let (a, _) = rich::sel(e);
        p = rich::pos_from_char(e, a);
        unsafe { ClientToScreen(e, &mut p) };
        p.y += (20.0 * scale(e)) as i32;
    }
    let cmd = app::popup(app::edit_menu(e), p.x, p.y, h);
    if cmd != 0 {
        unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
    }
}


pub(super) unsafe extern "system" fn edit_proc(e: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, parent: usize) -> LRESULT {
    match msg {
        // Drawn whole off screen, what goes over the text included, and put on the window
        // in one copy (`view`). Should RichEdit's text services not draw, it paints itself.
        WM_PAINT if crate::win::editor::view::on_paint(e) => {
            // Whatever it drew may have moved the text: the scroll bar keeps in step.
            super::scrollbar::sync(parent as HWND);
            return 0;
        }
        // `view` paints every pixel: nothing to clear first (that showed as a flash).
        WM_ERASEBKGND if rich::services(e).is_some() => return 1,
        // The caret is drawn with the rest of the frame (`view`), and blinks there.
        WM_TIMER if wp == crate::win::editor::view::CARET_TIMER => {
            crate::win::editor::view::blink(e);
            return 0;
        }
        _ => {}
    }
    // What changes the text or the view is done with drawing held, and shown in one frame
    // when it's done (`rich::quiet`): a key that changes several things (an address made a
    // link, a box typed and styled, a line pushed down) never shows them a step at a time.
    // A key that changed nothing that's drawn (a letter's key-down, a key-up, the caret
    // moved) shows nothing. Not around the menu keys, whose menu waits on its own loop.
    let quiet = match msg {
        WM_KEYDOWN => !matches!(wp as u16, VK_APPS | VK_F10),
        WM_KEYUP | WM_CHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO | EM_UNDO | rich::EM_REDO | rich::EM_SETSCROLLPOS | WM_VSCROLL | WM_SIZE | WM_TIMER => true,
        _ => false,
    };
    // Typing at the bottom edge keeps the caret's line in view (`follow_typing`).
    let typed = matches!(msg, WM_CHAR | WM_KEYDOWN | WM_IME_CHAR | WM_IME_COMPOSITION).then(|| super::scrollbar::before_typing(e));
    let now = || {
        let r = unsafe { edit_now(e, msg, wp, lp, parent) };
        if let Some(before) = typed {
            super::scrollbar::follow_typing(e, before);
        }
        r
    };
    if !quiet {
        let r = now();
        match msg {
            WM_SETFOCUS | WM_KILLFOCUS => crate::win::editor::view::focus(e, msg == WM_SETFOCUS),
            // What may move the caret without drawing anything else (a click, a drag, a
            // composed character): it's drawn where it went.
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_CAPTURECHANGED | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION | WM_IME_CHAR => {
                crate::win::editor::view::caret_moved(e)
            }
            WM_MOUSEMOVE if wp & 0x0001 != 0 => crate::win::editor::view::caret_moved(e),
            _ => {}
        }
        return r;
    }
    let before = matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_TIMER).then(|| view_of(e));
    let r = rich::quiet_if(
        e,
        now,
        |_| match before {
            Some(b) if msg == WM_TIMER => view_of(e) != b,
            Some(b) => key_drew(msg, wp as u16, b, view_of(e)),
            None => true,
        },
    );
    // A key that only moved the caret (an arrow) drew nothing else: the caret is drawn
    // where it went (nothing, when the frame just shown has it there).
    crate::win::editor::view::caret_moved(e);
    r
}

const EM_UNDO: u32 = 0x00C7;

/// `edit_proc`, with drawing held where it holds it.
unsafe fn edit_now(e: HWND, msg: u32, wp: WPARAM, lp: LPARAM, parent: usize) -> LRESULT {
    let h = parent as HWND;
    // Anything but the pointer moving, or a question, may change what's under it.
    if !matches!(msg, WM_MOUSEMOVE | WM_SETCURSOR | WM_NCHITTEST | WM_MOUSELEAVE) && !asks_only(msg) {
        hover_forget();
    }
    match msg {
        // One scroller for the whole note (see `scrollbar::wheel`), never RichEdit's own.
        WM_MOUSEWHEEL => {
            super::scrollbar::wheel(h, (wp >> 16) as i16);
            return 0;
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            super::scrollbar::interrupt(h);
            // A link's tip would cover what's being typed.
            crate::win::ui::tooltip::mute_for(e);
            let vk = wp as u16;
            let (ctrl, shift, alt) = (key_down(VK_CONTROL), key_down(VK_SHIFT), key_down(VK_MENU));
            let Some(st) = state(h) else { return unsafe { DefSubclassProc(e, msg, wp, lp) } };
            st.swallow = false;
            // The keyboard's Menu key, or Shift+F10: the text menu at the caret.
            if vk == VK_APPS || (shift && !ctrl && vk == VK_F10) {
                st.swallow = true;
                text_menu(h, e, POINT { x: -1, y: -1 });
                return 0;
            }
            if !matches!(vk, VK_BACK | VK_SHIFT | VK_CONTROL | VK_MENU) {
                st.auto = None;
            }
            // Undo and redo are the editor's own; a change made in several steps (a tick,
            // a new block, an assistant's edit) goes and comes back in one.
            if ctrl && !alt && ((shift && vk == b'Z' as u16) || (!shift && vk == b'Y' as u16)) {
                if !rich::redo_group(e) {
                    unsafe { DefSubclassProc(e, rich::EM_REDO, 0, 0) };
                    selection_off_boxes(e);
                }
                st.swallow = true;
                return 0;
            }
            if ctrl && !shift && !alt && vk == b'Z' as u16 {
                if rich::undo_group(e) {
                    mark_dirty(h);
                    st.swallow = true;
                    return 0;
                }
                // RichEdit's own undo, which selects what it put back.
                const EM_UNDO: u32 = 0x00C7;
                unsafe { DefSubclassProc(e, EM_UNDO, 0, 0) };
                selection_off_boxes(e);
                st.swallow = true;
                return 0;
            } else if !matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU) {
                rich::forget_groups();
            }
            if !alt {
                if let Some(cmd) = key_cmd(vk, ctrl, shift) {
                    st.swallow = true;
                    unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
                    return 0;
                }
                if vk == VK_BACK && !ctrl && block_backspace(e) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                if vk == VK_BACK && !ctrl && revert_auto(h, e) {
                    st.swallow = true;
                    return 0;
                }
                // At a nested item's start: out a level first; only at the top level does
                // the bullet go.
                if vk == VK_BACK && !ctrl && !shift && rich::backspace_outdents(e) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                // A tab character has no place in the title, and a note window has nothing
                // else for Tab to move on to (in All Notes it leaves the note, see
                // `allnotes::pre_translate`).
                if vk == VK_TAB && !ctrl && rich::sel_in_title(e) {
                    st.swallow = true;
                    return 0;
                }
                if vk == VK_TAB && !ctrl && block_tab(e, shift) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                if vk == VK_TAB && !ctrl && rich::nest(e, if shift { -1 } else { 1 }) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                // Shift+Tab anywhere else but a table (the cell before) takes a tab back off
                // the line's start, if it has one, and never types one.
                if vk == VK_TAB && !ctrl && shift && !matches!(rich::cell_at(e, &rich::text(e), rich::sel(e).0), Some((rich::Block::Table, ..))) {
                    st.swallow = true;
                    if rich::untab_lines(e) {
                        mark_dirty(h);
                    }
                    return 0;
                }
                if vk == VK_DELETE && !ctrl && delete_into_item(e) {
                    st.swallow = true;
                    mark_dirty(h);
                    return 0;
                }
                if vk == VK_RETURN {
                    rich::link_word_before(e);
                }
                if vk == VK_RETURN && !ctrl && !shift && handle_enter(h, e) {
                    st.swallow = true;
                    // Made with the view held still: a new line at the bottom edge comes
                    // into view, with its caret, as one RichEdit made would.
                    super::scrollbar::keep_caret_in_view(e);
                    return 0;
                }
            }
        }
        WM_CHAR => {
            let c = wp as u32;
            // An emoji's variation selector goes in as its hidden stand-in (see
            // rich::selectors): the real one would stop RichEdit drawing list bullets.
            if let Some(m) = rich::marker_for(c as u16) {
                let r = unsafe { DefSubclassProc(e, msg, m as usize, lp) };
                rich::hide_marks(e);
                return r;
            }
            if let Some(st) = state(h)
                && st.swallow && (c < 0x20 || c == 0x7F) {
                    st.swallow = false;
                    return 0;
                }
            if c == b' ' as u32 && try_auto_list(h, e) {
                return 0;
            }
            // A box typed (from the emoji panel, say) at a line's start makes the line a
            // checklist item: it gets the look at once.
            if c == rich::BOX as u32 || c == rich::CHECKED as u32 {
                let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
                let (at, _) = rich::sel(e);
                rich::style_boxes_in(e, at - 1, at - 1);
                return r;
            }
            // A space or Tab ends a word: an address becomes a link.
            if c == b' ' as u32 || c == b'\t' as u32 {
                let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
                rich::link_word_before(e);
                return r;
            }
        }
        WM_PASTE => {
            paste(h);
            return 0;
        }
        // Clicking away ends the word being typed too.
        WM_KILLFOCUS => rich::link_word_before(e),
        // A quick second click on a circle arrives as a double-click: tick again, rather
        // than letting RichEdit select the word under it.
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if let Some(st) = state(h) {
                st.auto = None;
            }
            // An attachment: a double-click opens it; a press drags it out, or (let go
            // where it was) selects it.
            if msg == WM_LBUTTONDBLCLK
                && let Some((_, _, url, _)) = link_under(e, x_of(lp), y_of(lp))
                && super::attach::file_of(&url).is_some()
            {
                super::open_link(&url);
                return 0;
            }
            if msg == WM_LBUTTONDOWN
                && let Some((_, _, url, _)) = link_under(e, x_of(lp), y_of(lp))
                && let Some(path) = super::attach::file_of(&url)
            {
                PRESS.with(|p| *p.borrow_mut() = Some((POINT { x: x_of(lp), y: y_of(lp) }, path, url)));
                unsafe {
                    SetFocus(e);
                    SetCapture(e);
                }
                return 0;
            }
            if !key_down(VK_SHIFT)
                && let Some(cp) = checkbox_now(e, x_of(lp), y_of(lp)) {
                    unsafe { SetFocus(e) };
                    toggle_check(e, cp);
                    mark_dirty(h);
                    return 0;
                }
        }
        // RichEdit hands right-clicks to its OLE callback (see `attach`) rather than send
        // WM_CONTEXTMENU, so the text menu opens from here. A right-click outside the
        // selection moves the caret there first; inside it, the selection stays to act on.
        WM_RBUTTONDOWN => {
            let cp = rich::char_from_pos(e, x_of(lp), y_of(lp));
            let (a, b) = rich::sel(e);
            if a == b || cp < a.min(b) || cp > a.max(b) {
                rich::set_sel(e, cp, cp);
                // On a checklist circle, after the box, as a left click leaves it.
                caret_off_box(e, false);
            }
            unsafe { SetFocus(e) };
            return 0;
        }
        WM_RBUTTONUP => {
            let mut p = POINT { x: x_of(lp), y: y_of(lp) };
            unsafe { ClientToScreen(e, &mut p) };
            text_menu(h, e, p);
            return 0;
        }
        WM_CONTEXTMENU => {
            text_menu(h, e, POINT { x: x_of(lp), y: y_of(lp) });
            return 0;
        }
        WM_MOUSEMOVE if PRESS.with(|p| p.borrow().is_some()) => {
            let far = PRESS.with(|p| {
                p.borrow().as_ref().is_some_and(|(at, _, _)| (x_of(lp) - at.x).abs() > 4 || (y_of(lp) - at.y).abs() > 4)
            });
            if far && let Some((_, path, url)) = PRESS.with(|p| p.borrow_mut().take()) {
                unsafe { ReleaseCapture() };
                crate::win::ui::tooltip::mute();
                super::attach::drag_out(e, &path, &url);
            }
            return 0;
        }
        WM_LBUTTONUP if PRESS.with(|p| p.borrow().is_some()) => {
            // Taken before letting go of the mouse: losing capture forgets the press.
            let press = PRESS.with(|p| p.borrow_mut().take());
            unsafe { ReleaseCapture() };
            if let Some((at, _, _)) = press
                && let Some((_, b, _, cp)) = link_under(e, at.x, at.y)
            {
                // Select it, as Explorer does with a file: it can be cut, copied or deleted.
                let (start, _) = item_shown_in(&rich::paras_around(e, cp as usize, cp as usize), cp as usize);
                pick(e, start as i32, b);
            }
            return 0;
        }
        WM_CAPTURECHANGED => {
            PRESS.with(|p| p.borrow_mut().take());
        }
        WM_MOUSEMOVE => {
            let hot = checkbox_at(e, x_of(lp), y_of(lp));
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: e, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut tme) };
            crate::win::editor::checks::set_hot(e, hot);
            pointer_in(h);
            // No link's or picture's tip in the middle of selecting by dragging.
            if wp & 0x0001 != 0 {
                crate::win::ui::tooltip::leave(e);
            } else {
                link_tip(e, x_of(lp), y_of(lp));
            }
            if hot.is_some() && wp & 0x0001 == 0 {
                // RichEdit may set its own cursor as it takes the move: the hand stays.
                let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
                unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
                return r;
            }
        }
        WM_MOUSELEAVE => {
            crate::win::ui::tooltip::leave(e);
            crate::win::editor::checks::set_hot(e, None);
            pointer_out(h);
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            let mut p = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut p);
                ScreenToClient(e, &mut p);
            }
            // Once a move, before its WM_MOUSEMOVE (which then asks the same point again).
            if checkbox_now(e, p.x, p.y).is_some() {
                unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
                return 1;
            }
        }
        WM_NCDESTROY => unsafe {
            if take_picked(e).is_some() {
                rich::hold_selection_hidden(e, false);
            }
            crate::win::editor::checks::forget(e);
            crate::win::editor::view::forget(e);
            rich::forget_block_colors(e);
            rich::forget_box_colors(e);
            rich::forget_groups_of(e);
            rich::forget_doc(e);
            rich::forget_pictures(e);
            RemoveWindowSubclass(e, Some(edit_proc), 1);
        },
        _ => {}
    }
    // A press on a selected picture may be on a handle that resizes it.
    if msg == WM_LBUTTONDOWN {
        rich::picture_pressed(e);
    }
    // A key may draw nothing (most key-ups, the key-down of a letter, a caret moved by an
    // arrow): told apart by whether it changed the text, the selection or the view.
    let key = matches!(msg, WM_KEYDOWN | WM_KEYUP).then(|| view_of(e));
    let r = unsafe { DefSubclassProc(e, msg, wp, lp) };
    // Selecting by dragging may have scrolled the text under the pointer.
    if msg == WM_MOUSEMOVE && wp & 0x0001 != 0 {
        hover_forget();
    }
    // Resized by its handles (RichEdit may do the whole drag inside the press): an edit,
    // saved like typing.
    if matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONUP) && rich::picture_dragged(e, msg == WM_LBUTTONUP) {
        mark_dirty(h);
    }
    // Moved by a key or a click to in front of a checkbox: after it instead.
    let moved = matches!(wp as u16, VK_HOME | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT | VK_END);
    if (msg == WM_KEYDOWN && moved && !key_down(VK_SHIFT)) || msg == WM_LBUTTONUP {
        caret_off_box(e, msg == WM_KEYDOWN && wp as u16 == VK_LEFT);
    }
    // Quotes, code and tables stay as wide as the text column, and pictures within it.
    if msg == WM_SIZE {
        rich::style_blocks(e);
        rich::refit_picture_widths(e);
    }
    // Anything that may have changed the text, the selection or the view: the scroll bar
    // keeps in step, and a picked picture is let go once its selection has moved on. (What
    // RichEdit draws goes through its WM_PAINT, overlays and all: see `view`.)
    let drew = matches!(
        msg,
        WM_CHAR
            | WM_LBUTTONDOWN
            | WM_LBUTTONUP
            | WM_LBUTTONDBLCLK
            | WM_SETFOCUS
            | WM_KILLFOCUS
            | WM_VSCROLL
            | WM_MOUSEWHEEL
            | WM_TIMER
            | WM_SIZE
            | WM_IME_COMPOSITION
            | WM_IME_ENDCOMPOSITION
            | WM_CUT
            | WM_UNDO
            | rich::EM_REDO
            | rich::EM_SETSCROLLPOS
    ) || (msg == WM_MOUSEMOVE && wp & 0x0001 != 0)
        || key.is_some_and(|before| key_drew(msg, wp as u16, before, view_of(e)));
    if drew {
        super::scrollbar::sync(h);
    }
    picked_after(e, msg, drew);
    r
}

/// What a key can change that RichEdit draws: the text's length, the selection, the scroll
/// position.
fn view_of(e: HWND) -> (usize, (i32, i32), i32) {
    (rich::text_len(e), rich::sel(e), rich::scroll_y(e))
}

/// Whether key message `msg` (key `vk`) may have drawn, from what it changed (`before`,
/// `after`: `view_of`). A key that changed none of it drew nothing, but one with Ctrl
/// (other than a move, or Ctrl itself) may have changed formatting alone. A caret only
/// moved, by an arrow or the like, draws nothing else (`view::caret_moved` draws the caret
/// where it went). The overlays went on three times a keystroke (down, character, up),
/// each a read of the window.
fn key_drew(msg: u32, vk: u16, before: (usize, (i32, i32), i32), after: (usize, (i32, i32), i32)) -> bool {
    let moved = matches!(vk, VK_HOME | VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT | VK_END);
    let modifier = matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN | VK_CAPITAL);
    if msg == WM_KEYDOWN && key_down(VK_CONTROL) && !moved && !modifier {
        return true;
    }
    if before == after {
        return false;
    }
    let caret = |s: (i32, i32)| s.0 == s.1;
    !(moved && before.0 == after.0 && before.2 == after.2 && caret(before.1) && caret(after.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, ES_MULTILINE, WS_POPUP};

    /// A picture picked by a click is selected with RichEdit's highlight hidden (it drew
    /// the picture as its colour negative), and once the selection moves on the highlight
    /// is RichEdit's again.
    #[test]
    fn a_picked_picture_is_framed_not_inverted() {
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 400, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        rich::insert_text(e, "T\r\rafter");
        rich::set_sel(e, 2, 2);
        let link = "{\\field{\\*\\fldinst{HYPERLINK \"attachment:p.png\"}}{\\fldrslt{\\u8203?}}}";
        rich::insert_rtf(e, &format!("{{\\rtf1 {}{link}}}", rich::test_picture(48, 24, 720, 360)));
        let t = rich::text(e);
        let obj = t.iter().position(|&c| c == 0xFFFC).unwrap() as i32;
        let end = rich::para_end(&t, obj);
        pick(e, obj, end);
        assert_eq!(rich::sel(e), (obj, end));
        assert_eq!(PICKED.with(|p| p.get()), Some((e as isize, (obj, end), obj, (720, 360))));
        assert!(rich::selection_held_hidden(e));
        // Drawn again, with the selection where it was: still picked.
        picked_after(e, WM_PAINT, true);
        assert!(PICKED.with(|p| p.get()).is_some());
        // Restyling that hides the selection while it works leaves it hidden.
        rich::list_level(e);
        assert!(rich::selection_held_hidden(e));
        // The focus back: still picked, still hidden.
        picked_after(e, WM_SETFOCUS, true);
        assert!(PICKED.with(|p| p.get()).is_some());
        assert!(rich::selection_held_hidden(e));
        // The caret goes elsewhere: let go.
        rich::set_sel(e, 0, 0);
        picked_after(e, 0x0437, false);
        assert_eq!(PICKED.with(|p| p.get()), None);
        assert!(!rich::selection_held_hidden(e));
        // A line without a picture (a file's chip) is just selected.
        pick(e, 0, 1);
        assert_eq!(PICKED.with(|p| p.get()), None);
        unsafe { DestroyWindow(e) };
    }

    /// A checklist note in a hidden editor, `w` x `h`, its caret drawn by the view and its
    /// keys taken by `edit_proc` (with no note window around it).
    fn typing_editor(w: i32, h: i32) -> HWND {
        let e = unsafe {
            LoadLibraryW(crate::win::util::wide("Msftedit.dll").as_ptr());
            CreateWindowExW(0, crate::win::util::wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, w, h, null_mut(), null_mut(), null_mut(), null_mut())
        };
        rich::setup(e);
        let mut s = String::from("Garden \u{1F331}\rSome words to start with");
        for i in 1..=12 {
            s.push_str(&format!("\r\u{2610} item number {i} \u{1F680}"));
        }
        rich::insert_text(e, &s);
        rich::apply_base_format(e, 0x1D1D1F, 0xF3E9C6);
        rich::style_boxes(e);
        rich::restyle(e);
        crate::win::editor::checks::set_style(e, 0x303030, 0xF3E9C6, 1.0);
        crate::win::editor::view::own_caret(e);
        unsafe { SetWindowSubclass(e, Some(edit_proc), 1, 0) };
        e
    }

    /// Editor `e`'s caret as it should be: (RichEdit's own caret, which it places at the
    /// selection, unseen; the selection's end where RichEdit says it is).
    fn system_caret(e: HWND) -> (RECT, POINT) {
        let mut g: GUITHREADINFO = unsafe { std::mem::zeroed() };
        g.cbSize = size_of::<GUITHREADINFO>() as u32;
        unsafe { GetGUIThreadInfo(windows_sys::Win32::System::Threading::GetCurrentThreadId(), &mut g) };
        assert_eq!(g.hwndCaret, e);
        // Kept unseen: the view draws it.
        assert_eq!(g.flags & GUI_CARETBLINKING, 0, "Windows' caret shows");
        (g.rcCaret, rich::pos_from_char(e, rich::sel(e).1))
    }

    /// The caret is drawn in the editor's frame, one bar where RichEdit's caret goes, and
    /// never left behind. After every key (letters, spaces, Backspace, Enter, undo and
    /// redo, a paste, arrows, a selection, a scroll, the focus out and back): Windows'
    /// caret (unseen, for screen readers) is at the selection's end, the frame differs from
    /// the editor drawn without a caret in exactly the caret's bar there (none with nothing
    /// to show it, or without the focus), and the window, painted only the rows each frame
    /// changed and the caret's pixels at a blink, shows just what a fresh frame does.
    #[test]
    fn the_caret_is_one_bar_where_the_selection_is() {
        use crate::win::editor::view::{CARET_COLOR, Parts, Screen, compose, frame_now};
        let (w, h) = (360, 240);
        let e = typing_editor(w, h);
        let send = |msg: u32, wp: usize| unsafe { SendMessageW(e, msg, wp, 0) };
        send(WM_SETFOCUS, 0);
        // At the end of the second line.
        let t = rich::text(e);
        let end = t.iter().enumerate().filter(|&(_, &c)| c == rich::CR).nth(1).unwrap().0 as i32;
        rich::set_sel(e, end, end);
        let mut screen = Screen::new(e);
        let bare = || {
            let mut c = crate::win::render::gfx::Canvas::new(w, h);
            assert!(compose(e, &mut c, 0, w, h, Parts::All));
            c.copy_out(RECT { left: 0, top: 0, right: w, bottom: h })
        };
        let (mut carets, mut blinks) = (0, 0);
        let mut check = |screen: &mut Screen, step: &str, focused: bool| {
            screen.present(e);
            let (px, at, on) = frame_now(e);
            let plain = bare();
            let (a, b) = rich::sel(e);
            // The editor drawn without a caret, and with one bar of the caret at `at`.
            let drawn = at.map(|r| {
                let mut d = plain.clone();
                for y in r.top..r.bottom {
                    d[(y * w + r.left) as usize..(y * w + r.right) as usize].fill(CARET_COLOR);
                }
                d
            });
            match at {
                Some(r) => {
                    assert!(focused && a == b, "{step}: a caret");
                    let (sys, end) = system_caret(e);
                    let mut caret = POINT { x: 0, y: 0 };
                    unsafe { GetCaretPos(&mut caret) };
                    // Where RichEdit's caret is, and as tall; Windows' caret there too.
                    assert_eq!((r.left, r.top, r.bottom), (sys.left, sys.top.max(0), sys.bottom.min(h)), "{step}");
                    assert_eq!((caret.x, caret.y), (sys.left, sys.top), "{step}");
                    assert_eq!(caret.x, end.x, "{step}: at the selection's end");
                    assert!(r.right - r.left >= 1 && r.bottom - r.top >= 10, "{step}: a bar");
                    // One bar, all of it the caret's, and nothing else of it anywhere (none
                    // in the hidden half of a blink).
                    assert!(px == if on { drawn.clone().unwrap() } else { plain.clone() }, "{step}: not one caret");
                    carets += 1;
                }
                None => {
                    assert!(!focused || a != b, "{step}: no caret");
                    assert!(px == plain, "{step}: caret pixels with no caret");
                }
            }
            // The window shows the frame: nothing left behind.
            assert!(screen.px == px, "{step}: the window differs from a fresh frame");
            // A blink takes it away (or brings it back) and back again, and only it.
            if let Some(drawn) = drawn {
                let (first, second) = if on { (&plain, &drawn) } else { (&drawn, &plain) };
                screen.blink(e);
                assert!(screen.px == *first, "{step}: the window after a blink");
                screen.blink(e);
                assert!(screen.px == *second, "{step}: the window after two blinks");
                blinks += 1;
                // Every other time left as the blink after leaves it, for the next key.
                if blinks % 2 == 0 {
                    screen.blink(e);
                }
            }
        };
        check(&mut screen, "start", true);
        let key = |vk: u16, ch: Option<u16>| {
            send(WM_KEYDOWN, vk as usize);
            if let Some(c) = ch {
                send(WM_CHAR, c as usize);
            }
            send(WM_KEYUP, vk as usize);
        };
        // Thirty-odd characters, spaces among them (one at the line's end, two together).
        for (i, c) in " quick brown fox  jumps over the".encode_utf16().enumerate() {
            let vk = if c == b' ' as u16 { VK_SPACE } else { (c as u8).to_ascii_uppercase() as u16 };
            let at = |r: Option<RECT>| r.map(|r| (r.left, r.top));
            let before = at(frame_now(e).1);
            key(vk, Some(c));
            // Every character moves the caret on at once, a space too (along its line, or
            // to the next one where the line wraps).
            let after = at(frame_now(e).1);
            assert!(after.zip(before).is_some_and(|(a, b)| a.0 > b.0 || a.1 > b.1), "typed {i}: the caret moved on, {before:?} to {after:?}");
            check(&mut screen, &format!("typed {i}"), true);
        }
        for i in 0..4 {
            key(VK_BACK, Some(0x08));
            check(&mut screen, &format!("backspace {i}"), true);
        }
        key(VK_RETURN, Some(0x0D));
        check(&mut screen, "enter", true);
        for c in "new".encode_utf16() {
            key((c as u8).to_ascii_uppercase() as u16, Some(c));
            check(&mut screen, "typed on the new line", true);
        }
        send(WM_UNDO, 0);
        check(&mut screen, "undo", true);
        send(rich::EM_REDO, 0);
        check(&mut screen, "redo", true);
        // A paste's worth of text at once.
        rich::replace_sel(e, "pasted text");
        crate::win::editor::view::caret_moved(e);
        check(&mut screen, "paste", true);
        key(VK_LEFT, None);
        check(&mut screen, "left", true);
        key(VK_HOME, None);
        check(&mut screen, "home", true);
        key(VK_END, None);
        check(&mut screen, "end", true);
        key(VK_UP, None);
        check(&mut screen, "up", true);
        // A selection: no caret; collapsed again: one.
        let (a, _) = rich::sel(e);
        unsafe { SendMessageW(e, 0x00B1, (a - 3) as usize, a as isize) };
        crate::win::editor::view::caret_moved(e);
        check(&mut screen, "selected", true);
        rich::set_sel(e, a, a);
        crate::win::editor::view::caret_moved(e);
        check(&mut screen, "collapsed", true);
        // Scrolled, the caret with its line.
        unsafe { SendMessageW(e, rich::EM_SETSCROLLPOS, 0, &POINT { x: 0, y: 40 } as *const _ as isize) };
        check(&mut screen, "scrolled", true);
        // The focus away and back.
        send(WM_KILLFOCUS, 0);
        check(&mut screen, "focus out", false);
        send(WM_SETFOCUS, 0);
        check(&mut screen, "focus in", true);
        assert!(carets >= 50 && blinks >= 50, "{carets} carets, {blinks} blinks");
        unsafe { DestroyWindow(e) };
    }

    /// A picture's frame is as tall as the picture at the width it's drawn, not to its
    /// line's baseline (which RichEdit sets a little above the picture's bottom).
    #[test]
    fn a_picture_frame_is_as_tall_as_its_picture() {
        assert_eq!(picture_bottom(5, 240, (3600, 2400)), 165);
        assert_eq!(picture_bottom(0, 100, (720, 360)), 50);
        // Narrowed to fit: the same shape.
        assert_eq!(picture_bottom(10, 120, (3600, 2400)), 90);
        // Nothing to divide by: square.
        assert_eq!(picture_bottom(0, 30, (0, 0)), 30);
    }
}
