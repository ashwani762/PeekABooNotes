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
    /// A fade (opacity), which Reduce Motion keeps: it moves nothing on screen.
    keep: bool,
}

impl Spring {
    /// A spring for pixel-sized quantities.
    pub fn px(value: f32, params: (f32, f32)) -> Spring {
        Spring { value, velocity: 0.0, target: value, damping: params.0, response: params.1, eps: 0.15, keep: false }
    }

    /// A spring for 0..1 progress / opacity / scale quantities.
    pub fn unit(value: f32, params: (f32, f32)) -> Spring {
        Spring { value, velocity: 0.0, target: value, damping: params.0, response: params.1, eps: 0.002, keep: false }
    }

    /// A 0..1 opacity that goes on fading with Reduce Motion on (a cross-fade moves
    /// nothing), where other springs land at once.
    pub fn fade(value: f32, params: (f32, f32)) -> Spring {
        Spring { keep: true, ..Spring::unit(value, params) }
    }

    /// This spring, settling only within `eps` of its target: a 0..1 spring that moves
    /// something hundreds of pixels lands within a fraction of a pixel rather than snapping
    /// the last half pixel in a frame.
    pub fn fine(mut self, eps: f32) -> Spring {
        self.eps = eps;
        self
    }

    pub fn to(&mut self, target: f32) {
        self.target = target;
    }

    /// Head for `target` and, from rest, set off at once: with half the speed a critically
    /// damped spring would leave with, so it moves from the first frame and slows from
    /// there, with no slow first step and a leap in the second (a spring let go from rest
    /// moves 2.2 times as far in its second frame as in its first). Already moving, it
    /// keeps its speed, as `to` does; and asked again for the target it has, it does
    /// nothing (called every frame, a kick each time it slowed near its end would have
    /// drawn that end out).
    pub fn launch(&mut self, target: f32) {
        let new = target != self.target;
        self.target = target;
        if new && self.velocity.abs() < self.eps * 8.0 {
            self.velocity = (target - self.value) * PI / self.response;
        }
    }

