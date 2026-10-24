// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard focus in the custom-drawn windows: the Tab order and the focus ring.

use crate::win::render::gfx::{Canvas, Shape};

/// The ring's width and its gap from the control's edge (device-independent px).
const WIDTH: f32 = 2.0;
const GAP: f32 = 2.0;

/// Where focus goes from `cur` on Tab (or Shift+Tab, `back`): the next target in `order`
/// that `live` accepts. A current target that's gone resumes from its place in the
/// order; none at all starts from the end Tab enters from. Without `wrap`, focus stops
/// at the ends instead of coming round again.
pub fn step<T: Copy + PartialEq>(order: &[T], cur: Option<T>, back: bool, wrap: bool, live: impl Fn(T) -> bool) -> Option<T> {
    let n = order.len() as isize;
    let at = cur.and_then(|c| order.iter().position(|&t| t == c)).map(|i| i as isize);
    for k in 1..=n {
        let i = match (at, back) {
            (Some(a), false) => a + k,
            (Some(a), true) => a - k,
            (None, false) => k - 1,
            (None, true) => n - k,
        };
        let i = if wrap {
            i.rem_euclid(n)
        } else if (0..n).contains(&i) {
            i
        } else {
            break;
        };
        if live(order[i as usize]) {
            return Some(order[i as usize]);
        }
    }
    // Nowhere further: focus stays where it is, if that's still there.
    cur.filter(|&c| at.is_some() && live(c))
}

/// The focus ring around control `sh`: its corners follow the control's.
pub fn ring(c: &mut Canvas, sh: &Shape, rgb: u32, alpha: f32, s: f32) {
    c.shape(&sh.inset(-(GAP + WIDTH) * s), rgb, alpha, 1.0, WIDTH * s);
}

#[cfg(test)]
mod tests {
    use super::step;

    const ORDER: [char; 4] = ['a', 'b', 'c', 'd'];
    fn all(_: char) -> bool {
        true
    }

    #[test]
    fn forward_and_back() {
        assert_eq!(step(&ORDER, Some('a'), false, true, all), Some('b'));
        assert_eq!(step(&ORDER, Some('c'), true, true, all), Some('b'));
    }

    #[test]
    fn wraps_round() {
        assert_eq!(step(&ORDER, Some('d'), false, true, all), Some('a'));
        assert_eq!(step(&ORDER, Some('a'), true, true, all), Some('d'));
    }

    #[test]
    fn stops_at_the_ends_without_wrap() {
        assert_eq!(step(&ORDER, Some('d'), false, false, all), Some('d'));
        assert_eq!(step(&ORDER, Some('a'), true, false, all), Some('a'));
        assert_eq!(step(&ORDER, Some('b'), false, false, all), Some('c'));
    }

    #[test]
    fn starts_from_the_end_tab_enters() {
        assert_eq!(step(&ORDER, None, false, true, all), Some('a'));
        assert_eq!(step(&ORDER, None, true, true, all), Some('d'));
        // A target that isn't in the order at all counts as none.
        assert_eq!(step(&ORDER, Some('z'), false, true, all), Some('a'));
    }

    #[test]
    fn skips_targets_not_there() {
        let live = |t: char| t != 'b' && t != 'c';
        assert_eq!(step(&ORDER, Some('a'), false, true, live), Some('d'));
        assert_eq!(step(&ORDER, Some('d'), true, true, live), Some('a'));
        assert_eq!(step(&ORDER, None, false, true, |t| t == 'c'), Some('c'));
    }

    #[test]
    fn a_gone_target_resumes_from_its_place() {
        // Focus was on 'b', which has just gone: Tab goes on to 'c', Shift+Tab back to 'a'.
        let live = |t: char| t != 'b';
        assert_eq!(step(&ORDER, Some('b'), false, true, live), Some('c'));
        assert_eq!(step(&ORDER, Some('b'), true, true, live), Some('a'));
        assert_eq!(step(&ORDER, Some('d'), false, false, |t| t != 'd'), None);
    }

    #[test]
    fn a_lone_target_keeps_focus() {
        assert_eq!(step(&['a'], Some('a'), false, true, all), Some('a'));
        assert_eq!(step(&ORDER, Some('c'), false, true, |t| t == 'c'), Some('c'));
    }

    #[test]
    fn nothing_to_focus() {
        assert_eq!(step::<char>(&[], None, false, true, all), None);
        assert_eq!(step(&ORDER, Some('a'), false, true, |_| false), None);
    }
}
