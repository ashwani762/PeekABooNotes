// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Share notes: their Markdown and attachments in one zip, handed to the Windows share
//! sheet (mail, Teams, nearby sharing and the rest).

use super::*;
use std::path::{Path, PathBuf};

use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{HSTRING, Interface};

thread_local! {
    /// The share sheet's request handler, kept until the next share replaces it (the
    /// sheet asks for the data after it opens).
    static HANDLER: RefCell<Option<(DataTransferManager, i64)>> = const { RefCell::new(None) };
}

/// Share notes `ids` from window `owner`.
pub fn share(owner: HWND, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    let name = match ids {
        [id] => store(|s| s.note(*id).map(|n| file_safe(&title_of(&n.text)))).unwrap_or_default(),
        _ => tn!("notes_count", ids.len()),
    };
    let name = if name.trim().is_empty() { t!("app_name").to_string() } else { name };
    let zip = match pack(&name, ids) {
        Ok(z) => z,
        Err(e) => {
            log::warn(&format!("share: couldn't make the zip: {e}"));
            return;
        }
    };
    if let Err(e) = show_sheet(owner, &zip, &name) {
        // No share sheet: show the zip in its folder, to send another way.
        log::warn(&format!("share: no share sheet: {:#010x} {}", e.code().0, e.message()));
        let args = wide(&format!("/select,\"{}\"", zip.display()));
        unsafe { ShellExecuteW(null_mut(), wide("open").as_ptr(), wide("explorer.exe").as_ptr(), args.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
    }
}

/// The notes written out and zipped, in a folder of their own under the temp folder.
fn pack(name: &str, ids: &[u64]) -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir().join("PeekABoo Share");
    // The last share's zip: whatever it went to has its copy by now.
    let _ = std::fs::remove_dir_all(&base);
    let stage = base.join("notes");
    std::fs::create_dir_all(&stage)?;
    let (written, failed) = write_markdown(&stage, ids);
    if written == 0 {
        return Err(failed.unwrap_or_else(|| std::io::Error::other("no notes written")));
    }
    let zip = base.join(format!("{name}.zip"));
    crate::common::zip::zip_dir(&stage, &zip)?;
    let _ = std::fs::remove_dir_all(&stage);
    log::info(&format!("share: zipped {written} notes"));
    Ok(zip)
}

fn show_sheet(owner: HWND, zip: &Path, title: &str) -> windows::core::Result<()> {
    let hwnd = windows::Win32::Foundation::HWND(owner);
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let dtm: DataTransferManager = unsafe { interop.GetForWindow(hwnd)? };
    let path = HSTRING::from(zip.to_string_lossy().as_ref());
    let title = HSTRING::from(title);
    HANDLER.with(|h| {
        if let Some((m, token)) = h.borrow_mut().take() {
            let _ = m.RemoveDataRequested(token);
        }
    });
    // Asked for when the sheet opens (the file is local: opening it is quick).
    let token = dtm.DataRequested(&TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
        let data = args.ok()?.Request()?.Data()?;
        data.Properties()?.SetTitle(&title)?;
        let item: IStorageItem = StorageFile::GetFileFromPathAsync(&path)?.join()?.cast()?;
        data.SetStorageItemsReadOnly(&windows_collections::IIterable::<IStorageItem>::from(vec![Some(item)]))
    }))?;
    HANDLER.with(|h| *h.borrow_mut() = Some((dtm, token)));
    unsafe { interop.ShowShareUIForWindow(hwnd) }
}
