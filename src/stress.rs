//! In-process stress test. Build with `--features stress`, run with `--stress`.
//!
//! Drives the app through its own APIs (no synthetic keyboard input, so nothing can leak
//! into other applications): create a note, type into it, close it (ghost transitions
//! included), open and close All Notes, repeat — then quit. Resource usage is sampled
//! from outside while it runs.

use std::cell::Cell;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer, WM_CLOSE};

use crate::{ui::{allnotes, note}, app};

const ROUNDS: u32 = 60;
const TIMER: usize = 0x5157;

thread_local! {
    static STEP: Cell<u32> = const { Cell::new(0) };
}

pub fn start() {
    let main = app::ui(|u| u.main);
    unsafe { SetTimer(main, TIMER, 320, Some(tick)) };
}

fn newest_open() -> Option<HWND> {
    app::ui(|u| u.notes.iter().max_by_key(|(id, _)| **id).map(|(_, h)| *h))
}

unsafe extern "system" fn tick(hwnd: HWND, _msg: u32, id: usize, _t: u32) {
    let step = STEP.with(|s| {
        let v = s.get();
        s.set(v + 1);
        v
    });
    let (round, phase) = (step / 4, step % 4);
    if round >= ROUNDS {
        unsafe { KillTimer(hwnd, id) };
        app::quit();
        return;
    }
    match phase {
        0 => app::new_note(),
        1 => {
            if let Some(h) = newest_open() {
                note::stress_type(h, &format!("Stress note {round}\n- first item\n- second item\n[ ] a task\nSome longer body text to wrap across the width of the card."));
            }
        }
        2 => {
            if let Some(h) = newest_open() {
                unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
            }
            if round % 3 == 0 {
                allnotes::show(false);
            }
        }
        _ => {
            let an = allnotes::hwnd();
            if !an.is_null() {
                unsafe { PostMessageW(an, WM_CLOSE, 0, 0) };
            }
        }
    }
}
