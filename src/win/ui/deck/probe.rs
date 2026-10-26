// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the deck's frames cost, off screen (run with `--ignored --nocapture`): a deck of
//! eight cards fans out, a card is pointed at and pulled out, and the deck folds again, as
//! the pointer does it, frame by frame at 99 Hz.

use super::*;
use std::time::{Duration, Instant};

const NOTES: [(&str, &str); 8] = [
    ("Shakshuka 🍳", "- 2 tins of tomatoes\n- 6 eggs\n- feta, parsley"),
    ("Book club", "Next: The Remains of the Day\n- bring the wine"),
    ("Gift ideas 🎁", "- Mum: a scarf\n- Dad: the gardening book\n- Sam: concert tickets"),
    ("Q4 goals, draft", "- Cut p95 latency to 200 ms\n- Ship offline mode\n- NPS from 45 to 55"),
    ("Podcast notes for the interview with the founders", "- Intro: how the company started in a garage\n- The first customer and what they asked for\n- Hiring the first ten people\n- What they would do differently\n- Questions from listeners\n- Wrap up and thanks"),
    ("Running plan for the autumn half marathon", "- Mon: rest\n- Tue: 8 km easy\n- Wed: intervals, 6 x 800 m\n- Thu: 6 km easy\n- Fri: rest\n- Sat: long run, 18 km\n- Sun: 5 km recovery\nKeep the long run slow: a pace you could talk at."),
    ("Groceries", "- milk\n- bread\n- apples"),
    ("Garden 🌻", "- prune the roses\n- seed the lawn"),
];

pub(super) fn deck() -> Deck {
    let work = RECT { left: 0, top: 0, right: 2560, bottom: 1040 };
    let mut geo = geo_for(NOTES.len(), false, 0.42, 8, work, 1.0);
    geo.caps = NOTES.len();
    let tabs = NOTES
        .iter()
        .enumerate()
        .map(|(i, (title, body))| {
            let mut t = Tab::new(i as u64 + 1, geo.slot_y(i));
            t.slot = i;
            t.title = title.to_string();
            t.label = title.to_uppercase();
            t.body = body_of(&format!("{title}\n{body}"));
            t.color = i as u8 % 6;
            t
        })
        .collect();
    Deck {
        hwnd: null_mut(),
        tabs,
        open: false,
        wide: false,
        previewing: false,
        pill: Spring::unit(1.0, motion::DEFAULT),
        extras: Spring::unit(0.0, motion::SHINGLE),
        extras_delay: 0.0,
        extras_goal: 0.0,
        plus_hot: Spring::unit(0.0, motion::SNAPPY),
        more_hot: Spring::unit(0.0, motion::SNAPPY),
        hover: Hit::None,
        pressed: Hit::None,
        tracking: false,
        menu: false,
        drag: None,
        fullscreen: false,
        visible: true,
        geo,
        fonts: HashMap::new(),
        font_scale: 1.0,
        canvas: None,
        marquee: None,
        marquee_live: false,
        kbd: None,
        summoned: false,
    }
}

#[repr(C)]
#[derive(Default)]
struct Counters {
    cb: u32,
    faults: u32,
    peak_ws: usize,
    ws: usize,
    quota: [usize; 4],
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
}

#[repr(C)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    id: u32,
    owner: u32,
    base: i32,
    delta: i32,
    flags: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(p: windows_sys::Win32::Foundation::HANDLE, c: *mut Counters, cb: u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> windows_sys::Win32::Foundation::HANDLE;
    fn Thread32First(h: windows_sys::Win32::Foundation::HANDLE, e: *mut ThreadEntry) -> i32;
    fn Thread32Next(h: windows_sys::Win32::Foundation::HANDLE, e: *mut ThreadEntry) -> i32;
}

pub(super) fn private_mb() -> f64 {
    let mut c = Counters { cb: size_of::<Counters>() as u32, ..Default::default() };
    unsafe { K32GetProcessMemoryInfo(windows_sys::Win32::System::Threading::GetCurrentProcess(), &mut c, c.cb) };
    c.private as f64 / 1_048_576.0
}

