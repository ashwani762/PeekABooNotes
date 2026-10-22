// Copyright (C) 2026 Ashwani Kumar Bachchan
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Path, PathBuf};

use embed_manifest::manifest::{ActiveCodePage, DpiAwareness, Setting};
use embed_manifest::{embed_manifest, new_manifest};

/// The folder the exe is built into (target\<profile>), from OUT_DIR
/// (target\<profile>\build\<crate>-<hash>\out).
fn exe_dir() -> Option<PathBuf> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR")?);
    out.ancestors().nth(3).map(Path::to_path_buf)
}

/// Put the string files beside the exe, in `lang\`, where translators can edit them and
/// add languages. (The built-in ones are also compiled in, so a lone exe still works.)
fn copy_strings() {
    let src = Path::new("src/i18n/strings");
    println!("cargo:rerun-if-changed={}", src.display());
    let Some(dir) = exe_dir() else { return };
    let dest = dir.join("lang");
    if std::fs::create_dir_all(&dest).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(src) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "xml") {
            println!("cargo:rerun-if-changed={}", p.display());
            let _ = std::fs::copy(&p, dest.join(e.file_name()));
        }
    }
}

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Common Controls v6 (visual styles, cue banners), per-monitor v2 DPI, UTF-8 code page.
        embed_manifest(
            new_manifest("Peekaboo.Notes")
                .dpi_awareness(DpiAwareness::PerMonitorV2)
                .active_code_page(ActiveCodePage::Utf8)
                .long_path_aware(Setting::Enabled),
        )
        .expect("unable to embed manifest");
    }
    copy_strings();
    println!("cargo:rerun-if-changed=build.rs");
}
