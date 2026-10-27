// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Springs: set by damping ratio and response time, they always move from their current
//! value and velocity, so any animation can be interrupted and retargeted mid-flight
//! without a jump. Portable: the frame clock that steps them belongs to each platform.

use std::cell::Cell;
use std::f32::consts::PI;

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

/// The display's timing at a frame, in seconds on the clock the frame's time is read from:
/// when one of its refreshes (a vblank) is, before or after now, and how long one lasts.
#[derive(Clone, Copy, Debug)]
pub struct Vsync {
    pub vblank: f64,
    pub period: f64,
}

/// The longest step a frame takes. A frame later than this (the system busy for a moment)
/// moves on this far and no further, so the motion carries on rather than leaping to its end.
const MAX_STEP: f64 = 1.0 / 20.0;

/// How far each frame moves the springs. A frame is seen at a display refresh, so it steps
/// by the whole refresh periods from the last frame's refresh to its own: one when it's on
/// time, two when it missed one, so the motion keeps to its time without a step of odd
/// length. The first frame after a rest steps by exactly one period, at the display's own
/// rate (the pose before it was on screen a refresh ago), however long the work before it
/// took. Portable: the platform reads the display's timing and the time.
#[derive(Clone, Copy, Debug)]
pub struct Pacer {
    /// When the last frame was drawn, and the refresh it was drawn for; None after a rest.
    last: Option<(f64, f64)>,
    period: f64,
    /// Whether the frames keep to the refreshes the timing describes. A frame drawn for the
    /// same refresh as the last one when most of a refresh has gone by says they don't (the
    /// frames are paced by another display than the one timed, or the wait returns just
    /// short of a refresh): the time since the last frame is the step then, until a rest.
    on_grid: bool,
}

impl Default for Pacer {
    fn default() -> Pacer {
        Pacer::new()
    }
}

impl Pacer {
    pub const fn new() -> Pacer {
        Pacer { last: None, period: 1.0 / 60.0, on_grid: true }
    }

    /// Nothing is moving: the next frame starts a new run.
    pub fn rest(&mut self) {
        self.last = None;
        self.on_grid = true;
    }

    /// The display's refresh period as last seen (1/60 s until then).
    pub fn period(&self) -> f32 {
        self.period as f32
    }

    /// The step for a frame drawn at `now`, given the display's timing (None where there's
    /// none to be had: the time since the last frame is the step then).
    pub fn step(&mut self, now: f64, vsync: Option<Vsync>) -> f32 {
        let v = vsync.filter(|v| v.period > 1.0 / 500.0 && v.period < 1.0 / 20.0 && v.vblank.is_finite());
        if let Some(v) = v {
            self.period = v.period;
        }
        // Shown at the first refresh after now: on the refreshes' grid, so frames on time
        // land exactly a period apart, whatever the jitter in when they were drawn.
        let at = v.map_or(now, |v| v.vblank + (((now - v.vblank) / v.period).floor() + 1.0) * v.period);
        let Some((then, last_at)) = self.last.replace((now, at)) else { return self.period as f32 };
        if let Some(v) = v.filter(|_| self.on_grid) {
            // Two frames drawn for the same refresh: the second shows the same moment.
            let periods = ((at - last_at) / v.period).round().max(0.0);
            if periods > 0.0 || now - then < 0.6 * v.period {
                let most = (MAX_STEP / v.period).floor().max(1.0);
                return (periods.min(most) * v.period) as f32;
            }
            self.on_grid = false;
        }
        (now - then).clamp(0.0, MAX_STEP) as f32
    }
}

thread_local! {
    static REDUCED: Cell<bool> = const { Cell::new(false) };
}

/// Reduce Motion: springs land on their target at once instead of moving.
pub fn set_reduced_motion(on: bool) {
    REDUCED.with(|r| r.set(on));
}

pub fn reduced() -> bool {
    REDUCED.with(|r| r.get())
}

