// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Files in notes: dropped (or pasted) files are copied into the attachments folder and
//! appear in the note, pictures inline and anything else as a paperclip chip. Both are
//! links to `attachment:<name>`: click to open, hover to see the name, drag one out to
//! copy the file somewhere else.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, GENERIC_READ, HGLOBAL};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{CreateStreamOnHGlobal, GetHGlobalFromStream, IStorage, PROPVARIANT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, IOleInPlaceFrame, IOleInPlaceUIWindow, IOleObject, OLEINPLACEFRAMEINFO, ReleaseStgMedium};
use windows::Win32::System::SystemServices::{MODIFIERKEYS_FLAGS, RECO_FLAGS};
use windows::Win32::UI::Controls::RichEdit::{CHARRANGE, IRichEditOleCallback, IRichEditOleCallback_Impl, RICH_EDIT_GET_CONTEXT_MENU_SEL_TYPE};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::{BOOL, GUID, Interface, OutRef, Ref, implement};

use super::*;
use crate::common::attach;

/// Posted to the note after a drop, which is handled outside RichEdit's own drop code.
pub(super) const WM_ATTACH: u32 = WM_APP + 42;
const CF_HDROP: u16 = 15;

type Pending = (HWND, i32, Vec<PathBuf>, Option<String>);

