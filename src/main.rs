// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! PeekABoo Notes: notes that live on the edge of your screen.
//!
//! `common` is the portable app (notes, storage, strings, design tokens, springs, the
//! log). Each platform module puts it on screen with that system's own windows, text and
//! services; today that's `win` (pure Win32 + RichEdit, no web view and no runtime: one
//! small exe that idles at a few megabytes with zero CPU). Linux and macOS go beside it.

#![windows_subsystem = "windows"]

#[macro_use]
mod common; // first, so t!/tn! are in scope everywhere

#[cfg(windows)]
mod win;

/// What `common` needs from the operating system, from the platform we're built for.
#[cfg(windows)]
use win::os;

#[cfg(not(windows))]
compile_error!("PeekABoo Notes runs on Windows for now; Linux and macOS support comes as a platform module beside `win`.");

fn main() {
    std::process::exit(win::run());
}
