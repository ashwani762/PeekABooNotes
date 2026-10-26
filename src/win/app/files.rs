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
    let total = ids.len();
    let (written, failed) = write_markdown(&dir, ids);
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

/// Write notes `ids` into `dir` as Markdown files, their attachments beside them: how many
/// were written, and the last error if any weren't.
pub(super) fn write_markdown(dir: &std::path::Path, ids: &[u64]) -> (usize, Option<std::io::Error>) {
    use crate::common::frontmatter::{self, Meta};
    let notes: Vec<(String, String, Meta)> = store(|s| ids.iter().filter_map(|id| s.note(*id)).map(|n| (n.rtf.clone(), n.text.clone(), Meta::of(n))).collect());
    let mut used = HashSet::new();
    let (mut written, mut failed) = (0, None);
    for (rtf, text, meta) in notes {
        let md = if rtf.is_empty() { text.clone() } else { rtf_to_markdown(&rtf) };
        let base = file_safe(&title_of(&text));
        let mut name = format!("{base}.md");
        let mut k = 2;
        // Never overwrite: not a file already in the folder, nor one written just now.
        while dir.join(&name).exists() || !used.insert(name.to_lowercase()) {
            name = format!("{base} ({k}).md");
            k += 1;
        }
        // The state the Markdown can't hold goes in front matter, for import to put back.
        let md = frontmatter::join(&meta, &crate::common::store::hard_breaks(&export_attachments(&md, dir)));
        match std::fs::write(dir.join(name), md + "\n") {
            Ok(()) => written += 1,
            Err(e) => failed = Some(e),
        }
    }
    (written, failed)
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
        if !src.is_file() {
            continue;
        }
        let target_dir = dir.join(attach::EXPORT_DIR);
        if std::fs::create_dir_all(&target_dir).is_err() {
            continue;
        }
        // The same file there already (an earlier export of these notes): not copied again.
        let same = attach::same_content(&src, &target_dir.join(&name));
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

/// The largest file import reads: a full note of four-byte characters, with room for
/// its front matter. Anything larger can't be a note.
const MAX_IMPORT_BYTES: u64 = crate::common::store::MAX_NOTE_CHARS as u64 * 4 + 64 * 1024;

/// What an import did: the notes it brought in, the files it left out because a note
/// here says the same already (notes exported from here, brought back), and those it
/// couldn't read or that hold more than a note can.
pub struct Imported {
    pub added: usize,
    pub here: usize,
    pub unread: usize,
}

/// Bring in the Markdown and text files the user picks, each as a note, with the Undo
/// toast, which also says what was here already and what couldn't be read. `Err` when the
/// files were all empty.
pub fn import(owner: HWND) -> Option<Result<Imported, ()>> {
    let mut buf = vec![0u16; 65536];
    let filter: Vec<u16> = format!("{} (*.md, *.txt)\0*.md;*.markdown;*.txt\0{}\0*.*\0\0", t!("dialog_filter_text"), t!("dialog_filter_all")).encode_utf16().collect();
    let title = wide(t!("dialog_import_title"));
    // Where the last import came from, as Export starts where it last went.
    let last = store(|s| s.d.settings.import_dir.clone());
    let start = wide(&last);
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    if !last.is_empty() && std::path::Path::new(&last).is_dir() {
        ofn.lpstrInitialDir = start.as_ptr();
    }
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
    if let Some(dir) = files[0].parent().map(|d| d.to_string_lossy().into_owned()).filter(|d| *d != last) {
        store_mut(|s| s.d.settings.import_dir = dir);
        save();
    }
    let w = ui(|u| u.worker);
    let (mut added, mut here, mut empty, mut failed, mut unread) = (Vec::new(), 0, 0, None, 0);
    // One operation: a file the notes share is copied in once, and one the attachments
    // folder holds already (by name and bytes) is linked there.
    let mut copies = crate::common::attach::Copies::reusing();
    let total = files.len();
    // The log says what went wrong and with what kind of file, never which file: its name
    // and folder are the user's.
    let ext = |f: &std::path::Path| f.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_else(|| "no extension".into());
    for f in files {
        // Checked before it's read: a huge file is never taken into memory whole.
        if std::fs::metadata(&f).is_ok_and(|m| m.len() > MAX_IMPORT_BYTES) {
            failed = Some(format!("a file ({}) longer than a note holds", ext(&f)));
            unread += 1;
            continue;
        }
        let bytes = match std::fs::read(&f) {
            Ok(b) => b,
            Err(e) => {
                failed = Some(format!("{:?} reading a file ({})", e.kind(), ext(&f)));
                unread += 1;
                continue;
            }
        };
        let text = crate::common::util::decode_text(&bytes);
        // An export's front matter gives the note back its colour, state and dates.
        let (meta, text) = crate::common::frontmatter::split(&text);
        if text.trim().is_empty() {
            empty += 1;
            continue;
        }
        // A note's own length counts, not its Markdown's: the marks and escapes an export
        // writes make a full note's file longer than the note.
        rich::load_markdown(w, text);
        if rich::too_long(w) {
            failed = Some(format!("a file ({}) longer than a note holds", ext(&f)));
            unread += 1;
            continue;
        }
        // Links to files beside it (an export's attachments) come in as attachments.
        // Only files beside the imported one (an export's attachments folder): a Markdown
        // file from elsewhere can't pull in whatever it names.
        let folder = f.parent().unwrap_or(std::path::Path::new("."));
        crate::win::ui::note::adopt_files_with(w, folder, &crate::common::attach::Allowed { roots: vec![folder.to_path_buf()], except: Vec::new() }, &mut copies);
        let plain = rich::to_markdown(w, false);
        // The same as a note here (its pictures the same files too): not made twice.
        if store(|s| s.says(&plain)) {
            here += 1;
            continue;
        }
        let rtf = rich::get_rtf(w);
        added.push(store_mut(|s| {
            let id = s.create(&plain, &rtf);
            if let (Some(m), Some(n)) = (&meta, s.note_mut(id)) {
                m.apply(n);
            }
            id
        }));
    }
    let n = added.len();
    let mut skipped = if here > 0 { format!(", {here} already there") } else { String::new() };
    if empty > 0 {
        skipped.push_str(&format!(", {empty} empty skipped"));
    }
    match &failed {
        Some(e) => log::warn(&format!("import: added {n} of {total} files{skipped}; last error: {e}")),
        None => log::info(&format!("import: added {n} of {total} files{skipped}")),
    }
    if n == 0 && here == 0 && unread == 0 {
        return Some(Err(()));
    }
    if n > 0 {
        save();
        refresh();
    }
    // A word in the toast either way: what came in (with Undo), what was here already, and
    // what couldn't be read (too long for a note, or not to be read at all).
    imported(added, here, unread);
    Some(Ok(Imported { added: n, here, unread }))
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
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let found = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as _, &mut size) == 0 };
    // Another copy's entry (an installed one, seen from a portable one) isn't ours.
    let entry = crate::win::util::from_wide(&buf).to_lowercase();
    found && entry.contains(&exe_path().to_string_lossy().to_lowercase())
}

/// At start: an autostart entry written by an older version (without `--quiet`) gets
/// the quiet form, so sign-in brings up only the deck.
pub fn upgrade_startup() {
    if packaged() || !startup_enabled() {
        return;
    }
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let read = unsafe { RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as _, &mut size) == 0 };
    if read && !crate::win::util::from_wide(&buf).contains(QUIET_ARG) {
        set_startup(true);
    }
}

pub fn set_startup(on: bool) {
    if packaged() {
        // Turned off in Task Manager (DisabledByUser), enabling is refused and the toggle
        // reads back off: only the user can undo that, there.
        if let Some(t) = startup_task() {
            let _ = if on { t.RequestEnableAsync().and_then(|op| op.join()).map(drop) } else { t.Disable() };
        }
        settings::startup_changed();
        return;
    }
    let (k, v) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        if on {
            // Quietly: at sign-in only the deck appears, and nothing takes the foreground.
            let cmd = wide(&format!("\"{}\" {}", exe_path().display(), QUIET_ARG));
            RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), REG_SZ, cmd.as_ptr() as _, (cmd.len() * 2) as u32);
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr());
        }
    }
    settings::startup_changed();
}
