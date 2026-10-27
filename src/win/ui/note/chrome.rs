// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note window's own chrome: header buttons, footer, painting and hit testing.

use super::*;

pub(super) fn header_h(s: f32) -> i32 {
    (36.0 * s) as i32
}

pub(super) fn buttons(cw: i32, s: f32) -> [(Btn, RECT); 6] {
    let (bw, bh) = ((28.0 * s) as i32, (24.0 * s) as i32);
    let top = (header_h(s) - bh) / 2;
    let right = cw - (6.0 * s) as i32;
    let at = |i: i32| rect(right - bw * (i + 1), top, bw, bh);
    [(Btn::Close, at(0)), (Btn::More, at(1)), (Btn::Color, at(2)), (Btn::Pin, at(3)), (Btn::Delete, at(4)), (Btn::Done, at(5))]
}

/// Show (or move along) the tooltip for header button `b`; `Btn::None` lets it go.
pub(super) fn tip_for(h: HWND, b: Btn) {
    use crate::common::i18n::keys;
    use crate::win::ui::tooltip;
    let Some(st) = state(h) else { return };
    if b == Btn::None || st.embedded {
        tooltip::leave(h);
        return;
    }
    let pinned = app::store(|s| s.note(st.id).map(|n| n.pinned)).unwrap_or(false);
    let (label, k) = match b {
        Btn::Done => (t!("action_archive"), keys(&["ctrl", "shift", "A"])),
        Btn::Delete => (t!("menu_delete"), keys(&["ctrl", "shift", "del"])),
        Btn::Pin => (if pinned { t!("tip_unpin") } else { t!("menu_keep_on_top") }, keys(&["ctrl", "shift", "P"])),
        Btn::Color => (t!("tip_color"), keys(&["ctrl", "."])),
        Btn::More => (t!("tip_more"), String::new()),
        _ => (t!("tip_close"), keys(&["esc"])),
    };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    if let Some((_, r)) = buttons(rc.right, st.s).into_iter().find(|(x, _)| *x == b) {
        // Above the buttons when there's room: under them is the title.
        tooltip::hover_over(h, r, label, &k);
    }
}

pub(super) fn btn_at(h: HWND, x: i32, y: i32) -> Btn {
    let Some(st) = state(h) else { return Btn::None };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    buttons(rc.right, st.s).into_iter().find(|(_, r)| crate::win::util::pt_in(r, x, y)).map(|(b, _)| b).unwrap_or(Btn::None)
}

pub(super) fn layout(h: HWND) {
    let Some(st) = state(h) else { return };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    let s = st.s;
    // The right margin is also where the scroll bar runs. In a note window it is the left
    // margin plus the selection bar RichEdit insets the text by on the left, so
    // right-aligned text sits as far from the right edge as left-aligned text from the left.
    let (l, r, b) = if st.embedded {
        (0, (12.0 * s) as i32, 0)
    } else {
        ((16.0 * s) as i32, ((16.0 + rich::SELECTION_BAR) * s) as i32, (30.0 * s) as i32)
    };
    let top = if st.embedded { 0 } else { header_h(s) + (2.0 * s) as i32 };
    unsafe { MoveWindow(st.edit, l, top, (rc.right - l - r).max(1), (rc.bottom - top - b).max(1), 1) };
}

pub(super) fn paint(h: HWND) {
    let Some(st) = state(h) else { return };
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(h, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        // The same canvas every paint (a hover fade paints 60 times a second); a new one
        // only when the window has changed size.
        let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
        if st.canvas.as_ref().map(|c| (c.w, c.h)) != Some((cw, ch)) {
            st.canvas = Some(Canvas::new(cw, ch));
        }
        let mut c = st.canvas.take().unwrap();
        draw(h, st, &mut c, rc);
        c.blit(hdc, 0, 0);
        st.canvas = Some(c);
        EndPaint(h, &ps);
    }
}