thread_local! {
    /// Files waiting for `WM_ATTACH`: (note editor, where to insert, paths, the
    /// attachment's own address when it was dragged here from this same note).
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
    /// An attachment being dragged out: (its editor, its file, its address).
    static DRAGGING: RefCell<Option<(HWND, PathBuf, String)>> = const { RefCell::new(None) };
    static LAST_DROP: RefCell<Option<(Vec<PathBuf>, std::time::Instant)>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------- pictures

/// Shown at most this wide or tall (DIPs); the picture itself has twice the pixels, for
/// sharp display at high scaling, unless the original is smaller.
const MAX_SHOW: f32 = 240.0;

/// The picture attachment `name`, fitted into `max_w` x `max_h` px (never enlarged) and
/// turned upright: (width, height, premultiplied BGRA pixels). For previews.
pub fn thumbnail(name: &str, max_w: u32, max_h: u32) -> Option<(u32, u32, Vec<u32>)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide_path = wide(&attach::dir().join(name).to_string_lossy());
        let decoder = wic.CreateDecoderFromFilename(windows::core::PCWSTR(wide_path.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 || max_w == 0 || max_h == 0 {
            return None;
        }
        let mut source: IWICBitmapSource = frame.cast().ok()?;
        if let Some(t) = orientation(&frame) {
            let r = wic.CreateBitmapFlipRotator().ok()?;
            r.Initialize(&source, t).ok()?;
            if t == WICBitmapTransformRotate90 || t == WICBitmapTransformRotate270 {
                std::mem::swap(&mut w, &mut h);
            }
            source = r.cast().ok()?;
        }
        let fit = (max_w as f32 / w as f32).min(max_h as f32 / h as f32).min(1.0);
        let (tw, th) = (((w as f32 * fit).round() as u32).max(1), ((h as f32 * fit).round() as u32).max(1));
        if (tw, th) != (w, h) {
            let s = wic.CreateBitmapScaler().ok()?;
            s.Initialize(&source, tw, th, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            source = s.cast().ok()?;
        }
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&source, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut bytes = vec![0u8; (tw * th * 4) as usize];
        conv.CopyPixels(std::ptr::null(), tw * 4, &mut bytes).ok()?;
        let px = bytes.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect();
        Some((tw, th, px))
    }
}

/// The picture at `path` as an RTF `\pict` group: scaled down, turned upright, and
/// re-encoded (PNG where it may be transparent, JPEG for photos), so a note stays light.
pub fn picture_rtf(path: &Path) -> Option<String> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide_path = wide(&path.to_string_lossy());
        let decoder = wic.CreateDecoderFromFilename(windows::core::PCWSTR(wide_path.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        // Photos from phones are stored sideways with an orientation tag.
        let turn = orientation(&frame);
        let mut source: IWICBitmapSource = frame.cast().ok()?;
        if let Some(t) = turn {
            let r = wic.CreateBitmapFlipRotator().ok()?;
            r.Initialize(&source, t).ok()?;
            if t == WICBitmapTransformRotate90 || t == WICBitmapTransformRotate270 {
                std::mem::swap(&mut w, &mut h);
            }
            source = r.cast().ok()?;
        }
        let fit = (MAX_SHOW / w as f32).min(MAX_SHOW / h as f32).min(1.0);
        let (show_w, show_h) = ((w as f32 * fit).max(1.0), (h as f32 * fit).max(1.0));
        let px = |v: f32, orig: u32| ((v * 2.0).round() as u32).clamp(1, orig);
        let (pw, ph) = (px(show_w, w), px(show_h, h));
        if (pw, ph) != (w, h) {
            let s = wic.CreateBitmapScaler().ok()?;
            s.Initialize(&source, pw, ph, WICBitmapInterpolationModeHighQualityCubic).ok()?;
            source = s.cast().ok()?;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let png = matches!(ext.as_str(), "png" | "gif" | "bmp" | "webp" | "tif" | "tiff");
        let mut fmt = if png { GUID_WICPixelFormat32bppBGRA } else { GUID_WICPixelFormat24bppBGR };
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&source, &fmt, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let stream = CreateStreamOnHGlobal(HGLOBAL::default(), true).ok()?;
        let container = if png { GUID_ContainerFormatPng } else { GUID_ContainerFormatJpeg };
        let enc = wic.CreateEncoder(&container, std::ptr::null()).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut fe = None;
        enc.CreateNewFrame(&mut fe, std::ptr::null_mut()).ok()?;
        let fe = fe?;
        fe.Initialize(None).ok()?;
        fe.SetSize(pw, ph).ok()?;
        fe.SetPixelFormat(&mut fmt).ok()?;
        fe.WriteSource(&conv, std::ptr::null()).ok()?;
        fe.Commit().ok()?;
        enc.Commit().ok()?;
        let hg = GetHGlobalFromStream(&stream).ok()?;
        let p = GlobalLock(hg) as *const u8;
        if p.is_null() {
            return None;
        }
        // The stream's memory can be larger than what was written: ask the stream.
        let size = {
            let mut st = Default::default();
            stream.Stat(&mut st, windows::Win32::System::Com::STATFLAG_NONAME).ok().map(|_| st.cbSize as usize).unwrap_or(GlobalSize(hg))
        };
        let bytes = std::slice::from_raw_parts(p, size);
        let mut hex = String::with_capacity(size * 2 + size / 32);
        for (i, b) in bytes.iter().enumerate() {
            if i % 64 == 0 {
                hex.push('\n');
            }
            hex.push_str(&format!("{b:02x}"));
        }
        let _ = GlobalUnlock(hg);
        let blip = if png { "pngblip" } else { "jpegblip" };
        // Twips: 15 per DIP.
        Some(format!(
            "{{\\pict\\{blip}\\picw{pw}\\pich{ph}\\picwgoal{}\\pichgoal{}{hex}}}",
            (show_w * 15.0).round() as i32,
            (show_h * 15.0).round() as i32
        ))
    }
}

/// The EXIF orientation as the turn that puts the picture upright.
fn orientation(frame: &IWICBitmapFrameDecode) -> Option<WICBitmapTransformOptions> {
    unsafe {
        let q = frame.GetMetadataQueryReader().ok()?;
        let mut v = PROPVARIANT::default();
        q.GetMetadataByName(windows::core::w!("/app1/ifd/{ushort=274}"), &mut v).ok()?;
        let inner = &v.Anonymous.Anonymous;
        // VT_UI2
        if inner.vt.0 != 18 {
            return None;
        }
        match inner.Anonymous.uiVal {
            3 => Some(WICBitmapTransformRotate180),
            6 => Some(WICBitmapTransformRotate90),
            8 => Some(WICBitmapTransformRotate270),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- putting files in a note

/// Text for RTF: escapes, and everything past ASCII as \uN.
fn rtf_text(s: &str) -> String {
    let mut out = String::new();
    for u in s.encode_utf16() {
        match u {
            0x5C => out.push_str("\\\\"),
            0x7B => out.push_str("\\{"),
            0x7D => out.push_str("\\}"),
            0x20..0x7F => out.push(u as u8 as char),
            _ => out.push_str(&format!("\\u{}?", u as i16)),
        }
    }
    out
}

/// One attachment, as RTF: its picture, or a paperclip and its name, followed by its
/// (invisible) link.
fn item_rtf(name: &str) -> String {
    let url = attach::url(name);
    let link = format!("{{\\field{{\\*\\fldinst{{HYPERLINK \"{url}\"}}}}{{\\fldrslt{{{}}}}}}}", rtf_text(&rich::ITEM_LINK_NAME.to_string()));
    let shown = attach::is_image(name).then(|| picture_rtf(&attach::dir().join(name))).flatten();
    let shown = shown.unwrap_or_else(|| rtf_text(&format!("{} {name}", rich::CLIP)));
    format!("{shown}{link}")
}

/// Copy `paths` in and add them to the note in editor `e` at `cp`, each on a line of
/// its own. Folders are skipped. Returns how many went in.
pub(super) fn add(e: HWND, cp: i32, paths: &[PathBuf]) -> usize {
    let mut names = Vec::new();
    for p in paths.iter().filter(|p| p.is_file()) {
        match attach::copy_in(p) {
            Ok(name) => names.push(name),
            Err(err) => crate::common::log::warn(&format!("attach: couldn't copy {}: {err}", p.display())),
        }
    }
    put(e, cp, &names)
}

/// Add attachments already in the folder to the note at `cp`, each on its own line.
fn put(e: HWND, cp: i32, names: &[String]) -> usize {
    let items: Vec<String> = names.iter().map(|n| item_rtf(n)).collect();
    if items.is_empty() {
        return 0;
    }
    let t = rich::text(e);
    let cp = cp.clamp(0, t.len() as i32);
    // On lines of their own: break the line first unless the caret starts one.
    let at_line_start = cp == 0 || t.get(cp as usize - 1) == Some(&13);
    let at_line_end = t.get(cp as usize).is_none_or(|&c| c == 13);
    let mut rtf = String::from("{\\rtf1\\ansi ");
    if !at_line_start {
        rtf.push_str("\\par ");
    }
    rtf.push_str(&items.join("\\par "));
    if !at_line_end {
        rtf.push_str("\\par ");
    }
    rtf.push('}');
    rich::set_sel(e, cp, cp);
    let before = rich::text(e).len() as i32;
    rich::insert_rtf(e, &rtf);
    let end = cp + rich::text(e).len() as i32 - before;
    // Body text, whatever the line it went in after (the title's look would carry over).
    rich::set_sel(e, cp, end);
    rich::set_body_format(e);
    rich::style_clips(e, cp, end, super::icon_face());
    rich::fit_pictures(e);
    rich::set_sel(e, end, end);
    items.len()
}

/// Files dropped on `e` (from RichEdit's callback or WM_DROPFILES) go in after the
/// current message: never from inside RichEdit's own drop handling.
pub(super) fn queue(e: HWND, cp: i32, paths: Vec<PathBuf>) {
    // RichEdit can ask about one drop more than once (once per format it tries), with
    // the first already handled in between.
    let again = LAST_DROP.with(|l| l.borrow().as_ref().is_some_and(|(pp, at)| *pp == paths && at.elapsed().as_millis() < 1500));
    if paths.is_empty() || again {
        return;
    }
    LAST_DROP.with(|l| *l.borrow_mut() = Some((paths.clone(), std::time::Instant::now())));
    // One of this note's own attachments, dropped back on it: asked about once the drag
    // is over (see `drag_out`).
    let own = DRAGGING.with(|d| d.borrow().as_ref().filter(|(de, dp, _)| *de == e && paths.len() == 1 && paths[0] == *dp).map(|d| d.2.clone()));
    let later = own.is_some();
    PENDING.with(|p| p.borrow_mut().push((e, cp, paths, own)));
    if !later && let Some(note) = note_of(e) {
        unsafe { PostMessageW(note, WM_ATTACH, 0, 0) };
    }
}

fn note_of(e: HWND) -> Option<HWND> {
    let h = unsafe { GetParent(e) };
    (!h.is_null()).then_some(h)
}

/// `WM_ATTACH`: put queued files into this note.
pub(super) fn flush_pending(h: HWND) {
    let Some(st) = state(h) else { return };
    let e = st.edit;
    let mine: Vec<_> = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let (mine, rest): (Vec<_>, Vec<_>) = p.drain(..).partition(|(pe, _, _, _)| *pe == e);
        *p = rest;
        mine
    });
    let mut n = 0;
    for (_, cp, paths, own) in mine {
        n += match own {
            Some(url) => move_or_copy(h, e, cp, &paths, &url),
            None => add(e, cp, &paths),
        };
    }
    if n > 0 {
        unsafe { SetFocus(e) };
        mark_dirty(h);
    }
}

/// Links in editor `e` (just loaded from Markdown in folder `base`) that point at local
/// files `allowed` admits: the files are copied in as attachments (links to others stay
/// as they are). A link alone on its line becomes the attachment itself (a picture
/// shows); one inside a sentence keeps its words and points at the copy.
pub fn adopt_files(e: HWND, base: &Path, allowed: &attach::Allowed) {
    let t = rich::text(e);
    // From the end, so what's changed never moves what's still to do.
    for (at, name_at, link) in rich::instructions(&t, 0, t.len()).into_iter().rev() {
        let Some(file) = attach::local_file(&link, base).filter(|f| allowed.admits(f)) else { continue };
        let Ok(name) = attach::copy_in(&file) else { continue };
        let t = rich::text(e);
        let Some((a, b, _)) = rich::link_at(e, name_at as i32) else { continue };
        let line_start = t[..at].iter().rposition(|&c| c == 13).map_or(0, |i| i + 1);
        let line_end = t[at..].iter().position(|&c| c == 13).map_or(t.len(), |i| at + i);
        let alone = rich::visible(&t, line_start, at).trim().is_empty() && rich::visible(&t, b as usize, line_end).trim().is_empty();
        if alone {
            rich::set_sel(e, line_start as i32, line_end as i32);
            rich::replace_sel(e, "");
            put(e, line_start as i32, &[name]);
        } else {
            let shown = b - name_at as i32;
            rich::remove_link(e, a, b);
            rich::set_link(e, a, a + shown, &attach::url(&name));
        }
    }
}

/// The folders an assistant's links may bring files in from: the user's own (pictures,
/// documents, downloads and the like) and the temp folder, where assistants make
/// charts; never the notes' folder itself, other than its attachments.
pub fn assistant_folders() -> attach::Allowed {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    let mut roots = Vec::new();
    for id in [&FOLDERID_Pictures, &FOLDERID_Documents, &FOLDERID_Downloads, &FOLDERID_Desktop, &FOLDERID_Videos, &FOLDERID_Music] {
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        unsafe {
            if SHGetKnownFolderPath(id, KF_FLAG_DEFAULT as _, std::ptr::null_mut(), &mut p) == 0 && !p.is_null() {
                let n = (0..).take_while(|&i| *p.add(i) != 0).count();
                roots.push(PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(p, n))));
            }
            windows_sys::Win32::System::Com::CoTaskMemFree(p as _);
        }
    }
    roots.push(std::env::temp_dir());
    let data = crate::common::util::data_dir();
    roots.push(data.join(attach::EXPORT_DIR));
    attach::Allowed { roots, except: vec![data] }
}

/// One of this note's attachments was dragged to another place in it: move it there,
/// or put a copy there?
fn move_or_copy(h: HWND, e: HWND, cp: i32, paths: &[PathBuf], url: &str) -> usize {
    use crate::win::ui::menu;
    // Where the attachment's line is now (address hidden in it, then the line around it).
    let t = rich::text(e);
    let Some(at) = rich::instructions(&t, 0, t.len()).into_iter().find(|l| l.2 == url).map(|l| l.0) else { return add(e, cp, paths) };
    let (start, end) = {
        let s = t[..at].iter().rposition(|&c| c == 13).map_or(0, |i| i + 1);
        let e2 = t[at..].iter().position(|&c| c == 13).map_or(t.len(), |i| at + i);
        (s as i32, e2 as i32)
    };
    if (start..=end).contains(&cp) {
        return 0; // dropped where it already is
    }
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    let items = vec![menu::action(1, t!("menu_move_here"), "", "\u{E7C2}"), menu::action(2, t!("menu_copy_here"), "", "\u{E8C8}")];
    match app::popup(items, p.x, p.y, h) {
        1 => {
            let Some(name) = attach::name_of(url) else { return 0 };
            // Take the line out (with its line break), then put it where it was dropped.
            let (a, b) = if (end as usize) < t.len() { (start, end + 1) } else { (start.saturating_sub(1).max(0), end) };
            rich::set_sel(e, a, b);
            rich::replace_sel(e, "");
            let cp = if cp > a { cp - (b - a) } else { cp };
            put(e, cp, &[name])
        }
        2 => add(e, cp, paths),
        _ => 0,
    }
}

/// The files in an HDROP.
fn hdrop_files(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len])))
            })
            .collect()
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn data_files(d: &IDataObject) -> Vec<PathBuf> {
    unsafe {
        let Ok(mut med) = d.GetData(&hdrop_format()) else { return Vec::new() };
        let files = hdrop_files(HDROP(med.u.hGlobal.0));
        ReleaseStgMedium(&mut med);
        files
    }
}

