// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The display-synced frame clock: it runs only while something is moving, and falls back
//! to a blocking `GetMessage` when idle. Frames are paced by the refreshes of the display
//! the motion is on (`vblank`, at its own rate: 60, 99.99, 144 Hz side by side), and each
//! steps by that display's whole refresh periods (`Pacer`); without them, by the
//! compositor's clock (`DwmGetCompositionTimingInfo`, `DwmFlush`).

use std::cell::Cell;
use std::time::Instant;

use windows_sys::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWM_TIMING_INFO, DwmGetCompositionTimingInfo};
use windows_sys::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTONEAREST, MonitorFromPoint, MonitorFromRect, MonitorFromWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible};

pub use crate::common::anim::*;
use crate::win::render::vblank;

/// The compositor's refresh timing, in seconds: one clock for every display (no timing
/// when it gives none, as a remote session may not).
fn compositor_timing() -> Option<Timing> {
    use std::sync::OnceLock;
    static FREQ: OnceLock<f64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
        }
        let mut v = 0;
        unsafe { QueryPerformanceFrequency(&mut v) };
        v.max(1) as f64
    });
    let mut ti: DWM_TIMING_INFO = unsafe { std::mem::zeroed() };
    ti.cbSize = size_of::<DWM_TIMING_INFO>() as u32;
    let ok = unsafe { DwmGetCompositionTimingInfo(std::ptr::null_mut(), &mut ti) } >= 0;
    let (period, vblank) = (ti.qpcRefreshPeriod, ti.qpcVBlank);
    (ok && period > 0 && vblank > 0).then(|| Timing { period: period as f64 / freq, vblank: Some(vblank as f64 / freq), display: 0 })
}

