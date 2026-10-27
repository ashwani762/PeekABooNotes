// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The display-synced frame clock: it runs only while something is moving. The message
//! loop paces frames with `DwmFlush` (one per compositor frame, i.e. the monitor's
//! refresh rate) and falls back to a blocking `GetMessage` when idle. Each frame steps by
//! the compositor's refresh periods (`Pacer`), read from `DwmGetCompositionTimingInfo`.

use std::cell::Cell;
use std::time::Instant;

use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};

pub use crate::common::anim::*;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
}

/// Now, in seconds on the performance counter, the clock the compositor's times are on.
fn qpc_now() -> (i64, f64) {
    thread_local! {
        static FREQ: Cell<i64> = const { Cell::new(0) };
    }
    let freq = FREQ.with(|f| {
        if f.get() == 0 {
            let mut v = 0;
            unsafe { QueryPerformanceFrequency(&mut v) };
            f.set(v.max(1));
        }
        f.get()
    });
    let mut c = 0;
    unsafe { QueryPerformanceCounter(&mut c) };
    (c, freq as f64)
}

/// The time now and the compositor's refresh timing, both in seconds; no timing when the
/// compositor gives none (a remote session may not).
fn display_timing() -> (f64, Option<Vsync>) {
    let (now, freq) = qpc_now();
    let mut ti: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    ti.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
    let ok = unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut ti) } >= 0;
    let (period, vblank) = (ti.qpcRefreshPeriod, ti.qpcVBlank);
    let v = (ok && period > 0 && vblank > 0).then(|| Vsync { vblank: vblank as f64 / freq, period: period as f64 / freq });
    (now as f64 / freq, v)
}

// ---------------------------------------------------------------- frame clock

/// Animated subsystems. Each owns a bit in the active mask.
#[derive(Clone, Copy)]
pub enum Client {
    Deck = 1,
    Ghost = 2,
    Toast = 4,
    Notes = 8,
    List = 16,
    Menu = 32,
    Settings = 64,
    Tip = 128,
}

thread_local! {
    static ACTIVE: Cell<u32> = const { Cell::new(0) };
    static PACER: Cell<Pacer> = const { Cell::new(Pacer::new()) };
}

/// Ask for frames for `c` until its frame function reports it has settled.
pub fn wake(c: Client) {
    ACTIVE.with(|a| {
        if a.get() == 0 {
            PACER.with(|p| {
                let mut pacer = p.get();
                pacer.rest();
                p.set(pacer);
            });
        }
        a.set(a.get() | c as u32);
    });
}

pub fn active() -> bool {
    ACTIVE.with(|a| a.get() != 0)
}

/// The display's refresh period, in seconds, as the clock last saw it.
pub fn period() -> f32 {
    PACER.with(|p| p.get().period())
}

/// Run one frame for every active client.
pub fn tick() {
    let started = Instant::now();
    let (now, vsync) = display_timing();
    let dt = PACER.with(|p| {
        let mut pacer = p.get();
        let dt = pacer.step(now, vsync);
        p.set(pacer);
        dt
    });
    let mask = ACTIVE.with(|a| a.get());
    let mut keep = 0;
    for c in [Client::Deck, Client::Ghost, Client::Toast, Client::Notes, Client::List, Client::Menu, Client::Settings, Client::Tip] {
        let bit = c as u32;
        if mask & bit == 0 {
            continue;
        }
        let more = match c {
            Client::Deck => crate::win::ui::deck::frame(dt),
            Client::Ghost => crate::win::ui::ghost::frame(dt),
            Client::Toast => crate::win::ui::toast::frame(dt),
            Client::Notes => crate::win::ui::note::frame(dt),
            Client::List => crate::win::ui::allnotes::frame(dt),
            Client::Menu => crate::win::ui::menu::frame(dt),
            Client::Settings => crate::win::ui::settings::frame(dt),
            Client::Tip => crate::win::ui::tooltip::frame(dt),
        };
        if more {
            keep |= bit;
        }
    }
    // Clients woken during this tick stay active.
    ACTIVE.with(|a| a.set((a.get() & !mask) | keep));
    perf(started.elapsed().as_secs_f32() * 1000.0, mask, dt);
}