    /// `launch`, with new damping and response.
    pub fn launch_as(&mut self, target: f32, params: (f32, f32)) {
        self.damping = params.0;
        self.response = params.1;
        self.launch(target);
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
        if self.settled() || (reduced() && !self.keep) {
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

/// A display's timing at a frame, in seconds on the clock the frame's time is read from:
/// how long one of its refreshes lasts, when one of them (a vblank) is, before or after now,
/// when that's known, and which display it is (frames moving to another one meet another
/// grid of refreshes).
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub period: f64,
    pub vblank: Option<f64>,
    pub display: u64,
}

/// The longest step a frame takes where the display's refreshes aren't known. A frame later
/// than this (the system busy for a moment) moves on this far and no further, so the motion
/// carries on rather than leaping to its end.
const MAX_STEP: f64 = 1.0 / 20.0;

/// How long without a frame, in refreshes, is the app having been busy (a delete, an undo, a
/// window made) rather than frames paced unevenly: the next frame steps one refresh then.
const BLOCKED: f64 = 2.5;

/// How far each frame moves the springs. A frame is seen at a refresh of the display it's
/// on, so it steps by one of that display's refresh periods, at whatever rate it runs (60,
/// 99.99, 144 Hz), and none when it's drawn for the refresh the last one was. One that
/// missed refreshes, the app busy, steps one too: the motion runs late rather than leaping
/// (stepping the refreshes it missed, up to a twentieth of a second, a toast starting across
/// a 100 ms delete came up 55% opaque in its first frame). The first frame after a rest
/// steps by exactly one period (the pose before it was on screen a refresh ago), however
/// long the work before it took; a step onto another display's refreshes goes by the time.
/// Portable: the platform reads the display's timing and the time.
#[derive(Clone, Copy, Debug)]
pub struct Pacer {
    /// When the last frame was drawn, the refresh it was drawn for (when the display's
    /// refreshes were known) and the display; None after a rest.
    last: Option<(f64, Option<f64>, u64)>,
    period: f64,
    /// Whether the frames keep to the refreshes the timing describes. A frame drawn for the
    /// same refresh as the last one when most of a refresh has gone by says they don't (the
    /// frames are paced by another display than the one timed): the time since the last
    /// frame is the step then, until a rest.
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

    /// The step for a frame drawn at `now`, given the timing of the display it's on (None
    /// where there's none to be had: the time since the last frame is the step then).
    pub fn step(&mut self, now: f64, timing: Option<Timing>) -> f32 {
        let t = timing.filter(|t| t.period > 1.0 / 500.0 && t.period < 1.0 / 20.0);
        if let Some(t) = t {
            self.period = t.period;
        }
        // Shown at the first refresh after now: on the refreshes' grid, so frames on time
        // land exactly a period apart, whatever the jitter in when they were drawn.
        let at = t.and_then(|t| t.vblank.filter(|v| v.is_finite()).map(|v| v + (((now - v) / t.period).floor() + 1.0) * t.period));
        let display = t.map_or(0, |t| t.display);
        let Some((then, last_at, last_display)) = self.last.replace((now, at, display)) else { return self.period as f32 };
        let elapsed = now - then;
        // Without the refreshes to go by, the time since the last frame, but a refresh after
        // the app was busy.
        let by_time = |period: f64| if elapsed > BLOCKED * period { period } else { elapsed.clamp(0.0, MAX_STEP) };
        let Some(t) = t.filter(|_| self.on_grid) else { return by_time(self.period) as f32 };
        let periods = match (at, last_at) {
            (Some(at), Some(last_at)) if display == last_display => {
                // Two frames drawn for the same refresh: the second shows the same moment.
                let n = ((at - last_at) / t.period).round().max(0.0);
                if n == 0.0 && elapsed >= 0.6 * t.period {
                    self.on_grid = false;
                    return by_time(t.period) as f32;
                }
                // Refreshes missed (the app busy): one, all the same.
                n.min(1.0)
            }
            // The refreshes not known on one side (the first frame on a display, or moved to
            // another): the refreshes the time since the last frame comes to, one at least.
            _ => (elapsed / t.period).round().max(1.0),
        };
        let most = (MAX_STEP / t.period).floor().max(1.0);
        (periods.min(most) * t.period) as f32
    }
}

/// What to wait for before the next frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wait {
    /// The display's next refresh, whenever it comes (none seen yet to time it by).
    Refresh,
    /// This many seconds.
    For(f64),
    /// Nothing: start the next frame at once.
    No,
}

/// What to wait for, at `now`, before drawing the next frame, when this one's turn (its
/// messages, painting and frame) began at `began`, on a display refreshing every `period`
/// (one of its refreshes at `vblank`, when known): until a moment (`lead`) after the first
/// refresh since the turn began, when the compositor has taken what was drawn for it (on
/// Windows its own wait returns about 2 ms after a refresh). Nothing when that's past
/// already (a turn that took a refresh or more, or waited for the screen itself, as a paint
/// that reads a window back does): the next one starts at once, so a turn never waits a
/// second refresh and halves the frame rate.
pub fn plan_wait(began: f64, now: f64, period: f64, vblank: Option<f64>) -> Wait {
    let lead = 0.002f64.min(0.2 * period);
    let Some(v) = vblank else { return Wait::Refresh };
    // A turn begun a hair before its moment (a timer's rounding) still counts as begun at it.
    let early = 0.0005f64.min(0.1 * period);
    let next = v + (((began + early - lead - v) / period).floor() + 1.0) * period + lead;
    if now < next { Wait::For(next - now) } else { Wait::No }
}

/// Which moving parts (bits of `active`) to ask, in turn, for the window their motion is
/// on, to pace the frames by its display: the one that last asked for frames first (what
/// was just set moving is what's being watched), then the others, lowest bit first.
pub fn pace_order(last: u32, active: u32) -> impl Iterator<Item = u32> {
    let first = (last & active != 0).then_some(last);
    first.into_iter().chain((0..32).map(|i| 1u32 << i).filter(move |&b| active & b != 0 && b != last))
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

/// A fade out over a set time that comes to nothing exactly at its end, never leaving a
/// faint remainder to be cut (a spring only nears its end: a window hidden once it was
/// faint enough vanished from 2 to 5 levels of 255 in one frame). It falls fastest at
/// first and eases into its end, the last steps a level or two.
#[derive(Clone, Copy, Debug)]
pub struct FadeOut {
    from: f32,
    t: f32,
    dur: f32,
}

impl FadeOut {
    /// From opacity `from`, over `dur` seconds.
    pub fn new(from: f32, dur: f32) -> FadeOut {
        FadeOut { from, t: 0.0, dur: dur.max(1e-3) }
    }

    /// `dt` seconds on: the opacity now.
    pub fn step(&mut self, dt: f32) -> f32 {
        self.t = (self.t + dt).min(self.dur);
        self.value()
    }

    pub fn value(&self) -> f32 {
        let k = 1.0 - self.t / self.dur;
        self.from * k * k
    }

    /// Faded to nothing.
    pub fn done(&self) -> bool {
        self.t >= self.dur
    }
}

/// A move from where it is to a target over a set time, along a quarter of a sine: it sets
/// off at an even pace (its first steps alike, never a slow first one or a leap) and slows
/// into its end, which it reaches exactly and on time. For what must start gently from
/// nothing and come to rest without a tail: a toast's fade and rise.
#[derive(Clone, Copy, Debug)]
pub struct Ease {
    from: f32,
    to: f32,
    t: f32,
    dur: f32,
}

impl Ease {
    /// At `v`, still.
    pub fn at(v: f32) -> Ease {
        Ease { from: v, to: v, t: 1.0, dur: 1.0 }
    }

    /// From where it is now to `to`, over `dur` seconds. Already on its way there, it
    /// carries on.
    pub fn go(&mut self, to: f32, dur: f32) {
        if to == self.to {
            return;
        }
        self.from = self.value();
        self.to = to;
        self.t = 0.0;
        self.dur = dur.max(1e-3);
    }

    /// `dt` seconds on: whether it's still on its way.
    pub fn step(&mut self, dt: f32) -> bool {
        let moving = self.t < self.dur;
        self.t = (self.t + dt).min(self.dur);
        moving
    }

    pub fn value(&self) -> f32 {
        let u = self.t / self.dur;
        self.from + (self.to - self.from) * (u * std::f32::consts::FRAC_PI_2).sin()
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn done(&self) -> bool {
        self.t >= self.dur
    }
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
    use super::{Pacer, Timing, Wait, pace_order, plan_wait};

    #[test]
    fn frames_follow_the_part_set_moving_last_then_the_others() {
        // The deck (1) and a toast (4) moving, the toast set off last: the toast's display.
        assert_eq!(pace_order(4, 1 | 4).collect::<Vec<_>>(), vec![4, 1]);
        // The part set off last has come to rest: the others, in order.
        assert_eq!(pace_order(32, 1 | 8 | 128).collect::<Vec<_>>(), vec![1, 8, 128]);
        assert_eq!(pace_order(0, 0).count(), 0);
    }

    /// The refresh rates the clock is tried at: the LG's 99.99 Hz is 100,000/1,000,1 as the
    /// display configuration gives it.
    const RATES: [f64; 5] = [60.0, 100_000.0 / 1_000.1, 99.0, 120.0, 144.0];

    /// Display `display`, refreshing every `period` from `origin`, as the platform reports it
    /// at time `t`: a refresh before `t`, or (`ahead`, as Windows 11's compositor does) after it.
    fn timing(display: u64, origin: f64, period: f64, t: f64, ahead: bool) -> Option<Timing> {
        let k = ((t - origin) / period).floor() + if ahead { 1.0 } else { 0.0 };
        Some(Timing { period, vblank: Some(origin + k * period), display })
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
                assert!(close(c.step(t, timing(1, 1000.0, p, t, ahead)), p), "{hz} Hz");
                assert!(close(c.period(), p));
                // And again after the next rest.
                c.rest();
                let t = 1003.0 + 0.1 * p;
                assert!(close(c.step(t, timing(1, 1000.0, p, t, ahead)), p), "{hz} Hz after a rest");
            }
        }
    }

    #[test]
    fn frames_on_time_step_exactly_a_refresh_whenever_in_it_they_are_drawn() {
        for hz in RATES {
            let p = 1.0 / hz;
            for ahead in [false, true] {
                let mut c = Pacer::new();
                // Woken anywhere in a refresh, then drawn just after each one with a
                // millisecond or two of jitter, up to most of the refresh.
                let phases = [0.37, 0.05, 0.6, 0.12, 0.08, 0.55, 0.02, 0.3];
                for (i, ph) in phases.iter().enumerate() {
                    let t = 50.0 + (i as f64 + ph) * p;
                    let dt = c.step(t, timing(1, 50.0, p, t, ahead));
                    assert!(close(dt, p), "{hz} Hz frame {i}: {dt}");
                }
            }
        }
    }

    #[test]
    fn a_late_frame_steps_one_refresh_and_the_motion_runs_late_rather_than_leaping() {
        for hz in RATES {
            let p = 1.0 / hz;
            let mut c = Pacer::new();
            let mut steps = Vec::new();
            // Refreshes 0, 1, then 4 (a heavy frame missed two), 5, 6.
            for (n, ph) in [(0.0, 0.1), (1.0, 0.1), (4.0, 0.15), (5.0, 0.1), (6.0, 0.2)] {
                let t = 7.0 + (n + ph) * p;
                steps.push(c.step(t, timing(1, 7.0, p, t, true)));
            }
            for s in &steps {
                assert!(close(*s, p), "{hz} Hz: {steps:?}");
            }
        }
    }

    #[test]
    fn two_frames_for_one_refresh_show_the_same_moment() {
        let p = 1.0 / 99.0;
        let mut c = Pacer::new();
        assert!(close(c.step(0.2 * p, timing(1, 0.0, p, 0.2 * p, false)), p));
        assert!(close(c.step(0.5 * p, timing(1, 0.0, p, 0.5 * p, false)), 0.0));
        assert!(close(c.step(1.1 * p, timing(1, 0.0, p, 1.1 * p, false)), p));
    }

    #[test]
    fn after_the_app_was_busy_100_ms_the_next_frame_steps_one_refresh() {
        for hz in [60.0, 100_000.0 / 1_000.1] {
            let p = 1.0 / hz;
            // A run going on time, then 100 ms of work (a delete, an undo), then on time.
            let mut c = Pacer::new();
            let mut t = 5.0 + 0.1 * p;
            let mut steps = vec![c.step(t, timing(1, 5.0, p, t, false))];
            for _ in 0..3 {
                t += p;
                steps.push(c.step(t, timing(1, 5.0, p, t, false)));
            }
            t += 0.100;
            steps.push(c.step(t, timing(1, 5.0, p, t, false)));
            t += p;
            steps.push(c.step(t, timing(1, 5.0, p, t, false)));
            for s in &steps {
                assert!(close(*s, p), "{hz} Hz: {steps:?}");
            }
            // A fade set off across the stall comes up one refresh's worth, not 55%.
            let mut fade = crate::common::anim::Spring::fade(0.0, (1.0, 0.18));
            fade.to(1.0);
            fade.step(steps[4]);
            assert!(fade.value < 0.15, "{hz} Hz: {}", fade.value);
            // So too frames paced by another display than the one timed, and with no timing.
            let mut c = Pacer::new();
            c.step(1.0, timing(1, 0.0, p, 1.0, false));
            c.step(1.0 + 0.3 * p, timing(1, 0.0, p, 1.0 + 0.3 * p, false));
            c.step(1.0 + 0.95 * p, timing(1, 0.0, p, 1.0 + 0.95 * p, false));
            let off = c.step(1.0 + 0.95 * p + 0.100, timing(1, 0.0, p, 1.0 + 0.95 * p + 0.100, false));
            assert!(close(off, p), "{hz} Hz: {off}");
            let mut c = Pacer::new();
            c.step(2.0, Some(Timing { period: p, vblank: None, display: 1 }));
            c.step(2.0 + p, None);
            assert!(close(c.step(2.0 + p + 0.100, None), p), "{hz} Hz");
        }
    }

    #[test]
    fn a_display_whose_refreshes_arent_known_yet_steps_whole_refreshes_by_the_time() {
        // The first frame of a run is drawn before the display's first refresh is seen.
        for hz in RATES {
            let p = 1.0 / hz;
            let mut c = Pacer::new();
            let unknown = Some(Timing { period: p, vblank: None, display: 1 });
            assert!(close(c.step(3.0, unknown), p));
            // The next, just after a refresh 0.3 refreshes on: one refresh, never none.
            let t = 3.0 + 0.3 * p;
            assert!(close(c.step(t, timing(1, 3.0 + 0.28 * p, p, t, false)), p), "{hz} Hz");
            let t = 3.0 + 1.3 * p;
            assert!(close(c.step(t, timing(1, 3.0 + 0.28 * p, p, t, false)), p), "{hz} Hz");
        }
    }

    #[test]
    fn mixed_displays_each_keep_to_their_own_refreshes() {
        // The LG at 99.99 Hz and the other display at 60, each with its own refreshes; the
        // compositor's one clock is neither's business here: a card flies on the LG, then
        // across onto the 60 Hz display, paced by each one's refreshes in turn.
        let (lg, other) = (1.0 / (100_000.0 / 1_000.1), 1.0 / 60.0);
        let mut c = Pacer::new();
        let (mut t, mut moved) = (20.0 + 0.1 * lg, 0.0);
        let start = t - lg;
        for i in 0..30 {
            let dt = c.step(t, timing(1, 20.0, lg, t, false));
            assert!(close(dt, lg), "on the LG, frame {i}: {dt}");
            moved += dt as f64;
            t += lg;
        }
        // Onto the other display, whose refreshes fall anywhere against the LG's.
        let origin = t - 0.43 * other;
        let mut t = origin + other + 0.1 * other;
        for i in 0..30 {
            let dt = c.step(t, timing(2, origin, other, t, true));
            assert!(close(dt, other), "on the 60 Hz display, frame {i}: {dt}");
            moved += dt as f64;
            t += other;
        }
        // The motion kept to the time, give or take a refresh at the crossing.
        let took = t - other - start;
        assert!((moved - took).abs() < other, "{moved} for {took}");
    }

    #[test]
    fn frames_paced_by_another_display_than_the_one_timed_step_by_the_time() {
        // Timed by a 60 Hz clock, the frames come at 99.99 Hz (another display sets them).
        let (grid, p) = (1.0 / 60.0, 1.0 / (100_000.0 / 1_000.1));
        let mut c = Pacer::new();
        let mut moved = 0.0;
        let n = 40;
        for i in 0..n {
            let t = 3.0 + (i as f64 + 0.1) * p;
            let dt = c.step(t, timing(1, 3.0, grid, t, true));
            moved += dt as f64;
            // Once a frame came for a refresh already drawn for, every step is the time.
            if i > 3 {
                assert!(close(dt, p), "frame {i}: {dt}");
            }
        }
        let took = (n - 1) as f64 * p + grid;
        assert!((moved - took).abs() < grid, "{moved} for {took}");
        // A rest trusts the timing again.
        c.rest();
        let t = 9.0 + 0.1 * grid;
        c.step(t, timing(1, 3.0, grid, t, true));
        let t = 9.0 + 1.1 * grid;
        assert!(close(c.step(t, timing(1, 3.0, grid, t, true)), grid));
    }

    #[test]
    fn a_turn_waits_for_the_refresh_after_it_began_and_never_for_a_second_one() {
        for hz in RATES {
            let p = 1.0 / hz;
            let lead = 0.002f64.min(0.2 * p);
            let until = |began: f64, now: f64| match plan_wait(began, now, p, Some(0.0)) {
                Wait::For(w) => Some(now + w),
                Wait::No => None,
                Wait::Refresh => panic!("{hz} Hz: the refreshes are known"),
            };
            let at = |n: f64| Some(n * p + lead);
            let near = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-9,
                (a, b) => a == b,
            };
            // Begun just after refresh 10 (as the last wait returned), done 3 ms later: it
            // waits until just after refresh 11.
            let began = 10.0 * p + lead + 0.0001;
            assert!(near(until(began, began + 0.003f64.min(0.5 * p)), at(11.0)), "{hz} Hz");
            // Woken a hair early by the timer: the same refresh, not the one before.
            let began = 10.0 * p + lead - 0.0001;
            assert!(near(until(began, began + 0.001), at(11.0)), "{hz} Hz early");
            // Done just after refresh 11: only the rest of the lead.
            assert!(near(until(10.0 * p + lead + 0.0001, 11.0 * p + 0.0004f64.min(0.1 * p)), at(11.0)), "{hz} Hz");
            // A turn that waited for the screen itself, or took longer than a refresh: past
            // its moment already, so the next starts at once.
            assert_eq!(until(10.0 * p + lead + 0.0001, 11.0 * p + lead + 0.0002), None, "{hz} Hz");
            // Begun late in a refresh (the turn before ran over): the next refresh, not the one after.
            assert!(near(until(10.8 * p, 10.8 * p + 0.0001), at(11.0)), "{hz} Hz late");
            // A display whose refreshes aren't known yet: its next one, whenever it comes.
            assert_eq!(plan_wait(10.8 * p, 10.8 * p, p, None), Wait::Refresh);
        }
    }