/// The timing of display `mon`: its own rate and refreshes; until one of its refreshes has
/// been seen, the compositor's when that clock runs at the display's rate (it's the
/// display's then, as with one display, or the main one); the compositor's alone when the
/// display's rate can't be had.
fn timing_of(mon: HMONITOR) -> Option<Timing> {
    let display = mon as u64;
    match vblank::timing(mon) {
        Some((period, Some(seen))) => Some(Timing { period, vblank: Some(seen), display }),
        Some((period, None)) => {
            let shared = compositor_timing().filter(|c| (c.period - period).abs() < 0.002 * period).and_then(|c| c.vblank);
            // Its refreshes as another clock has them, a little apart from where they're
            // seen: another grid, so the step onto the display's own goes by the time.
            Some(Timing { period, vblank: shared, display: display | 1 << 63 })
        }
        None => compositor_timing(),
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
    Tip = 128,
}

thread_local! {
    static ACTIVE: Cell<u32> = const { Cell::new(0) };
    static PACER: Cell<Pacer> = const { Cell::new(Pacer::new()) };
    /// The display a part said this run's motion is on (a card in flight: its window's).
    static PACE: Cell<Option<HMONITOR>> = const { Cell::new(None) };
    /// The part that last asked for frames.
    static LAST: Cell<u32> = const { Cell::new(0) };
    /// The display this run's frames were last paced by through a window (or a card).
    static RUN_DISPLAY: Cell<Option<HMONITOR>> = const { Cell::new(None) };
}

/// Ask for frames for `c` until its frame function reports it has settled.
pub fn wake(c: Client) {
    LAST.with(|l| l.set(c as u32));
    ACTIVE.with(|a| {
        if a.get() == 0 {
            PACER.with(|p| {
                let mut pacer = p.get();
                pacer.rest();
                p.set(pacer);
            });
            TURN.with(|t| t.set(None));
            PACE.with(|p| p.set(None));
            RUN_DISPLAY.with(|d| d.set(None));
        }
        a.set(a.get() | c as u32);
    });
}

pub fn active() -> bool {
    ACTIVE.with(|a| a.get() != 0)
}

/// Pace this run's frames by the display that holds screen rect `r` (where a card flies).
pub fn pace_on(r: &RECT) {
    let mon = unsafe { MonitorFromRect(r, MONITOR_DEFAULTTONEAREST) };
    if !mon.is_null() {
        PACE.with(|p| p.set(Some(mon)));
    }
}

/// The window part `c`'s motion is in (null when it has none showing: a flying card names
/// its display itself, with `pace_on`).
fn window_of(c: u32) -> HWND {
    use crate::win::ui;
    let h = match c {
        x if x == Client::Deck as u32 => ui::deck::window(),
        x if x == Client::Toast as u32 => ui::toast::window(),
        x if x == Client::List as u32 => ui::allnotes::hwnd(),
        x if x == Client::Menu as u32 => ui::menu::window(),
        x if x == Client::Settings as u32 => ui::settings::hwnd(),
        x if x == Client::Tip as u32 => ui::tooltip::window(),
        // A note's controls, scroll bar or ticks: the note whose own motion last stepped,
        // else the note (or All Notes) being used.
        x if x == Client::Notes as u32 => {
            let moving = ui::note::pace_window();
            if !moving.is_null() {
                moving
            } else {
                unsafe {
                    let fg = GetForegroundWindow();
                    let ours = !fg.is_null() && GetWindowThreadProcessId(fg, std::ptr::null_mut()) == windows_sys::Win32::System::Threading::GetCurrentThreadId();
                    if ours { fg } else { std::ptr::null_mut() }
                }
            }
        }
        _ => std::ptr::null_mut(),
    };
    if !h.is_null() && unsafe { IsWindowVisible(h) } != 0 { h } else { std::ptr::null_mut() }
}

/// The display the frames are paced by: the one a part named for this run (a card in
/// flight), else the one the window of what's moving is on (what was set moving last
/// first), else the one under the pointer. Paced by the pointer's alone, motion on a
/// 99.99 Hz display ran on a 60 Hz one's refreshes whenever the pointer was there (a
/// toast, a note's tick or scroll seen from the other screen).
fn pace_display() -> HMONITOR {
    if let Some(m) = PACE.with(|p| p.get()) {
        RUN_DISPLAY.with(|d| d.set(Some(m)));
        return m;
    }
    let (last, active) = (LAST.with(|l| l.get()), ACTIVE.with(|a| a.get()));
    if let Some(h) = pace_order(last, active).map(window_of).find(|h| !h.is_null()) {
        let m = unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST) };
        RUN_DISPLAY.with(|d| d.set(Some(m)));
        return m;
    }
    // No window to go by now (a note just closed, its last frames still running): the
    // display the run went by until now, rather than the pointer's, which put an Esc'd
    // note's last frames on the other display's refreshes.
    if let Some(m) = RUN_DISPLAY.with(|d| d.get()) {
        return m;
    }
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut pt);
        MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
    }
}

/// The display's refresh period, in seconds, as the clock last saw it.
pub fn period() -> f32 {
    PACER.with(|p| p.get().period())
}