/// The window's own part of it (all but the text: its header, footer and scroll bar) as it
/// is now, on `c`, the size of its client area `rc`: what it paints, and what its picture
/// for a flying card is made of.
pub(super) fn draw(h: HWND, st: &NoteWin, c: &mut Canvas, rc: RECT) {
    let dark = app::dark();
    let (color, pinned, updated, blank) = app::store(|s| s.note(st.id).map(|n| (n.color, n.pinned, n.updated, n.text.trim().is_empty()))).unwrap_or((0, false, 0, true));
    let paper = theme::paper(color, dark);
    let ink = theme::paper_ink(color, dark);
    let a = crate::win::render::anim::clamp01(st.ctl.value);
    let s = st.s;
    c.clear(0xFF00_0000 | paper);
    c.begin_batch();
    if let (false, true, Some(icons), Some(meta_f)) = (st.embedded, a > 0.01 || pinned, st.icons.as_ref(), st.meta.as_ref()) {
        for (b, r) in buttons(rc.right, s) {
            let a = if b == Btn::Pin && pinned { a.max(0.55) } else { a };
            if a <= 0.01 {
                continue;
            }
            let (bx, by, bw, bh) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
            // Delete warns in red when aimed at.
            let danger = if dark { 0xFF453A } else { 0xD70015 };
            let hot = if st.hot == b { st.hot_a.value } else { 0.0 };
            if hot > 0.0 {
                let tint = if b == Btn::Delete { danger } else { ink };
                c.shape(&Shape::new(bx, by, bw, bh, 7.0 * s), tint, 0.13 * hot * a, 1.0, 0.0);
            }
            match b {
                Btn::Color => {
                    let d = 11.0 * s;
                    let dot = Shape::new(bx + (bw - d) / 2.0, by + (bh - d) / 2.0, d, d, d / 2.0);
                    c.shape(&dot, theme::color(color).accent, a, 1.0, 0.0);
                    c.shape(&dot, 0x000000, 0.18 * a, 1.0, 1.0);
                }
                _ => {
                    let glyph = match b {
                        Btn::Done => "\u{E73E}",
                        Btn::Delete => "\u{E74D}",
                        Btn::Pin => if pinned { "\u{E841}" } else { "\u{E718}" },
                        Btn::More => "\u{E712}",
                        _ => "\u{E8BB}",
                    };
                    let rgb = if b == Btn::Delete { mix(ink, danger, hot) } else { ink };
                    c.text(glyph, icons, r, rgb, a * 0.9, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    if b == Btn::Pin && pinned {
                        // The font's solid pin has no needle: draw the outline pin over
                        // it, so the filled head keeps its point.
                        c.text("\u{E718}", icons, r, ink, a * 0.9, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    }
                }
            }
        }
        // A blank note hasn't been edited yet. A save that failed is said in place
        // of the edited time, for as long as it goes on failing.
        let unsaved = app::save_failed();
        if !blank || st.full_at.is_some() || unsaved {
            let fr = rect((25.0 * s) as i32, rc.bottom - (26.0 * s) as i32, rc.right - (43.0 * s) as i32, (18.0 * s) as i32);
            let meta = if unsaved {
                t!("note_save_failed").to_string()
            } else if st.full_at.is_some() {
                t!("note_full").to_string()
            } else {
                t!("note_edited", crate::common::i18n::time::edited_time(updated))
            };
            // At full strength once shown: the colour is the one tuned to read at 4.5:1.
            c.text(&meta, meta_f, fr, theme::paper_subtext(color, dark), a, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        }
    }
    c.flush();
    super::scrollbar::paint(h, st, c, ink);
}

pub(super) fn hit_test(h: HWND, lp: LPARAM) -> LRESULT {
    let Some(st) = state(h) else { return HTCLIENT as LRESULT };
    let mut p = POINT { x: x_of(lp), y: y_of(lp) };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        ScreenToClient(h, &mut p);
        GetClientRect(h, &mut rc);
    }
    // The scroll bar wins over the resize edge it runs along, all but the outermost few
    // pixels, where the window is still resized beside it.
    if super::scrollbar::on_bar(h, p.x, p.y) {
        return HTCLIENT as LRESULT;
    }
    let b = (6.0 * st.s) as i32;
    let (l, r, t, bo) = (p.x < b, p.x >= rc.right - b, p.y < b, p.y >= rc.bottom - b);
    let code = match (l, r, t, bo) {
        (true, _, true, _) => HTTOPLEFT,
        (_, true, true, _) => HTTOPRIGHT,
        (true, _, _, true) => HTBOTTOMLEFT,
        (_, true, _, true) => HTBOTTOMRIGHT,
        (true, ..) => HTLEFT,
        (_, true, ..) => HTRIGHT,
        (_, _, true, _) => HTTOP,
        (_, _, _, true) => HTBOTTOM,
        _ if p.y < header_h(st.s) => {
            if btn_at(h, p.x, p.y) != Btn::None { HTCLIENT } else { HTCAPTION }
        }
        _ => HTCLIENT,
    };
    code as LRESULT
}

/// Whether this note flies back to the deck when you click away: unpinned, a window of
/// its own, and the setting on.
pub(super) fn hides(h: HWND) -> bool {
    let Some(st) = state(h) else { return false };
    let id = st.id;
    !st.embedded && !app::store(|s| s.d.settings.keep_open || s.note(id).is_some_and(|n| n.pinned))
}

/// The note lost the foreground to `to`: unless that's part of the note (a dialog it
/// owns), start watching for the moment to go. A click on another app's window brings
/// that window over the note at once: the note is kept on top of it, in view, so it flies
/// home from where it's seen as the click ends, rather than vanishing behind the window
/// and a card coming out over it after.
pub(super) fn hide_later(h: HWND, to: HWND) {
    if !hides(h) {
        // Kept open (pinned, or the setting changed while it was in use): among the
        // other windows again, behind the one that took the foreground.
        lower_behind(h, to);
        return;
    }
    if !to.is_null() && unsafe { GetWindow(to, GW_OWNER) } == h {
        return;
    }
    // On top of every other window since it was taken up (see `use_note`), so the window
    // clicked never covers it, even for the frame or two before this. Not when the
    // foreground went to one of ours (another note opening, the deck, a menu): kept up
    // then, it came up over the deck card flying open, and for a few frames the peek
    // seemed to blink off; nor while the pointer is on it.
    if pointer_on(h) || ours(to) {
        lower_behind(h, to);
    }
    // A click away: it goes as the button comes up, told so at once (`watch_release`).
    if button_held() {
        watch_release(h);
    }
    // Looked at about every refresh: a click away flies the note home as soon as the button
    // is up (every 40 ms, it waited 20 ms on average, up to 40, before setting off).
    unsafe { SetTimer(h, T_HIDE, 16, None) };
}

/// Whether window `w` is this app's.
fn ours(w: HWND) -> bool {
    let mut pid = 0;
    !w.is_null() && unsafe { GetWindowThreadProcessId(w, &mut pid) } != 0 && pid == unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() }
}

/// Whether the pointer is over the note's window.
fn pointer_on(h: HWND) -> bool {
    let mut p = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        GetCursorPos(&mut p);
        GetWindowRect(h, &mut r);
    }
    crate::win::util::pt_in(&r, p.x, p.y)
}

