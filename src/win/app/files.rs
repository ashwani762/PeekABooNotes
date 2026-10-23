// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files and the system: import, export, the notes folder, start with Windows.

use super::*;

pub fn open_notes_folder() {
    open_path(&data_dir());
}

/// Outcome of an import or export: `None` when the dialog was cancelled, else how many
/// notes went through (`Err` when none could be).
pub type Outcome = Option<Result<usize, ()>>;

pub fn export_all(owner: HWND) -> Outcome {
    let ids: Vec<u64> = store(|s| s.d.notes.iter().map(|n| n.id).collect());
    export(owner, &ids)
}

pub(super) fn open_path(p: &std::path::Path) {
    let w = wide(&p.to_string_lossy());
    unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), w.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
}

/// Write the notes as Markdown files into a folder the user picks, then show it.
pub fn export(owner: HWND, ids: &[u64]) -> Outcome {
    if ids.is_empty() {
        return None;
    }
    let last = store(|s| s.d.settings.export_dir.clone());
    let dir = pick_folder(owner, &tn!("dialog_export_title", ids.len()), &last)?;
    let chosen = dir.to_string_lossy().into_owned();
    if chosen != last {
        store_mut(|s| s.d.settings.export_dir = chosen);
        save();
    }
    let notes: Vec<(String, String)> = store(|s| ids.iter().filter_map(|id| s.note(*id)).map(|n| (n.rtf.clone(), n.text.clone())).collect());
    let mut used = HashSet::new();
    let (mut written, mut failed) = (0, None);
    let total = notes.len();
    for (rtf, text) in notes {
        let md = if rtf.is_empty() { text.clone() } else { rtf_to_markdown(&rtf) };
        let base = file_safe(&title_of(&text));
        let mut name = format!("{base}.md");
        let mut k = 2;
        // Never overwrite: not a file already in the folder, nor one written just now.
        while dir.join(&name).exists() || !used.insert(name.to_lowercase()) {
            name = format!("{base} ({k}).md");
            k += 1;
        }
        let md = export_attachments(&md, &dir);
        match std::fs::write(dir.join(name), md + "\n") {
            Ok(()) => written += 1,
            Err(e) => failed = Some(e),
        }
    }
    match &failed {
        Some(e) => log::warn(&format!("export: wrote {written} of {total} notes; last error: {e}")),
        None => log::info(&format!("export: wrote {written} notes")),
    }
    if written == 0 {
        return Some(Err(()));
    }
    open_path(&dir);
    Some(Ok(written))
}

/// Copy the attachments exported Markdown `md` points at into `dir`/attachments, so the
/// files travel with it. A different file already there under the same name gets a
/// free name, and the link follows it.
fn export_attachments(md: &str, dir: &std::path::Path) -> String {
    use crate::common::attach;
    let prefix = format!("]({}/", attach::EXPORT_DIR);
    let mut out = md.to_string();
    let mut from = 0;
    while let Some(i) = out[from..].find(&prefix).map(|i| from + i) {
        let start = i + prefix.len();
        let Some(end) = out[start..].find(')').map(|e| start + e) else { break };
        from = end;
        let Some(name) = attach::name_of(&format!("{}{}", attach::SCHEME, &out[start..end])) else { continue };
        let src = attach::dir().join(&name);
        let Ok(meta) = std::fs::metadata(&src) else { continue };
        let target_dir = dir.join(attach::EXPORT_DIR);
        if std::fs::create_dir_all(&target_dir).is_err() {
            continue;
        }
        let same = std::fs::metadata(target_dir.join(&name)).is_ok_and(|m| m.len() == meta.len());
        let used = if same { name.clone() } else { attach::free_name(&target_dir, &name) };
        if !same && let Err(e) = std::fs::copy(&src, target_dir.join(&used)) {
            log::warn(&format!("export: couldn't copy attachment: {e}"));
            continue;
        }
        if used != name {
            let link = attach::export_link(&used);
            out.replace_range(i + 2..end, &link);
            from = i + 2 + link.len();
        }
    }
    out
}

