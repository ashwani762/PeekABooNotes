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
        Btn::Done => (t!("tip_done"), keys(&["ctrl", "shift", "A"])),
        Btn::Delete => (t!("menu_delete"), keys(&["ctrl", "shift", "del"])),
        Btn::Pin => (if pinned { t!("tip_unpin") } else { t!("menu_keep_on_top") }, keys(&["ctrl", "shift", "P"])),
        Btn::Color => (t!("tip_color"), keys(&["ctrl", "."])),
        Btn::More => (t!("tip_more"), String::new()),
        _ => (t!("tip_close"), keys(&["esc"])),
    };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    if let Some((_, r)) = buttons(rc.right, st.s).into_iter().find(|(x, _)| *x == b) {
        tooltip::hover(h, r, label, &k);
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
    // The right margin is also where the scroll bar runs.
    let (l, r, b) = if st.embedded { (0, (12.0 * s) as i32, 0) } else { ((16.0 * s) as i32, (10.0 * s) as i32, (30.0 * s) as i32) };
    let top = if st.embedded { 0 } else { header_h(s) + (2.0 * s) as i32 };
    unsafe { MoveWindow(st.edit, l, top, (rc.right - l - r).max(1), (rc.bottom - top - b).max(1), 1) };
}

pub(super) fn paint(h: HWND) {
    let Some(st) = state(h) else { return };
    let dark = app::dark();
    let (color, pinned, updated, blank) = app::store(|s| s.note(st.id).map(|n| (n.color, n.pinned, n.updated, n.text.trim().is_empty()))).unwrap_or((0, false, 0, true));
    let paper = theme::paper(color, dark);
    let ink = theme::paper_ink(color, dark);
    let a = crate::win::render::anim::clamp01(st.ctl.value);
    let s = st.s;
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
                c.text(&meta, meta_f, fr, theme::paper_subtext(color, dark), a * 0.85, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
            }
        }
        c.flush();
        super::scrollbar::paint(h, st, &mut c, ink);
        c.blit(hdc, 0, 0);
        st.canvas = Some(c);
        EndPaint(h, &ps);
    }
}

pub(super) fn hit_test(h: HWND, lp: LPARAM) -> LRESULT {
    let Some(st) = state(h) else { return HTCLIENT as LRESULT };
    let mut p = POINT { x: x_of(lp), y: y_of(lp) };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe {
        ScreenToClient(h, &mut p);
        GetClientRect(h, &mut rc);
    }
    // The scroll bar wins over the resize edge it runs along.
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
fn hides(h: HWND) -> bool {
    let Some(st) = state(h) else { return false };
    let id = st.id;
    !st.embedded && !app::store(|s| s.d.settings.keep_open || s.note(id).is_some_and(|n| n.pinned))
}

/// The note lost the foreground to `to`: unless that's part of the note (a dialog it
/// owns), start watching for the moment to go.
pub(super) fn hide_later(h: HWND, to: HWND) {
    if !hides(h) || (!to.is_null() && unsafe { GetWindow(to, GW_OWNER) } == h) {
        return;
    }
    unsafe { SetTimer(h, T_HIDE, 300, None) };
}

/// Go once the pointer is off the note with no button held: someone still pointing at it,
/// or dragging something onto it from another app, isn't done with it. While the pointer
/// rests on the note the timer stops; `pointer_out` starts it again when it leaves.
pub(super) fn hide_tick(h: HWND) {
    let fg = unsafe { GetForegroundWindow() };
    let ours = fg == h || (!fg.is_null() && unsafe { GetWindow(fg, GW_OWNER) } == h) || crate::win::ui::menu::is_menu(fg);
    if ours || !hides(h) {
        unsafe { KillTimer(h, T_HIDE) };
        return;
    }
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    let held = unsafe { GetAsyncKeyState(VK_LBUTTON as i32) as u16 & 0x8000 != 0 || GetAsyncKeyState(VK_RBUTTON as i32) as u16 & 0x8000 != 0 };
    if held {
        return; // keep looking: a drag may end here or elsewhere
    }
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
