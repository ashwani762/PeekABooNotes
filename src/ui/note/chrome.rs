//! The note window's own chrome: header buttons, footer, painting and hit testing.

use super::*;

pub(super) fn header_h(s: f32) -> i32 {
    (36.0 * s) as i32
}

pub(super) fn buttons(cw: i32, s: f32) -> [(Btn, RECT); 4] {
    let (bw, bh) = ((28.0 * s) as i32, (24.0 * s) as i32);
    let top = (header_h(s) - bh) / 2;
    let right = cw - (6.0 * s) as i32;
    let at = |i: i32| rect(right - bw * (i + 1), top, bw, bh);
    [(Btn::Close, at(0)), (Btn::More, at(1)), (Btn::Color, at(2)), (Btn::Pin, at(3))]
}

pub(super) fn btn_at(h: HWND, x: i32, y: i32) -> Btn {
    let Some(st) = state(h) else { return Btn::None };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut rc) };
    buttons(rc.right, st.s).into_iter().find(|(_, r)| crate::platform::util::pt_in(r, x, y)).map(|(b, _)| b).unwrap_or(Btn::None)
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
    let (color, pinned, updated) = app::store(|s| s.note(st.id).map(|n| (n.color, n.pinned, n.updated))).unwrap_or((0, false, 0));
    let paper = theme::paper(color, dark);
    let ink = theme::paper_ink(color, dark);
    let a = crate::render::anim::clamp01(st.ctl.value);
    let s = st.s;
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(h, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let mut c = Canvas::new(rc.right.max(1), rc.bottom.max(1));
        c.clear(0xFF00_0000 | paper);
        c.begin_batch();
        if let (false, true, Some(icons), Some(meta_f)) = (st.embedded, a > 0.01 || pinned, st.icons.as_ref(), st.meta.as_ref()) {
            for (b, r) in buttons(rc.right, s) {
                let a = if b == Btn::Pin && pinned { a.max(0.55) } else { a };
                if a <= 0.01 {
                    continue;
                }
                let (bx, by, bw, bh) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
                if st.hot == b {
                    let ha = st.hot_a.value * a;
                    c.shape(&Shape::new(bx, by, bw, bh, 7.0 * s), ink, 0.13 * ha, 1.0, 0.0);
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
                            Btn::Pin => if pinned { "\u{E841}" } else { "\u{E718}" },
                            Btn::More => "\u{E712}",
                            _ => "\u{E8BB}",
                        };
                        c.text(glyph, icons, r, ink, a * 0.9, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                        if b == Btn::Pin && pinned {
                            // The font's solid pin has no needle: draw the outline pin over
                            // it, so the filled head keeps its point.
                            c.text("\u{E718}", icons, r, ink, a * 0.9, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                        }
                    }
                }
            }
            let fr = rect((18.0 * s) as i32, rc.bottom - (26.0 * s) as i32, rc.right - (36.0 * s) as i32, (18.0 * s) as i32);
            let meta = t!("note_edited", crate::i18n::time::edited_time(updated));
            c.text(&meta, meta_f, fr, theme::paper_subtext(color, dark), a * 0.85, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
        }
        c.flush();
        super::scrollbar::paint(h, st, &mut c, ink);
        c.blit(hdc, 0, 0);
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

pub(super) fn click(h: HWND, b: Btn) {
    let Some(st) = state(h) else { return };
    let id = st.id;
    match b {
        Btn::Close => unsafe {
            PostMessageW(h, WM_CLOSE, 0, 0);
        },
        Btn::Pin => app::note_command(id, NC_PIN),
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
