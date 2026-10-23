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
