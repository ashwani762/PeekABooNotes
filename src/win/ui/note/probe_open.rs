// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What opening a note costs, off screen (run with `--ignored --nocapture`): its window made
//! hidden (never shown), each part timed, and its picture for the flying card, drawn here and
//! asked of Windows (which has none for a window it hasn't composed).

use super::*;
use std::time::Instant;

fn md_long() -> String {
    let mut md = String::from("Trip plan for the autumn 🍂\n- [ ] passport and tickets\n- [x] book the hotel near the station\n- charger, adapter\n  - the long cable\n");
    for k in 0..30 {
        md.push_str(&format!("Paragraph {k}: we leave early on Friday and come back on Sunday evening, unless the release checklist slips again.\n"));
        if k % 5 == 0 {
            md.push_str("- [ ] call the hotel\n- [x] pack the bag\n");
        }
    }
    md.push_str("See [the booking](https://example.com/booking) before Thursday.");
    md
}

#[test]
#[ignore]
fn probe_open_costs() {
    app::init_for_probe();
    register();
    let notes = [
        ("empty", String::new()),
        ("garden", "Garden 🌱\n- [ ] Repot the basil\n- [ ] Order tulip bulbs\n- [x] Fix the drip line".to_string()),
        ("long", md_long()),
    ];
    let r = RECT { left: 1880, top: 300, right: 2200, bottom: 660 };
    // A hundred notes beside them, as a notes file has.
    for i in 0..100 {
        app::store_mut(|s| s.create(&format!("Note {i}\n{}", md_long()), ""));
    }
    let t = Instant::now();
    app::save_now();
    eprintln!("PROBE writing 103 notes: {:?}", t.elapsed());
    for round in 0..3 {
        for (name, md) in &notes {
            let id = app::store_mut(|s| s.create(md, ""));
            let t0 = Instant::now();
            // As `create` makes it, but hidden: never shown.
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
                raised: false,
                edited_at: None,
                before: None,
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
                glide: scrollbar::Glide::new(),
                save_held: None,
                sizing: None,
                canvas: None,
            });
            let h = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                    wide(CLASS).as_ptr(),
                    wide("x").as_ptr(),
                    WS_POPUP | WS_THICKFRAME | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN,
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
            let made = t0.elapsed();
            let t1 = Instant::now();
            unsafe {
                crate::win::util::no_transitions(h);
                SetWindowPos(h, null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let framed = t1.elapsed();
            let t2 = Instant::now();
            open_at_place(h);
            let placed = t2.elapsed();
            let edit = state(h).unwrap().edit;
            let mut er: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(edit, &mut er) };
            let t3 = Instant::now();
            let img = crate::win::editor::view::look(edit, 0, er.bottom);
            let drawn = t3.elapsed();
            let t4 = Instant::now();
            let pic = picture(h);
            let pictured = t4.elapsed();
            let t5 = Instant::now();
            let cap = crate::win::render::gfx::capture_now(h);
            let captured = t5.elapsed();
            eprintln!(
                "PROBE round {round} {name}: window made {made:?} (editor {}x{}), frame dropped {framed:?}, placed {placed:?}, editor drawn off screen {drawn:?} ({}), picture {pictured:?} ({}), Windows' picture {captured:?} ({})",
                er.right,
                er.bottom,
                if img.is_some() { "ok" } else { "none" },
                if pic.is_some() { "ok" } else { "none" },
                if cap.is_some() { "ok" } else { "none" },
            );
            // Closing, as WM_CLOSE does it (but for the card): the text kept, the picture,
            // the window gone.
            let t6 = Instant::now();
            flush(h);
            let flushed = t6.elapsed();
            let t7 = Instant::now();
            let snap = snapshot(h);
            let snapped = t7.elapsed();
            let t8 = Instant::now();
            unsafe { DestroyWindow(h) };
            let destroyed = t8.elapsed();
            eprintln!("PROBE round {round} {name} closing: kept {flushed:?}, picture and words {snapped:?} ({}), window gone {destroyed:?}", if snap.is_some() { "ok" } else { "none" });
        }
    }
}