pub(super) fn threads() -> usize {
    let pid = std::process::id();
    let mut n = 0;
    unsafe {
        let snap = CreateToolhelp32Snapshot(4, 0);
        let mut e = ThreadEntry { size: size_of::<ThreadEntry>() as u32, usage: 0, id: 0, owner: 0, base: 0, delta: 0, flags: 0 };
        let mut ok = Thread32First(snap, &mut e);
        while ok != 0 {
            if e.owner == pid {
                n += 1;
            }
            ok = Thread32Next(snap, &mut e);
        }
        windows_sys::Win32::Foundation::CloseHandle(snap);
    }
    n
}

/// The process's CPU time so far (all threads: Direct2D may draw on others).
pub(super) fn cpu() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    let z = || FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut a, mut b, mut k, mut u) = (z(), z(), z(), z());
    unsafe { windows_sys::Win32::System::Threading::GetProcessTimes(windows_sys::Win32::System::Threading::GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u) };
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) * 100;
    Duration::from_nanos(t(k) + t(u))
}

/// Frames as the clock runs them, one per 99 Hz refresh, until the deck rests: (frames,
/// time spent drawing).
pub(super) fn settle(d: &mut Deck) -> (u32, Duration) {
    settle_for(d, Duration::from_secs(15))
}

thread_local! {
    static SEED: std::cell::Cell<u32> = const { std::cell::Cell::new(7) };
}

/// `settle`, for at most `limit`. Frames come a little early or late, as they do.
pub(super) fn settle_for(d: &mut Deck, limit: Duration) -> (u32, Duration) {
    let period = Duration::from_micros(10_101);
    let (mut n, mut spent) = (0, Duration::ZERO);
    let start = Instant::now();
    loop {
        let t0 = Instant::now();
        let jitter = SEED.with(|s| {
            let v = s.get().wrapping_mul(1_103_515_245).wrapping_add(12_345);
            s.set(v);
            (v >> 16) as f32 / 65_536.0
        });
        let (moving, _, draw) = advance(d, (0.9 + 0.2 * jitter) / 99.0);
        if draw {
            paint(d, false);
        }
        spent += t0.elapsed();
        n += 1;
        if !(moving || d.marquee_live) || start.elapsed() > limit {
            return (n, spent);
        }
        if let Some(rest) = period.checked_sub(t0.elapsed()) {
            std::thread::sleep(rest);
        }
    }
}

#[test]
#[ignore]
fn probe_peeks() {
    crate::common::i18n::init("en");
    let mut d = deck();
    let (mem0, threads0) = (private_mb(), threads());
    let mut report = String::new();
    for round in 0..6 {
        for (k, (title, _)) in NOTES.iter().enumerate() {
            // The pointer arrives: the deck fans out.
            let c0 = cpu();
            set_open(&mut d, true, false);
            let (fan_n, fan_t) = settle(&mut d);
            let fan_cpu = cpu() - c0;
            // It rests on card k, which comes out after the dwell.
            let c0 = cpu();
            d.hover = Hit::Tab(k);
            let (hov_n, hov_t) = settle(&mut d);
            begin_preview(&mut d, k);
            let (peek_n, peek_t) = settle(&mut d);
            let peek_cpu = cpu() - c0;
            // It leaves: the card tucks back and the deck folds.
            let c0 = cpu();
            d.hover = Hit::None;
            end_preview(&mut d);
            set_open(&mut d, false, false);
            let (fold_n, fold_t) = settle(&mut d);
            let fold_cpu = cpu() - c0;
            if round == 0 {
                report += &format!(
                    "  {title:.24}: fan {fan_n} frames {fan_t:.1?} (cpu {fan_cpu:.0?}), hover+peek {} frames {:.1?} (cpu {peek_cpu:.0?}), fold {fold_n} frames {fold_t:.1?} (cpu {fold_cpu:.0?})\n",
                    hov_n + peek_n,
                    hov_t + peek_t
                );
            }
        }
        report += &format!("  after round {}: private {:.1} MB (from {mem0:.1}), threads {} (from {threads0})\n", round + 1, private_mb(), threads());
    }
    eprintln!("PROBE deck peeks, 8 cards at 99 Hz:\n{report}");
}