/// On top of every other window until it goes (or `lower` puts it back), but under the
/// app's own that stay on top (the deck, a toast, a menu, a tooltip): at the very top, a
/// note in use covered the deck fanned out beside it.
fn raise(h: HWND) {
    let Some(st) = state(h) else { return };
    st.raised = true;
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    unsafe {
        SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, flags);
        let under = lowest_of(&[crate::win::ui::deck::window(), crate::win::ui::toast::window(), crate::win::ui::menu::window(), crate::win::ui::tooltip::window()]);
        if !under.is_null() && under != h {
            SetWindowPos(h, under, 0, 0, 0, 0, flags);
        }
    }
}

/// The lowest of `windows` (those showing, on top of all) among the windows on top of all.
fn lowest_of(windows: &[HWND]) -> HWND {
    let mut lowest = null_mut();
    unsafe {
        let mut w = GetTopWindow(null_mut());
        while !w.is_null() && GetWindowLongPtrW(w, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST != 0 {
            if windows.contains(&w) && IsWindowVisible(w) != 0 {
                lowest = w;
            }
            w = GetWindow(w, GW_HWNDNEXT);
        }
    }
    lowest
}

/// The note was taken up (activated). One that flies home when you click away goes on top
/// of every other window now, while it's in use: lifted only once it had lost the
/// foreground, it was too late, the window clicked already drawn over it, and the whole
/// note vanished for a frame before coming back to fly home. One kept open stays among the
/// other windows.
pub(super) fn use_note(h: HWND) {
    if hides(h) {
        raise(h);
    } else {
        lower(h);
    }
}

/// Back among the other windows after `raise`, unless it's pinned (on top anyway).
pub(super) fn lower(h: HWND) {
    let Some(st) = state(h) else { return };
    if !st.raised {
        return;
    }
    st.raised = false;
    let id = st.id;
    if !app::store(|s| s.note(id).is_some_and(|n| n.pinned)) {
        unsafe { SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }
}

/// `lower`, and behind `to` (the window that took the foreground), as it would be had it
/// never been lifted: put back among the other windows, it went above all of them.
fn lower_behind(h: HWND, to: HWND) {
    lower(h);
    let pinned = state(h).is_some_and(|st| app::store(|s| s.note(st.id).is_some_and(|n| n.pinned)));
    // Behind a window on top of all (the deck, a menu) it would be lifted too.
    let top = !to.is_null() && unsafe { GetWindowLongPtrW(to, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
    if !pinned && !to.is_null() && !top && unsafe { IsWindowVisible(to) } != 0 {
        unsafe { SetWindowPos(h, to, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) };
    }
}

/// Go once the pointer is off the note with no button held: someone still pointing at it,
/// or dragging something onto it from another app, isn't done with it. While the pointer
/// rests on the note the timer stops; `pointer_out` starts it again when it leaves.
/// Whether a mouse button is down (as the system has it, wherever the pointer is).
fn button_held() -> bool {
    unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 || GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000 != 0 }
}

thread_local! {
    /// Notes waiting for the button clicked in another window to come up, and the hook
    /// that tells them: (hook, notes, released since).
    static RELEASE: std::cell::RefCell<(isize, Vec<HWND>, bool)> = const { std::cell::RefCell::new((0, Vec::new(), false)) };
}

/// Have note `h` told the moment a mouse button comes up anywhere (a low-level mouse hook,
/// only while a note waits on a click away): looked for every 16 ms instead, which Windows
/// keeps to its 15.6 ms tick, it set off up to 31 ms after the button came up.
fn watch_release(h: HWND) {
    unsafe extern "system" fn hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code >= 0 {
            match wp as u32 {
                WM_LBUTTONUP | WM_RBUTTONUP => {
                    let notes = RELEASE.with(|r| {
                        let mut r = r.borrow_mut();
                        r.2 = true;
                        r.1.clone()
                    });
                    for n in notes {
                        unsafe { PostMessageW(n, WM_TIMER, T_HIDE, 0) };
                    }
                }
                WM_LBUTTONDOWN | WM_RBUTTONDOWN => RELEASE.with(|r| r.borrow_mut().2 = false),
                _ => {}
            }
        }
        unsafe { CallNextHookEx(null_mut(), code, wp, lp) }
    }
    RELEASE.with(|r| {
        let mut r = r.borrow_mut();
        if !r.1.contains(&h) {
            r.1.push(h);
        }
        if r.0 == 0 {
            r.2 = false;
            r.0 = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook), app::hinst(), 0) } as isize;
        }
    });
}