/// Files copied in Explorer and pasted into the note.
pub(super) fn clipboard_files() -> Vec<PathBuf> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP as u32) == 0 || OpenClipboard(null_mut()) == 0 {
            return Vec::new();
        }
        let h = GetClipboardData(CF_HDROP as u32);
        let files = if h.is_null() { Vec::new() } else { hdrop_files(HDROP(h)) };
        CloseClipboard();
        files
    }
}

/// WM_DROPFILES (files dropped on the note's frame rather than its text): at the end.
pub(super) fn dropped_on_frame(h: HWND, hdrop: usize) {
    let files = hdrop_files(HDROP(hdrop as *mut _));
    unsafe { windows::Win32::UI::Shell::DragFinish(HDROP(hdrop as *mut _)) };
    if let Some(st) = state(h) {
        let end = rich::text(st.edit).len() as i32;
        queue(st.edit, end, files);
    }
}

// ---------------------------------------------------------------- RichEdit's OLE callback

/// Lets RichEdit keep its own text drag and drop, and takes file drops over.
#[implement(IRichEditOleCallback)]
struct Callback {
    edit: HWND,
}

impl IRichEditOleCallback_Impl for Callback_Impl {
    fn GetNewStorage(&self) -> windows::core::Result<IStorage> {
        Err(E_NOTIMPL.into())
    }
    fn GetInPlaceContext(&self, _: OutRef<IOleInPlaceFrame>, _: OutRef<IOleInPlaceUIWindow>, _: *mut OLEINPLACEFRAMEINFO) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ShowContainerUI(&self, _: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryInsertObject(&self, _: *mut GUID, _: Ref<IStorage>, _: i32) -> windows::core::Result<()> {
        Ok(())
    }
    fn DeleteObject(&self, _: Ref<IOleObject>) -> windows::core::Result<()> {
        Ok(())
    }
    fn QueryAcceptData(&self, data: Ref<IDataObject>, _: *mut u16, _: RECO_FLAGS, really: BOOL, _: HGLOBAL) -> windows::core::Result<()> {
        let Some(d) = data.as_ref() else { return Ok(()) };
        // S_FALSE says "not here" too.
        if unsafe { d.QueryGetData(&hdrop_format()) } != windows::Win32::Foundation::S_OK {
            return Ok(()); // text and the like: RichEdit's business
        }
        if really.as_bool() {
            // Where the pointer let go.
            let mut p = POINT { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut p);
                ScreenToClient(self.edit, &mut p);
            }
            let cp = rich::char_from_pos(self.edit, p.x, p.y);
            queue(self.edit, cp, data_files(d));
            // Refused, so RichEdit doesn't insert the files its own way too.
            return Err(E_FAIL.into());
        }
        Ok(())
    }
    fn ContextSensitiveHelp(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn GetClipboardData(&self, _: *mut CHARRANGE, _: u32, _: OutRef<IDataObject>) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetDragDropEffect(&self, drag: BOOL, _: MODIFIERKEYS_FLAGS, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        // Dropping files copies them.
        if !drag.as_bool() && !effect.is_null() {
            unsafe {
                if (*effect).0 & DROPEFFECT_COPY.0 != 0 {
                    *effect = DROPEFFECT_COPY;
                }
            }
        }
        Ok(())
    }
    fn GetContextMenu(&self, _: RICH_EDIT_GET_CONTEXT_MENU_SEL_TYPE, _: Ref<IOleObject>, _: *mut CHARRANGE, _: *mut windows::Win32::UI::WindowsAndMessaging::HMENU) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

/// Hook editor `e` up for file drops.
pub(super) fn hook(e: HWND) {
    const EM_SETOLECALLBACK: u32 = WM_USER + 70;
    let cb: IRichEditOleCallback = Callback { edit: e }.into();
    // RichEdit keeps its own reference.
    rich::send(e, EM_SETOLECALLBACK, 0, windows::core::Interface::as_raw(&cb) as isize);
}

// ---------------------------------------------------------------- dragging out

/// The attachment file behind link `url`, if it is one and still exists.
pub(super) fn file_of(url: &str) -> Option<PathBuf> {
    attach::path_of(url).filter(|p| p.is_file())
}

/// Drag the file at `path` out of the note (Explorer, the desktop, a mail): a copy.
pub(super) fn drag_out(e: HWND, path: &Path, url: &str) {
    use windows::Win32::UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop};
    unsafe {
        let w = wide(&path.to_string_lossy());
        // The same data Explorer offers for the file (the file list among it).
        let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(windows::core::PCWSTR(w.as_ptr()), None) else { return };
        let Ok(data) = item.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) else { return };
        DRAGGING.with(|d| *d.borrow_mut() = Some((e, path.to_path_buf(), url.to_string())));
        // The shell's own drag image and drop source.
        let _ = SHDoDragDrop(Some(windows::Win32::Foundation::HWND(e as _)), &data, None, DROPEFFECT_COPY | windows::Win32::System::Ole::DROPEFFECT_LINK);
        DRAGGING.with(|d| d.borrow_mut().take());
        // Dropped back on its own note: ask now, with the drag over.
        if PENDING.with(|p| p.borrow().iter().any(|(pe, _, _, own)| *pe == e && own.is_some()))
            && let Some(note) = note_of(e)
        {
            PostMessageW(note, WM_ATTACH, 0, 0);
        }
    }
}