/// The system folder picker, opening in `start` when that folder still exists.
fn pick_folder(owner: HWND, title: &str, start: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    let pick = || -> windows::core::Result<Option<std::path::PathBuf>> {
        unsafe {
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            d.SetOptions(d.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
            d.SetTitle(&windows::core::HSTRING::from(title))?;
            d.SetOkButtonLabel(&windows::core::HSTRING::from(t!("dialog_export_ok")))?;
            if !start.is_empty()
                && std::path::Path::new(start).is_dir()
                && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&windows::core::HSTRING::from(start), None)
            {
                let _ = d.SetFolder(&item);
            }
            d.Show(Some(windows::Win32::Foundation::HWND(owner)))?;
            let path = d.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let s = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as _));
            Ok(s.map(Into::into))
        }
    };
    // COM only for the dialog's lifetime: uninitialising unloads the shell extensions it
    // pulled in now, rather than leaving them to tear down at exit.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let path = pick().unwrap_or_else(|e| {
        // Cancel arrives as an error too; only real failures are worth a line.
        if e.code() != windows::core::HRESULT::from_win32(windows_sys::Win32::Foundation::ERROR_CANCELLED) {
            log::warn(&format!("folder picker failed: {:#010x} {}", e.code().0, e.message()));
        }
        None
    });
    if com.is_ok() {
        unsafe { CoUninitialize() };
    }
    path
}

pub fn import(owner: HWND) -> Outcome {
    let mut buf = vec![0u16; 65536];
    let filter: Vec<u16> = format!("{} (*.md, *.txt)\0*.md;*.markdown;*.txt\0{}\0*.*\0\0", t!("dialog_filter_text"), t!("dialog_filter_all")).encode_utf16().collect();
    let title = wide(t!("dialog_import_title"));
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        // 0 is a plain cancel; anything else failed (such as too many files for the buffer).
        let err = unsafe { CommDlgExtendedError() };
        if err != 0 {
            log::warn(&format!("import: file dialog failed ({err:#x})"));
        }
        return None;
    }
    let parts: Vec<String> = buf.split(|&c| c == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy).collect();
    let files: Vec<std::path::PathBuf> = match parts.as_slice() {
        [] => return None,
        [one] => vec![one.into()],
        [dir, names @ ..] => names.iter().map(|n| std::path::Path::new(dir).join(n)).collect(),
    };
    let w = ui(|u| u.worker);
    let (mut added, mut empty, mut failed) = (0, 0, None);
    let total = files.len();
    for f in files {
        let bytes = match std::fs::read(&f) {
            Ok(b) => b,
            Err(e) => {
                failed = Some(e);
                continue;
            }
        };
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{FEFF}');
        if text.trim().is_empty() {
            empty += 1;
            continue;
        }
        rich::load_markdown(w, text);
        // Links to files beside it (an export's attachments) come in as attachments.
        crate::win::ui::note::adopt_files(w, f.parent().unwrap_or(std::path::Path::new(".")));
        let rtf = rich::get_rtf(w);
        let plain = rich::to_markdown(w, false);
        store_mut(|s| s.create(&plain, &rtf));
        added += 1;
    }
    let skipped = if empty > 0 { format!(", {empty} empty skipped") } else { String::new() };
    match &failed {
        Some(e) => log::warn(&format!("import: added {added} of {total} files{skipped}; last read error: {e}")),
        None => log::info(&format!("import: added {added} of {total} files{skipped}")),
    }
    if added == 0 {
        return Some(Err(()));
    }
    save();
    refresh();
    Some(Ok(added))
}

/// The startup task declared in the MSIX manifest (`packaging/msix/AppxManifest.xml`).
const STARTUP_TASK: &str = "PeekABooNotesStartup";

/// Installed from an MSIX package. There the Run key is virtualised per package (Windows
/// never reads it), so start-with-Windows goes through the manifest's startup task.
pub(super) fn packaged() -> bool {
    use windows_sys::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
    use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Packaged: the empty buffer is just too small for the name.
    *P.get_or_init(|| unsafe { GetCurrentPackageFullName(&mut 0, null_mut()) != APPMODEL_ERROR_NO_PACKAGE })
}

fn startup_task() -> Option<windows::ApplicationModel::StartupTask> {
    windows::ApplicationModel::StartupTask::GetAsync(&windows::core::HSTRING::from(STARTUP_TASK)).ok()?.join().ok()
}

pub fn startup_enabled() -> bool {
    if packaged() {
        use windows::ApplicationModel::StartupTaskState as S;
        return startup_task().and_then(|t| t.State().ok()).is_some_and(|s| s == S::Enabled || s == S::EnabledByPolicy);
    }
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut()) == 0 }
}

pub fn set_startup(on: bool) {
    if packaged() {
        // Turned off in Task Manager (DisabledByUser), enabling is refused and the toggle
        // reads back off: only the user can undo that, there.
        if let Some(t) = startup_task() {
            let _ = if on { t.RequestEnableAsync().and_then(|op| op.join()).map(drop) } else { t.Disable() };
        }
        settings::refresh();
        return;
    }
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        if on {
            let cmd = wide(&format!("\"{}\"", exe_path().display()));
            RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), REG_SZ, cmd.as_ptr() as _, (cmd.len() * 2) as u32);
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
        }
    }
    settings::refresh();
}