    #[test]
    fn without_the_display_timing_it_steps_by_the_time_since_the_last_frame() {
        let mut c = Pacer::new();
        assert!(close(c.step(10.0, None), 1.0 / 60.0));
        assert!(close(c.step(10.007, None), 0.007));
        // Long after (the app busy): a refresh, as the last one known lasted.
        assert!(close(c.step(12.0, None), 1.0 / 60.0));
        // Nonsense timing (a period of zero) is no timing.
        assert!(close(c.step(12.01, Some(Timing { period: 0.0, vblank: Some(0.0), display: 1 })), 0.01));
    }
}

#[cfg(test)]
mod spring_tests {
    use super::{Spring, set_reduced_motion};

    /// Each frame's step of a spring heading from 0 to 100 at `hz`, until it rests.
    fn steps(mut sp: Spring, hz: f32) -> Vec<f32> {
        let mut v = Vec::new();
        let mut last = sp.value;
        while sp.step(1.0 / hz) && v.len() < 400 {
            v.push(sp.value - last);
            last = sp.value;
        }
        v
    }

    /// The largest ratio of a step to the one before it, over the steps of at least a pixel
    /// (the tail's tiny steps aren't seen).
    fn worst_leap(v: &[f32]) -> f32 {
        v.windows(2).filter(|w| w[1].abs() >= 1.0).map(|w| w[1] / w[0].abs().max(1e-3)).fold(0.0, f32::max)
    }