/// Run one frame for every active client.
pub fn tick() {
    let started = Instant::now();
    let now = vblank::now();
    let mon = pace_display();
    vblank::track(mon);
    let timing = timing_of(mon);
    TURN.with(|t| {
        if t.get().is_none() {
            t.set(Some(now));
        }
    });
    let dt = PACER.with(|p| {
        let mut pacer = p.get();
        let dt = pacer.step(now, timing);
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
    if !ACTIVE.with(|a| a.get()) & Client::Ghost as u32 != 0 {
        // The cards have landed: what moves on goes by its own window.
        PACE.with(|p| p.set(None));
    }
    paint_pending();
    if !active() {
        // At rest: the thread noting the display's refreshes rests too.
        vblank::rest();
    }
    perf(started.elapsed().as_secs_f32() * 1000.0, mask, dt);
}

thread_local! {
    static PERF: Cell<Option<(u32, f32, f32, u32)>> = const { Cell::new(None) };
    static PERF_ON: Cell<Option<bool>> = const { Cell::new(None) };
}

/// With PEEKABOO_PERF=1, append average/max frame times to %TEMP%\peekaboo-perf.txt, the
/// display's refresh period, and how many frames stepped more than one refresh (late).
fn perf(ms: f32, mask: u32, dt: f32) {
    if !perf_on() {
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

/// Whether PEEKABOO_PERF=1 asks for frame times.
pub fn perf_on() -> bool {
    PERF_ON.with(|p| {
        if p.get().is_none() {
            p.set(Some(std::env::var_os("PEEKABOO_PERF").is_some()));
        }
        p.get().unwrap_or(false)
    })
}

/// A line for %TEMP%\peekaboo-perf.txt (with PEEKABOO_PERF=1).
pub fn perf_line(line: &str) {
    use std::io::Write;
    if perf_on()
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("peekaboo-perf.txt"))
    {
        let _ = writeln!(f, "{line}");
    }
}

thread_local! {
    /// When the message being handled was taken from the queue: what a flight's lag is
    /// counted from.
    static MESSAGE_AT: Cell<f64> = const { Cell::new(0.0) };
}

/// A message was taken from the queue, to be handled now.
pub fn message_taken() {
    MESSAGE_AT.with(|m| m.set(vblank::now()));
}

/// When the message being handled was taken from the queue, on `vblank::now`'s clock.
pub fn message_at() -> f64 {
    MESSAGE_AT.with(|m| m.get())
}

thread_local! {
    /// When the frame being made began its turn: when the wait before it returned, or its
    /// tick after a rest.
    static TURN: Cell<Option<f64>> = const { Cell::new(None) };
    /// A high-resolution timer to wait on (null where Windows has none).
    static TIMER: Cell<Option<HANDLE>> = const { Cell::new(None) };
}

fn timer() -> HANDLE {
    use windows_sys::Win32::System::Threading::{CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, TIMER_ALL_ACCESS};
    TIMER.with(|t| {
        if t.get().is_none() {
            t.set(Some(unsafe { CreateWaitableTimerExW(std::ptr::null(), std::ptr::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS) }));
        }
        t.get().unwrap_or(std::ptr::null_mut())
    })
}

/// Sleep `secs` seconds, to a fraction of a millisecond.
fn sleep(secs: f64) {
    use windows_sys::Win32::System::Threading::{INFINITE, SetWaitableTimer, WaitForSingleObject};
    let timer = timer();
    // In 100 ns units, negative for "from now".
    let due = -((secs * 1e7).ceil() as i64).max(1);
    if !timer.is_null() && unsafe { SetWaitableTimer(timer, &due, 0, None, std::ptr::null(), 0) } != 0 {
        unsafe { WaitForSingleObject(timer, INFINITE) };
    } else {
        std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    }
}

/// Wait for the next frame's moment: just after the first refresh, of the display the motion
/// is on, since this frame's turn began (`plan_wait`), or not at all when that's gone by.
/// DwmFlush, which this used alone, waits until what was drawn is on screen: after a turn
/// that painted a window (a note's text, Settings, a checkbox) that's a refresh later than
/// the frame needed, so every frame took two and ran at 30 fps; and it keeps to one clock
/// for every display, whatever their rates.
pub fn wait_vsync() {
    let now = vblank::now();
    let began = TURN.with(|t| t.get()).unwrap_or(now);
    let plan = match timing_of(pace_display()) {
        Some(t) => plan_wait(began, now, t.period, t.vblank),
        None => Wait::Refresh,
    };
    match plan {
        Wait::For(secs) => sleep(secs),
        Wait::No => {}
        // No refresh seen to time it by (the first frame of a run): the display's own next,
        // as its waiting thread sees it begin, and the moment after it the compositor takes
        // the frame. The compositor's (DwmFlush) is another display's clock with displays at
        // other rates: a 60 Hz one's made the first frame on a 100 Hz display a refresh late,
        // 17 to 19 ms, every run.
        Wait::Refresh => {
            let mon = pace_display();
            let period = timing_of(mon).map_or(period() as f64, |t| t.period);
            match vblank::wait_next(mon, 1.5 * period) {
                Some(true) => sleep(0.002f64.min(0.2 * period)),
                // The display isn't refreshing (asleep, gone): on by the time.
                Some(false) => {}
                None => {
                    if unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() } < 0 {
                        sleep(period);
                    }
                }
            }
        }
    }
    TURN.with(|t| t.set(Some(vblank::now())));
}

/// Paint now whatever of this thread's windows a frame left to paint (a note's header, its
/// scroll bar, Settings, All Notes), so the frame goes to the compositor whole, before the
/// wait: left to the next turn's messages, it reached the screen a refresh late.
fn paint_pending() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_UPDATENOW, RedrawWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, IsWindowVisible};
    unsafe extern "system" fn each(h: HWND, _: LPARAM) -> windows_sys::core::BOOL {
        unsafe {
            if IsWindowVisible(h) != 0 {
                RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN);
            }
        }
        1
    }
    unsafe { EnumThreadWindows(windows_sys::Win32::System::Threading::GetCurrentThreadId(), Some(each), 0) };
}

