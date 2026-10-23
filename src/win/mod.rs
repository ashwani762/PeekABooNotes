// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Windows: Win32 windows and the message loop, RichEdit, DirectWrite, the tray, global
//! hotkeys, the registry, file dialogs and crash handling. `os` answers what the portable
//! code (`crate::common`) asks of the operating system.

pub mod app;
pub mod crash;
pub mod editor;
pub mod os;
pub mod render;
pub mod theme;
pub mod ui;
pub mod util;

/// Run the app; returns the process exit code.
pub fn run() -> i32 {
    crash::install();
    app::run()
}