/// Linear interpolation helpers.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::{Pacer, Vsync};

    const RATES: [f64; 4] = [60.0, 99.0, 120.0, 144.0];

    /// A display refreshing every `period` from `origin`, as the compositor reports it at
    /// time `t`: the refresh before `t`, or (`ahead`, as Windows 11 does) the one after it.
    fn vsync(origin: f64, period: f64, t: f64, ahead: bool) -> Vsync {
        let k = ((t - origin) / period).floor() + if ahead { 1.0 } else { 0.0 };
        Vsync { vblank: origin + k * period, period }
    }

    fn close(a: f32, b: f64) -> bool {
        (a as f64 - b).abs() < 1e-6
    }

    #[test]
    fn the_first_frame_after_a_rest_steps_one_refresh_of_the_display() {
        for hz in RATES {
            let p = 1.0 / hz;
            for ahead in [false, true] {
                let mut c = Pacer::new();
                // Drawn late in a refresh, after a window's picture and layout took most of it.
                let t = 1000.0 + 0.9 * p;
                assert!(close(c.step(t, Some(vsync(1000.0, p, t, ahead))), p), "{hz} Hz");
                assert!(close(c.period(), p));
                // And again after the next rest.
                c.rest();
                let t = 1003.0 + 0.1 * p;
                assert!(close(c.step(t, Some(vsync(1000.0, p, t, ahead))), p), "{hz} Hz after a rest");
            }
        }
    }

    #[test]
    fn frames_on_time_step_exactly_a_refresh_whenever_in_it_they_are_drawn() {
        for hz in RATES {
            let p = 1.0 / hz;
            for ahead in [false, true] {
                let mut c = Pacer::new();
                // Woken anywhere in a refresh, then drawn just after each one (as DwmFlush
                // returns) with a millisecond or two of jitter, up to most of the refresh.
                let phases = [0.37, 0.05, 0.6, 0.12, 0.08, 0.55, 0.02, 0.3];
                for (i, ph) in phases.iter().enumerate() {
                    let t = 50.0 + (i as f64 + ph) * p;
                    let dt = c.step(t, Some(vsync(50.0, p, t, ahead)));
                    assert!(close(dt, p), "{hz} Hz frame {i}: {dt}");
                }
            }
        }
    }

    #[test]
    fn a_late_frame_steps_the_refreshes_it_missed_and_the_next_one_a_refresh() {
        for hz in RATES {
            let p = 1.0 / hz;
            let mut c = Pacer::new();
            let mut steps = Vec::new();
            // Refreshes 0, 1, then 4 (a heavy frame missed two), 5, 6.
            for (n, ph) in [(0.0, 0.1), (1.0, 0.1), (4.0, 0.15), (5.0, 0.1), (6.0, 0.2)] {
                let t = 7.0 + (n + ph) * p;
                steps.push(c.step(t, Some(vsync(7.0, p, t, true))));
            }
            let want = [1.0, 1.0, 3.0, 1.0, 1.0];
            for (s, w) in steps.iter().zip(want) {
                assert!(close(*s, w * p), "{hz} Hz: {steps:?}");
            }
            // The motion keeps to the display's time: six refreshes, six refreshes' steps.
            assert!(close(steps.iter().sum(), 7.0 * p));
        }
    }

    #[test]
    fn two_frames_for_one_refresh_show_the_same_moment() {
        let p = 1.0 / 99.0;
        let mut c = Pacer::new();
        assert!(close(c.step(0.2 * p, Some(vsync(0.0, p, 0.2 * p, false))), p));
        assert!(close(c.step(0.5 * p, Some(vsync(0.0, p, 0.5 * p, false))), 0.0));
        assert!(close(c.step(1.1 * p, Some(vsync(0.0, p, 1.1 * p, false))), p));
    }

    #[test]
    fn a_long_stall_steps_no_more_than_a_twentieth_of_a_second_in_whole_refreshes() {
        for hz in RATES {
            let p = 1.0 / hz;
            let mut c = Pacer::new();
            c.step(0.1 * p, Some(vsync(0.0, p, 0.1 * p, false)));
            let t = 2.0 + 0.1 * p;
            let dt = c.step(t, Some(vsync(0.0, p, t, false))) as f64;
            assert!(dt <= 1.0 / 20.0 + 1e-9 && dt > 1.0 / 20.0 - p, "{hz} Hz: {dt}");
            let periods = dt / p;
            assert!((periods - periods.round()).abs() < 1e-6, "{hz} Hz: {periods} refreshes");
        }
    }

    #[test]
    fn frames_paced_by_another_display_than_the_one_timed_step_by_the_time() {
        // The compositor says 60 Hz; the frames come at 99 Hz (another monitor sets them).
        let (grid, p) = (1.0 / 60.0, 1.0 / 99.0);
        let mut c = Pacer::new();
        let mut moved = 0.0;
        let n = 40;
        for i in 0..n {
            let t = 3.0 + (i as f64 + 0.1) * p;
            let dt = c.step(t, Some(vsync(3.0, grid, t, true)));
            moved += dt as f64;
            // Once a frame came for a refresh already drawn for, every step is the time.
            if i > 3 {
                assert!(close(dt, p), "frame {i}: {dt}");
            }
        }
        // The motion kept to the time, give or take the refresh it was taken in by.
        let took = (n - 1) as f64 * p + 1.0 / 60.0;
        assert!((moved - took).abs() < 1.0 / 60.0, "{moved} for {took}");
        // A rest trusts the timing again.
        c.rest();
        let t = 9.0 + 0.1 * grid;
        c.step(t, Some(vsync(3.0, grid, t, true)));
        let t = 9.0 + 1.1 * grid;
        assert!(close(c.step(t, Some(vsync(3.0, grid, t, true))), grid));
    }

    #[test]
    fn without_the_display_timing_it_steps_by_the_time_since_the_last_frame() {
        let mut c = Pacer::new();
        assert!(close(c.step(10.0, None), 1.0 / 60.0));
        assert!(close(c.step(10.007, None), 0.007));
        assert!(close(c.step(12.0, None), 1.0 / 20.0));
        // Nonsense timing (a period of zero) is no timing.
        assert!(close(c.step(12.01, Some(Vsync { vblank: 0.0, period: 0.0 })), 0.01));
    }
}