thread_local! {
    static PERF: Cell<Option<(u32, f32, f32, u32)>> = const { Cell::new(None) };
    static PERF_ON: Cell<Option<bool>> = const { Cell::new(None) };
}

/// With PEEKABOO_PERF=1, append average/max frame times to %TEMP%\peekaboo-perf.txt, the
/// display's refresh period, and how many frames stepped more than one refresh (late).
fn perf(ms: f32, mask: u32, dt: f32) {
    let on = PERF_ON.with(|p| {
        if p.get().is_none() {
            p.set(Some(std::env::var_os("PEEKABOO_PERF").is_some()));
        }
        p.get().unwrap_or(false)
    });
    if !on {
        return;
    }
    PERF.with(|p| {
        let period = period();
        let (n, sum, max, late) = p.get().unwrap_or((0, 0.0, 0.0, 0));
        let (n, sum, max, late) = (n + 1, sum + ms, max.max(ms), late + (dt > 1.5 * period) as u32);
        if n >= 60 {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("peekaboo-perf.txt")) {
                let _ = writeln!(f, "frames={n} avg={:.2}ms max={:.2}ms clients={mask:#04x} refresh={:.2}ms late={late}", sum / n as f32, max, period * 1000.0);
            }
            p.set(None);
        } else {
            p.set(Some((n, sum, max, late)));
        }
    });
}

/// Wait for the next compositor frame.
pub fn wait_vsync() {
    let hr = unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
    if hr < 0 {
        std::thread::sleep(std::time::Duration::from_millis(7));
    }
}

#[cfg(test)]
mod tests {
    /// Probe (run with --ignored --nocapture): the clock against this computer's compositor,
    /// paced as the message loop paces it, with no window: the refresh it reads, each
    /// frame's step in refreshes, a frame of heavy work (25 ms) and a rest.
    #[test]
    #[ignore]
    fn probe_frame_clock() {
        use super::*;
        let mut pacer = Pacer::new();
        let mut steps = Vec::new();
        let (mut late, mut even) = (0, 0);
        // What the clock did before: the time since the last frame, 1/60 s for the first.
        let (mut was, mut old) = (None::<Instant>, Vec::new());
        for i in 0..120 {
            if i == 80 {
                pacer.rest();
                was = None;
            }
            let (now, vsync) = display_timing();
            let dt = pacer.step(now, vsync);
            let p = pacer.period();
            let t = Instant::now();
            let before = was.map_or(1.0 / 60.0, |w: Instant| t.duration_since(w).as_secs_f32().clamp(1.0 / 400.0, 1.0 / 24.0));
            was = Some(t);
            old.push(before / p);
            if i == 0 {
                eprintln!("PROBE refresh {:.3} ms ({:.2} Hz), timing from the compositor: {}", p * 1000.0, 1.0 / p, vsync.is_some());
            }
            let periods = dt / p;
            steps.push(format!("{periods:.2}"));
            if (periods - 1.0).abs() < 1e-3 {
                even += 1;
            } else {
                late += 1;
            }
            if i == 40 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            wait_vsync();
        }
        eprintln!("PROBE steps in refreshes: {}", steps.join(" "));
        let (lo, hi) = old.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        let off = old.iter().filter(|s| (**s - 1.0).abs() > 0.05).count();
        eprintln!("PROBE before, the time between frames: {lo:.2} to {hi:.2} refreshes, {off} of {} frames more than 5% off one", old.len());
        eprintln!("PROBE {even} frames stepped one refresh, {late} more or less (the frame after the heavy one should be the only one)");
    }
}