/// Memory as the pointer goes in and out over the deck, pulling a card out each time, at
/// frames that never quite repeat.
#[test]
#[ignore]
fn probe_hover_memory() {
    crate::common::i18n::init("en");
    let mut d = deck();
    let mut report = format!("  at start: private {:.1} MB, threads {}\n", private_mb(), threads());
    let c0 = cpu();
    for round in 0..8 {
        for k in 0..NOTES.len() {
            set_open(&mut d, true, false);
            settle_for(&mut d, Duration::from_millis(300));
            d.hover = Hit::Tab(k);
            begin_preview(&mut d, k);
            settle_for(&mut d, Duration::from_millis(600));
            d.hover = Hit::None;
            end_preview(&mut d);
            set_open(&mut d, false, false);
            settle_for(&mut d, Duration::from_millis(300));
        }
        report += &format!("  after round {}: private {:.1} MB, threads {}, cpu {:.0?}\n", round + 1, private_mb(), threads(), cpu() - c0);
    }
    eprintln!("PROBE deck hover memory, 8 peeks a round:\n{report}");
}

/// Memory as an emoji is drawn fading (at an opacity it wasn't drawn at before) and turning
/// (at a new angle), 2,000 times a step: Direct2D kept each one it drew for good.
#[test]
#[ignore]
fn probe_emoji_memory() {
    use crate::win::render::gfx::Canvas;
    let mut c = Canvas::new(200, 400);
    let f = Font::new(13.0, 600, "Segoe UI");
    let mut seed = 7u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (seed >> 16) as f32 / 65_536.0
    };
    let clip = rect(0, 0, 200, 400);
    let mut out = String::new();
    for turning in [false, true] {
        out += &format!("  {}: {:.1} MB", if turning { "turning" } else { "fading" }, private_mb());
        for _ in 0..4 {
            for _ in 0..2000 {
                c.clear(0);
                let a = rnd();
                if turning {
                    c.rotated("GIFT IDEAS \u{1F381}", &f, (100.0, 300.0), -90.0 + a * 1.3, (100.0, 300.0), clip, 0, 1.0);
                } else {
                    c.text("Gift ideas \u{1F381}", &f, rect(10, 10, 180, 40), 0, a, 0);
                }
            }
            out += &format!(", {:.1}", private_mb());
        }
        out += "\n";
    }
    eprintln!("PROBE an emoji drawn 2,000 times a step:\n{out}");
}

/// What one frame costs to draw in each state the deck rests in.
#[test]
#[ignore]
fn probe_frame_costs() {
    crate::common::i18n::init("en");
    let mut d = deck();
    let avg = |d: &mut Deck| {
        paint(d, false);
        let t0 = Instant::now();
        for _ in 0..200 {
            d.marquee_live = false;
            paint(d, false);
        }
        t0.elapsed() / 200
    };
    let folded = avg(&mut d);
    set_open(&mut d, true, true);
    let fanned = avg(&mut d);
    d.hover = Hit::Tab(5);
    settle_for(&mut d, Duration::from_millis(400));
    let marquee = avg(&mut d);
    d.hover = Hit::Tab(3);
    begin_preview(&mut d, 3);
    settle_for(&mut d, Duration::from_secs(2));
    let pulled = avg(&mut d);
    d.hover = Hit::Tab(5);
    begin_preview(&mut d, 5);
    settle_for(&mut d, Duration::from_millis(500));
    let pulled_long = avg(&mut d);
    eprintln!("PROBE deck frame: folded {folded:?}, fanned {fanned:?}, a long title scrolling {marquee:?}, a card out {pulled:?}, a long card out {pulled_long:?}");
}
