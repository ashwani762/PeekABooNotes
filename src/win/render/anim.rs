// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The display-synced frame clock: it runs only while something is moving. The message
//! loop paces frames with `DwmFlush` (one per compositor frame, i.e. the monitor's
//! refresh rate) and falls back to a blocking `GetMessage` when idle.

use std::cell::Cell;
use std::time::Instant;

pub use crate::common::anim::*;


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
}

thread_local! {
    static ACTIVE: Cell<u32> = const { Cell::new(0) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Ask for frames for `c` until its frame function reports it has settled.
pub fn wake(c: Client) {
    ACTIVE.with(|a| {
        if a.get() == 0 {
            LAST.with(|l| l.set(None));
        }
        a.set(a.get() | c as u32);
    });
}

pub fn active() -> bool {
    ACTIVE.with(|a| a.get() != 0)
}

/// Run one frame for every active client.
pub fn tick() {
    let now = Instant::now();
    let dt = LAST.with(|l| {
        let d = l.get().map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(1.0 / 60.0);
        l.set(Some(now));
        d.clamp(1.0 / 400.0, 1.0 / 24.0)
    });
    let mask = ACTIVE.with(|a| a.get());
    let mut keep = 0;
    for c in [Client::Deck, Client::Ghost, Client::Toast, Client::Notes, Client::List, Client::Menu, Client::Settings] {
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
        };
        if more {
            keep |= bit;
        }
    }
    // Clients woken during this tick stay active.
    ACTIVE.with(|a| a.set((a.get() & !mask) | keep));
    perf(now.elapsed().as_secs_f32() * 1000.0, mask);
}

thread_local! {
    static PERF: Cell<Option<(u32, f32, f32)>> = const { Cell::new(None) };
    static PERF_ON: Cell<Option<bool>> = const { Cell::new(None) };
}

/// With PEEKABOO_PERF=1, append average/max frame times to %TEMP%\peekaboo-perf.txt.
fn perf(ms: f32, mask: u32) {
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
        let (n, sum, max) = p.get().unwrap_or((0, 0.0, 0.0));
        let (n, sum, max) = (n + 1, sum + ms, max.max(ms));
        if n >= 60 {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("peekaboo-perf.txt")) {
                let _ = writeln!(f, "frames={n} avg={:.2}ms max={:.2}ms clients={mask:#04x}", sum / n as f32, max);
            }
            p.set(None);
        } else {
            p.set(Some((n, sum, max)));
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
