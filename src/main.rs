//! Peekaboo Notes — notes that live on the edge of your screen, for Windows.
//!
//! Pure Win32 + RichEdit, no web view and no runtime: the whole app is one small exe
//! that idles at a few megabytes with zero CPU.

#![windows_subsystem = "windows"]

#[macro_use]
mod i18n; // first, so t!/tn! are in scope everywhere

mod app;
mod editor;
mod platform;
mod render;
mod store;
mod theme;
mod ui;

fn main() {
    platform::crash::install();
    std::process::exit(app::run());
}