    #[test]
    fn a_launched_spring_moves_from_its_first_frame_without_a_leap() {
        for params in [(1.0, 0.32), (1.0, 0.2), (1.0, 0.12), (0.9, 0.26)] {
            for hz in [60.0, 99.99, 144.0] {
                let mut sp = Spring::px(0.0, params);
                sp.to(100.0);
                let from_rest = steps(sp, hz);
                let mut sp = Spring::px(0.0, params);
                sp.launch(100.0);
                let launched = steps(sp, hz);
                // Let go from rest, the second step is twice the first or more: a leap
                // (unless the whole move is over in a few frames).
                if params.1 >= 0.2 {
                    assert!(from_rest[1] / from_rest[0] > 1.6, "{params:?} at {hz} Hz: {from_rest:?}");
                }
                // Launched, no step is more than 1.6 times the one before it, and the first
                // is a real move.
                assert!(worst_leap(&launched) < 1.6, "{params:?} at {hz} Hz: {launched:?}");
                assert!(launched[0] > 0.5 * launched[1], "{params:?} at {hz} Hz: {launched:?}");
                // And it gets there no later.
                assert!(launched.len() <= from_rest.len() + 2, "{params:?} at {hz} Hz");
            }
        }
    }

    #[test]
    fn a_fade_out_comes_to_nothing_smoothly_and_on_time() {
        for hz in [60.0f32, 99.99, 144.0] {
            let mut f = super::FadeOut::new(1.0, 0.12);
            let mut levels = vec![255.0];
            while !f.done() {
                levels.push((f.step(1.0 / hz) * 255.0).round());
            }
            // Down to exactly nothing, each step no bigger than the one before, and the
            // last ones a level or two: nothing left over to cut.
            assert_eq!(*levels.last().unwrap(), 0.0);
            let steps: Vec<f32> = levels.windows(2).map(|w| w[0] - w[1]).collect();
            assert!(steps.windows(2).all(|w| w[1] <= w[0] + 1.0), "{hz} Hz: {levels:?}");
            assert!(steps[steps.len() - 1] <= 3.0, "{hz} Hz: {levels:?}");
            assert!(levels.len() as f32 <= 0.12 * hz + 2.0);
        }
    }

