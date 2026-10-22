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
    let mut written = 0;
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
        written += std::fs::write(dir.join(name), md + "\n").is_ok() as usize;
    }
    if written == 0 {
        return Some(Err(()));
    }
    open_path(&dir);
    Some(Ok(written))
}

/// The system folder picker, opening in `start` when that folder still exists.
fn pick_folder(owner: HWND, title: &str, start: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
    use windows::Win32::UI::Shell::{
        FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    let pick = || unsafe {
        let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        d.SetOptions(d.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        d.SetTitle(&windows::core::HSTRING::from(title)).ok()?;
        d.SetOkButtonLabel(&windows::core::HSTRING::from(t!("dialog_export_ok"))).ok()?;
        if !start.is_empty()
            && std::path::Path::new(start).is_dir()
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&windows::core::HSTRING::from(start), None)
        {
            let _ = d.SetFolder(&item);
        }
        d.Show(Some(windows::Win32::Foundation::HWND(owner))).ok()?;
        let path = d.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as _));
        s.map(Into::into)
    };
    // COM only for the dialog's lifetime: uninitialising unloads the shell extensions it
    // pulled in now, rather than leaving them to tear down at exit.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let path = pick();
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
        return None;
    }
    let parts: Vec<String> = buf.split(|&c| c == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy).collect();
    let files: Vec<std::path::PathBuf> = match parts.as_slice() {
        [] => return None,
        [one] => vec![one.into()],
        [dir, names @ ..] => names.iter().map(|n| std::path::Path::new(dir).join(n)).collect(),
    };
    let w = ui(|u| u.worker);
    let mut added = 0;
    for f in files {
        let Ok(bytes) = std::fs::read(&f) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{FEFF}');
        if text.trim().is_empty() {
            continue;
        }
        rich::load_markdown(w, text);
        let rtf = rich::get_rtf(w);
        let plain = rich::to_markdown(w, false);
        store_mut(|s| s.create(&plain, &rtf));
        added += 1;
    }
    if added == 0 {
        return Some(Err(()));
    }
    save();
    refresh();
    Some(Ok(added))
}

/// The startup task declared in the MSIX manifest (`packaging/msix/AppxManifest.xml`).
const STARTUP_TASK: &str = "PeekabooNotesStartup";

/// Installed from an MSIX package. There the Run key is virtualised per package (Windows
/// never reads it), so start-with-Windows goes through the manifest's startup task.
fn packaged() -> bool {
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