#[cfg(test)]
mod tests {
    /// Probe (run with --ignored --nocapture): the clock and the wait against this computer's
    /// displays, paced as the message loop paces it, with no window, on the display under the
    /// pointer: its rate and the compositor's, each frame's step and each turn's length in
    /// refreshes, with 3 ms of work a frame, a frame of heavy work (25 ms), a rest, and a
    /// stretch of turns that wait for the screen in their own work (as a paint that reads a
    /// window back does, here a DwmFlush); then the same stretch paced by DwmFlush, as the
    /// loop was.
    #[test]
    #[ignore]
    fn probe_frame_clock() {
        use super::*;
        let spin = |ms: f64| {
            let t = Instant::now();
            while t.elapsed().as_secs_f64() * 1000.0 < ms {}
        };
        let mon = pace_display();
        let own = vblank::timing(mon).map(|t| 1.0 / t.0);
        let shared = compositor_timing().map(|t| 1.0 / t.period);
        eprintln!("PROBE display under the pointer: {own:.4?} Hz from the configuration; the compositor's clock: {shared:.4?} Hz");
        let mut pacer = Pacer::new();
        let (mut steps, mut turns) = (Vec::new(), Vec::new());
        let mut last = Instant::now();
        let mut seen_first = None;
        for i in 0..120 {
            if i == 80 {
                pacer.rest();
                vblank::rest();
                TURN.with(|t| t.set(None));
            }
            let now = vblank::now();
            vblank::track(mon);
            let timing = timing_of(mon);
            if seen_first.is_none() && timing.is_some_and(|t| t.vblank.is_some()) {
                seen_first = Some(i);
            }
            TURN.with(|t| {
                if t.get().is_none() {
                    t.set(Some(now));
                }
            });
            let dt = pacer.step(now, timing);
            let p = pacer.period();
            steps.push(format!("{:.2}", dt / p));
            spin(3.0);
            if i == 40 {
                spin(25.0);
            }
            if (90..110).contains(&i) {
                unsafe { windows_sys::Win32::Graphics::Dwm::DwmFlush() };
            }
            wait_vsync();
            turns.push(format!("{:.2}", last.elapsed().as_secs_f32() / p));
            last = Instant::now();
        }
        vblank::rest();
        eprintln!("PROBE the display's refreshes known from frame {seen_first:?}");
        eprintln!("PROBE steps in refreshes: {}", steps.join(" "));
        eprintln!("PROBE turns in refreshes (heavy frame at 40, a rest at 80, waits for the screen 90-109): {}", turns.join(" "));
        let p = period();
        let mut old = Vec::new();
        let mut last = Instant::now();
        for _ in 0..20 {
            spin(3.0);
            unsafe {
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
                windows_sys::Win32::Graphics::Dwm::DwmFlush();
            }
            old.push(format!("{:.2}", last.elapsed().as_secs_f32() / p));
            last = Instant::now();
        }
        eprintln!("PROBE before, paced by DwmFlush, turns that wait for the screen: {}", old.join(" "));
    }
}