    #[test]
    fn an_ease_sets_off_evenly_and_lands_on_time() {
        for hz in [60.0f32, 100.0, 144.0] {
            let mut e = super::Ease::at(0.0);
            e.go(1.0, 0.22);
            let mut v = vec![0.0];
            while e.step(1.0 / hz) {
                v.push(e.value());
            }
            let steps: Vec<f32> = v.windows(2).map(|w| w[1] - w[0]).collect();
            // The first frame a small step, the next no bigger, each after no bigger than
            // the one before; exactly there at the end.
            assert!(steps[0] < 0.12 && steps[0] > 0.04, "{hz} Hz: {steps:?}");
            assert!(steps.windows(2).all(|w| w[1] <= w[0] + 1e-6), "{hz} Hz: {steps:?}");
            assert_eq!(*v.last().unwrap(), 1.0);
        }
    }

    #[test]
    fn launching_again_for_the_same_target_changes_nothing() {
        let mut sp = Spring::px(0.0, (1.0, 0.32));
        sp.launch(100.0);
        for _ in 0..30 {
            sp.step(1.0 / 60.0);
        }
        let v = sp.velocity;
        sp.launch(100.0);
        assert_eq!(sp.velocity, v);
    }

    #[test]
    fn launching_a_moving_spring_keeps_its_speed() {
        let mut sp = Spring::px(0.0, (1.0, 0.32));
        sp.to(100.0);
        for _ in 0..5 {
            sp.step(1.0 / 60.0);
        }
        let v = sp.velocity;
        sp.launch(0.0);
        assert_eq!(sp.velocity, v);
    }

    #[test]
    fn reduce_motion_lands_moves_at_once_and_keeps_fades() {
        set_reduced_motion(true);
        let mut moving = Spring::unit(0.0, (1.0, 0.2));
        moving.to(1.0);
        assert!(!moving.step(1.0 / 60.0));
        assert_eq!(moving.value, 1.0);
        let mut fade = Spring::fade(0.0, (1.0, 0.2));
        fade.to(1.0);
        assert!(fade.step(1.0 / 60.0));
        assert!(fade.value > 0.0 && fade.value < 0.5);
        set_reduced_motion(false);
    }
}
