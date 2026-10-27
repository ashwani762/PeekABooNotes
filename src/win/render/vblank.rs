// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Each display's own refreshes, for the frame clock: how long one lasts (the display
//! configuration's exact rate: 99.99 Hz, not 100) and when they come. The compositor has one
//! clock for every display, so with a 99.99 Hz display beside a 60 Hz one, motion paced by
//! it on the other display gets frames at the wrong rate, two to some refreshes and none to
//! others. While anything moves, a thread of its own waits on the vertical blank of the
//! display the motion is on (`D3DKMTWaitForVerticalBlankEvent`) and notes when each refresh
//! began; the message loop never waits on it itself, so a display that stops refreshing
//! (asleep, unplugged) can't hold the app up. Displays are kept until they change (`forget`,
//! from `displays::forget`).

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use windows_sys::Win32::Graphics::Gdi::{CreateDCW, DEVMODEW, DeleteDC, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};

use crate::win::util::from_wide;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
}

#[repr(C)]
struct OpenAdapterFromHdc {
    hdc: HDC,
    adapter: u32,
    luid_low: u32,
    luid_high: i32,
    source: u32,
}

#[repr(C)]
struct WaitForVerticalBlank {
    adapter: u32,
    device: u32,
    source: u32,
}

#[repr(C)]
struct CloseAdapter {
    adapter: u32,
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn D3DKMTOpenAdapterFromHdc(p: *mut OpenAdapterFromHdc) -> i32;
    fn D3DKMTWaitForVerticalBlankEvent(p: *const WaitForVerticalBlank) -> i32;
    fn D3DKMTCloseAdapter(p: *const CloseAdapter) -> i32;
}

/// Now, in seconds on the performance counter, the clock the compositor's times are on.
pub fn now() -> f64 {
    static FREQ: OnceLock<f64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut v = 0;
        unsafe { QueryPerformanceFrequency(&mut v) };
        v.max(1) as f64
    });
    let mut c = 0;
    unsafe { QueryPerformanceCounter(&mut c) };
    c as f64 / freq
}

/// A display's video source, as the vertical-blank wait takes it: its adapter and source
/// numbers in one (never 0: an adapter is never 0).
fn key(adapter: u32, source: u32) -> u64 {
    ((adapter as u64) << 32) | source as u64
}

/// Wait for the next refresh of the source `key` stands for. False if it can't be waited on.
fn wait_on(key: u64) -> bool {
    let w = WaitForVerticalBlank { adapter: (key >> 32) as u32, device: 0, source: key as u32 };
    unsafe { D3DKMTWaitForVerticalBlankEvent(&w) == 0 }
}

/// The source the waiting thread waits on (0: none, it rests), and the last refresh it saw
/// begin: which source, and when (an f64's bits).
static WANTED: AtomicU64 = AtomicU64::new(0);
static SEEN_ON: AtomicU64 = AtomicU64::new(0);
static SEEN_AT: AtomicU64 = AtomicU64::new(0);
static WAITER: OnceLock<std::thread::Thread> = OnceLock::new();

fn waiter() -> &'static std::thread::Thread {
    WAITER.get_or_init(|| {
        std::thread::Builder::new()
            .name("vblank".into())
            .spawn(|| {
                loop {
                    let k = WANTED.load(Ordering::Acquire);
                    if k == 0 {
                        std::thread::park();
                        continue;
                    }
                    if wait_on(k) {
                        SEEN_AT.store(now().to_bits(), Ordering::Release);
                        SEEN_ON.store(k, Ordering::Release);
                    } else {
                        // Can't be waited on (the display gone): don't spin.
                        std::thread::sleep(std::time::Duration::from_millis(16));
                    }
                }
            })
            .map(|h| h.thread().clone())
            .unwrap_or_else(|_| std::thread::current())
    })
}

/// A display as the clock knows it.
struct Output {
    mon: HMONITOR,
    /// How long a refresh lasts, in seconds.
    period: Option<f64>,
    /// Its video source, to wait on its refreshes.
    source: Option<u64>,
}

thread_local! {
    static OUTPUTS: RefCell<Vec<Output>> = const { RefCell::new(Vec::new()) };
    static TRACKED: Cell<u64> = const { Cell::new(0) };
}