/// Note `h` waits on no button any more (gone, or staying); the hook goes with the last.
pub(super) fn unwatch_release(h: HWND) {
    RELEASE.with(|r| {
        let mut r = r.borrow_mut();
        r.1.retain(|&n| n != h);
        if r.1.is_empty() && r.0 != 0 {
            unsafe { UnhookWindowsHookEx(r.0 as HHOOK) };
            r.0 = 0;
        }
    });
}

/// Whether the button held when a note began waiting on a click away has come up (the
/// hook saw it), whatever the async state says yet.
fn released() -> bool {
    RELEASE.with(|r| r.borrow().2)
}

pub(super) fn hide_tick(h: HWND) {
    let fg = unsafe { GetForegroundWindow() };
    // The deck holding the keys (Ctrl+Alt+K from this note) or one of our menus has them
    // for a moment: they come back here (Esc gives them back) or go on elsewhere, so keep
    // looking until they do.
    if hides(h) && (crate::win::ui::deck::holds_keys(fg) || crate::win::ui::menu::is_menu(fg)) {
        return;
    }
    let ours = fg == h || (!fg.is_null() && unsafe { GetWindow(fg, GW_OWNER) } == h);
    if ours || !hides(h) {
        unsafe { KillTimer(h, T_HIDE) };
        unwatch_release(h);
        lower(h);
        return;
    }
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    if button_held() && !released() {
        return; // keep looking: a drag may end here or elsewhere
    }
    unwatch_release(h);
    // On the note itself (not on a window lying over it): wait for it to leave.
    let under = unsafe { WindowFromPoint(p) };
    if !under.is_null() && (under == h || unsafe { IsChild(h, under) } != 0) {
        unsafe { KillTimer(h, T_HIDE) };
        return;
    }
    unsafe {
        KillTimer(h, T_HIDE);
        PostMessageW(h, WM_CLOSE, 0, 0);
    }
}

pub(super) fn click(h: HWND, b: Btn) {
    let Some(st) = state(h) else { return };
    let id = st.id;
    match b {
        Btn::Close => unsafe {
            PostMessageW(h, WM_CLOSE, 0, 0);
        },
        Btn::Pin => app::note_command(id, NC_PIN),
        // Posted: both take this window away, which mustn't happen inside its own click.
        Btn::Done => unsafe {
            PostMessageW(h, WM_NOTE_CMD, NC_ARCHIVE as usize, 0);
        },
        Btn::Delete => unsafe {
            PostMessageW(h, WM_NOTE_CMD, NC_DELETE as usize, 0);
        },
        Btn::Color => {
            let mut rc: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(h, &mut rc) };
            let r = buttons(rc.right, st.s)[2].1;
            let mut p = POINT { x: r.left, y: r.bottom };
            unsafe { ClientToScreen(h, &mut p) };
            let cmd = app::popup(app::color_menu(id), p.x, p.y, h);
            if cmd != 0 {
                app::note_command(id, cmd);
            }
        }
        Btn::More => {
            let mut rc: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(h, &mut rc) };
            let r = buttons(rc.right, st.s)[1].1;
            let mut p = POINT { x: r.left, y: r.bottom };
            unsafe { ClientToScreen(h, &mut p) };
            let cmd = app::popup(app::note_menu(id, true), p.x, p.y, h);
            if cmd != 0 {
                unsafe { PostMessageW(h, WM_NOTE_CMD, cmd as usize, 0) };
            }
        }
        Btn::None => {}
    }
}
