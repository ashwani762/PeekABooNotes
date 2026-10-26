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
                rich::set_sel(e, start as i32, b);
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
            link_tip(e, x_of(lp), y_of(lp));
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
            crate::win::editor::checks::forget(e);
            crate::win::editor::fade::forget(e);
            rich::forget_block_colors(e);
            rich::forget_box_colors(e);
            rich::forget_groups_of(e);
            rich::forget_doc(e);
            rich::forget_pictures(e);
            RemoveWindowSubclass(e, Some(edit_proc), 1);
        },
        _ => {}
    }
    // Which fade bands RichEdit is about to paint over (they have to be faded again after;
    // the others are left alone).
    let scale = crate::win::editor::checks::paper(e).map_or(1.0, |(_, s)| s);
    let painted_bands = if msg == WM_PAINT {
        let mut u: RECT = unsafe { std::mem::zeroed() };
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetClientRect(e, &mut rc) };
        if unsafe { GetUpdateRect(e, &mut u, 0) } != 0 { crate::win::editor::fade::reach(u.top, u.bottom, rc.bottom, scale) } else { 0 }
    } else {
        0
    };
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
    // RichEdit draws outside WM_PAINT too (typing, selecting, scrolling): put the
    // checkboxes back over their placeholder glyphs after anything that may have drawn.
    let drew = matches!(
        msg,
        WM_PAINT
            | WM_CHAR
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
            // Scrolled (the wheel's glide sets the position every frame): RichEdit draws the
            // rows coming into view right away, so the circles go on them right away too.
            | rich::EM_SETSCROLLPOS
    ) || (msg == WM_MOUSEMOVE && wp & 0x0001 != 0)
        || key.is_some_and(|before| key_drew(msg, wp as u16, before, view_of(e)));
    if drew {
        // The text on screen, read once for both.
        let (t, a0, b0) = rich::screen_text(e);
        crate::win::editor::checks::paint_in(e, &t, a0, b0);
        crate::win::editor::emoji::paint_in(e, &t, a0, b0);
        if let Some((paper, s)) = crate::win::editor::checks::paper(e) {
            use crate::win::editor::fade;
            // Scrolling may have drawn anywhere. Typing, selecting and the like draw around
            // the selection: a band only when that's at its edge.
            let touched = match msg {
                WM_PAINT => painted_bands,
                WM_VSCROLL | WM_MOUSEWHEEL | rich::EM_SETSCROLLPOS => fade::TOP | fade::BOTTOM,
                _ => {
                    let mut rc: RECT = unsafe { std::mem::zeroed() };
                    unsafe { GetClientRect(e, &mut rc) };
                    let (a, b) = rich::sel(e);
                    let (ya, yb) = (rich::pos_from_char(e, a).y, rich::pos_from_char(e, b).y);
                    fade::reach(ya.min(yb) - (24.0 * s) as i32, ya.max(yb) + (40.0 * s) as i32, rc.bottom, s)
                }
            };
            fade::paint(e, paper, s, touched);
        }
        super::scrollbar::sync(h);
    }
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
/// moved, by an arrow or the like, draws nothing either: the caret is Windows' own. The
/// overlays went on three times a keystroke (down, character, up), each a read of the
/// window.
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
