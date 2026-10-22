// Copyright (C) 2026 Ashwani Kumar Bachchan
// SPDX-License-Identifier: GPL-3.0-or-later

//! Springs and a display-synced frame clock.
//!
//! Springs are set by damping ratio and response time, and always move
//! from their current value and velocity, so any animation can be interrupted and
//! retargeted mid-flight without a jump. The frame clock runs only while something is
//! moving: the message loop paces frames with `DwmFlush` (one per compositor frame, i.e.
//! the monitor's refresh rate) and falls back to a blocking `GetMessage` when idle.

use std::cell::Cell;
use std::f32::consts::PI;
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    damping: f32,
    response: f32,
    eps: f32,
}

impl Spring {
    /// A spring for pixel-sized quantities.
    pub fn px(value: f32, params: (f32, f32)) -> Spring {
        Spring { value, velocity: 0.0, target: value, damping: params.0, response: params.1, eps: 0.15 }
    }

    /// A spring for 0..1 progress / opacity / scale quantities.
    pub fn unit(value: f32, params: (f32, f32)) -> Spring {
        Spring { value, velocity: 0.0, target: value, damping: params.0, response: params.1, eps: 0.002 }
    }

    pub fn to(&mut self, target: f32) {
        self.target = target;
    }

    pub fn retarget(&mut self, target: f32, params: (f32, f32)) {
        self.target = target;
        self.damping = params.0;
        self.response = params.1;
    }

    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.velocity = 0.0;
    }

    pub fn settled(&self) -> bool {
        (self.value - self.target).abs() < self.eps && self.velocity.abs() < self.eps * 8.0
    }

    /// Advance by `dt` seconds. Returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.settled() || reduced() {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }
        let k = (2.0 * PI / self.response).powi(2);
        let c = 4.0 * PI * self.damping / self.response;
        let mut left = dt.min(1.0 / 20.0);
        while left > 0.0 {
            let h = left.min(1.0 / 480.0);
            let a = -k * (self.value - self.target) - c * self.velocity;
            self.velocity += a * h;
            self.value += self.velocity * h;
            left -= h;
        }
        true
    }
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
}

thread_local! {
    static ACTIVE: Cell<u32> = const { Cell::new(0) };
    static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    static REDUCED: Cell<bool> = const { Cell::new(false) };
}

pub fn set_reduced_motion(on: bool) {
    REDUCED.with(|r| r.set(on));
}

pub fn reduced() -> bool {
    REDUCED.with(|r| r.get())
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
            Client::Deck => crate::ui::deck::frame(dt),
            Client::Ghost => crate::ui::ghost::frame(dt),
            Client::Toast => crate::ui::toast::frame(dt),
            Client::Notes => crate::ui::note::frame(dt),
            Client::List => crate::ui::allnotes::frame(dt),
            Client::Menu => crate::ui::menu::frame(dt),
            Client::Settings => crate::ui::settings::frame(dt),
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

/// Linear interpolation helpers.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}