fn open(mon: HMONITOR) -> Output {
    let mut out = Output { mon, period: None, source: None };
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
            return out;
        }
        let gdi = from_wide(&mi.szDevice);
        out.period = crate::win::ui::displays::refresh_period(&gdi).or_else(|| {
            // Without the configuration's rate, the whole hertz GDI has (99 for 99.99).
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = size_of::<DEVMODEW>() as u16;
            (EnumDisplaySettingsW(mi.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) != 0 && dm.dmDisplayFrequency > 1).then(|| 1.0 / dm.dmDisplayFrequency as f64)
        });
        let dc = CreateDCW(std::ptr::null(), mi.szDevice.as_ptr(), std::ptr::null(), std::ptr::null());
        if !dc.is_null() {
            let mut o = OpenAdapterFromHdc { hdc: dc, adapter: 0, luid_low: 0, luid_high: 0, source: 0 };
            if D3DKMTOpenAdapterFromHdc(&mut o) == 0 {
                out.source = Some(key(o.adapter, o.source));
            }
            DeleteDC(dc);
        }
    }
    out
}

fn with<R>(mon: HMONITOR, f: impl FnOnce(&Output) -> R) -> R {
    OUTPUTS.with(|o| {
        let mut o = o.borrow_mut();
        let i = match o.iter().position(|x| x.mon == mon) {
            Some(i) => i,
            None => {
                o.push(open(mon));
                o.len() - 1
            }
        };
        f(&o[i])
    })
}

/// Have the refreshes of display `mon` noted from now on (the motion is on it).
pub fn track(mon: HMONITOR) {
    let k = with(mon, |o| o.source).unwrap_or(0);
    if TRACKED.with(|t| t.replace(k)) != k {
        WANTED.store(k, Ordering::Release);
        if k != 0 {
            waiter().unpark();
        }
    }
}

/// Display `mon`'s refresh period and, when its refreshes are being noted, when the last one
/// began; None when its rate can't be had.
pub fn timing(mon: HMONITOR) -> Option<(f64, Option<f64>)> {
    let (period, source) = with(mon, |o| (o.period, o.source));
    let seen = source.filter(|&k| SEEN_ON.load(Ordering::Acquire) == k).map(|_| f64::from_bits(SEEN_AT.load(Ordering::Acquire)));
    period.map(|p| (p, seen))
}

/// Motion has stopped: the waiting thread rests, and what it saw is let go.
pub fn rest() {
    TRACKED.with(|t| t.set(0));
    WANTED.store(0, Ordering::Release);
    SEEN_ON.store(0, Ordering::Release);
}

/// The displays changed: work every one out again when next asked.
pub fn forget() {
    rest();
    for o in OUTPUTS.with(|o| std::mem::take(&mut *o.borrow_mut())) {
        if let Some(k) = o.source {
            unsafe { D3DKMTCloseAdapter(&CloseAdapter { adapter: (k >> 32) as u32 }) };
        }
    }
}

#[cfg(test)]
mod tests {
    /// Probe (run with --ignored --nocapture): every display's refresh as the clock sees it,
    /// with no window: its rate from the display configuration, and the time between its
    /// refreshes waited on one after another.
    #[test]
    #[ignore]
    fn probe_displays() {
        use super::*;
        use windows_sys::Win32::Foundation::{LPARAM, RECT};
        use windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors;
        unsafe extern "system" fn each(mon: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows_sys::core::BOOL {
            unsafe { (*(lp as *mut Vec<HMONITOR>)).push(mon) };
            1
        }
        let mut mons: Vec<HMONITOR> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut mons as *mut Vec<HMONITOR> as LPARAM) };
        for mon in mons {
            let period = timing(mon).map(|t| t.0);
            let source = with(mon, |o| o.source);
            let mut gaps = Vec::new();
            if let Some(k) = source {
                wait_on(k);
                let mut last = now();
                for _ in 0..30 {
                    if !wait_on(k) {
                        break;
                    }
                    let t = now();
                    gaps.push(t - last);
                    last = t;
                }
            }
            let (lo, hi) = gaps.iter().fold((f64::MAX, 0.0f64), |(lo, hi), &g| (lo.min(g), hi.max(g)));
            let mean = gaps.iter().sum::<f64>() / gaps.len().max(1) as f64;
            eprintln!(
                "PROBE display {mon:?}: {:.4} Hz from the configuration; its refreshes waited on: {}, {} gaps, mean {:.3} ms ({:.4} Hz), {:.3} to {:.3} ms",
                period.map_or(0.0, |p| 1.0 / p),
                source.is_some(),
                gaps.len(),
                mean * 1000.0,
                1.0 / mean,
                lo * 1000.0,
                hi * 1000.0
            );
        }
        forget();
    }
}
