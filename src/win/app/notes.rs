// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Note lifecycle: open, create, step, colour, pin, archive, delete and undo.

use super::*;

pub(super) fn note_hwnd(id: u64) -> Option<HWND> {
    ui(|u| u.notes.get(&id).copied())
}

/// Redraw everything that shows note lists.
pub fn refresh() {
    deck::refresh();
    allnotes::refresh();
    settings::notes_changed();
}

/// Note `id`'s content was saved as it was typed: the deck and All Notes bring its card
/// and row up to date, and nothing else (the Data page's counts can't have changed).
pub fn note_edited(id: u64) {
    deck::note_edited(id);
    allnotes::note_edited(id);
}

/// Plain text + list markers for an RTF blob, via the hidden worker editor.
pub fn rtf_to_text(rtf: &str) -> String {
    let w = ui(|u| u.worker);
    rich::set_rtf(w, rtf);
    rich::to_markdown(w, false)
}

/// Notes saved before previews showed pictures and files have only their names in their
/// light text: bring it up to date from their rich text, once, without touching when
/// they were edited.
pub(super) fn refresh_light_text() {
    let stale: Vec<(u64, String)> = store_mut(|s| {
        let ids = s.d.stale_light_text();
        // Done from here on, also for notes whose links sit inside a sentence (which never
        // get the line form): they're rebuilt now and not at every start.
        s.d.light_text = crate::common::store::LIGHT_TEXT;
        ids.iter().filter_map(|id| s.note(*id)).map(|n| (n.id, n.rtf.clone())).collect()
    });
    if stale.is_empty() {
        return;
    }
    for (id, rtf) in stale {
        let text = rtf_to_text(&rtf);
        store_mut(|s| {
            if let Some(n) = s.note_mut(id) {
                n.text = text;
            }
        });
    }
    save();
}

/// Notes stored before pictures kept their own size (`store::PICTURE_SIZES`): their
/// pictures narrower than their pixels make them were narrowed to fit a note, so they get
/// that size back, once, without touching when the notes were edited.
pub(super) fn widen_stored_pictures() {
    let changed = store_mut(|s| {
        if s.d.picture_sizes >= crate::common::store::PICTURE_SIZES {
            return false;
        }
        s.d.picture_sizes = crate::common::store::PICTURE_SIZES;
        for n in s.d.notes.iter_mut().filter(|n| n.rtf.contains("{\\pict")) {
            if let Some(rtf) = rich::widen_stored_pictures(&n.rtf) {
                n.rtf = rtf;
            }
        }
        true
    });
    if changed {
        save();
    }
}

pub fn rtf_to_markdown(rtf: &str) -> String {
    let w = ui(|u| u.worker);
    rich::set_rtf(w, rtf);
    rich::to_markdown(w, true)
}
/// Markdown (as `rtf_to_markdown` writes it) as a note's RTF: its pictures and files
/// (`attachments/<name>` there) are the note's attachments again.
pub fn markdown_to_rtf(md: &str) -> String {
    let w = ui(|u| u.worker);
    rich::load_markdown(w, md);
    note::own_attachments(w);
    rich::get_rtf(w)
}


/// `r` moved so that at least a third of it lies in the work area `work`, and its top
/// (the header, where it's picked up) is never above it. A rect that already does is
/// left alone.
pub(super) fn clamp_to_work(r: RECT, work: RECT) -> RECT {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let x = r.left.clamp((work.left - w * 2 / 3).min(work.left), (work.right - w / 3).max(work.left));
    let y = r.top.clamp(work.top, (work.bottom - h / 3).max(work.top));
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub(super) fn placement(id: u64) -> RECT {
    let saved = store(|s| s.note(id).and_then(|n| n.rect));
    if let Some([l, t, r, b]) = saved
        && r - l > 50
        && b - t > 50
    {
        // Back where it was; a monitor gone since (or a different arrangement) leaves it
        // on the nearest one, with enough of it on screen to take hold of.
        let rc = RECT { left: l, top: t, right: r, bottom: b };
        unsafe {
            let mon = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 {
                return clamp_to_work(rc, mi.rcWork);
            }
        }
    }
    let (ex, cy, s, work, left) = deck::anchor(Some(id));
    let (w, h) = ((320.0 * s) as i32, (360.0 * s) as i32);
    let gap = (8.0 * s) as i32;
    let x = if left { ex + gap } else { ex - w - gap };
    let y = (cy - (60.0 * s) as i32).clamp(work.top + gap, (work.bottom - h - gap).max(work.top));
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn open_note(id: u64) {
    open_note_from(id, deck::card_rect(id));
}

/// Open a note, growing it out of `from` (a screen rect + corner radius) when given.
pub fn open_note_from(id: u64, from: Option<(RECT, f32)>) {
    if let Some(h) = note_hwnd(id) {
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
        return;
    }
    let Some((color, text)) = store(|s| s.note(id).map(|n| (n.color, n.text.clone()))) else { return };
    let r = placement(id);
    match from {
        Some(from) if !crate::win::render::anim::reduced() => {
            // Its picture before the deck tucks it away.
            let from_image = deck::card_image(id);
            let h = note::create(id, r, note::Show::Later);
            deck::hand_off(id);
            // Shows what the window will (it's already laid out, scrolled to the end).
            let mut content = note::flight_content(h, &text);
            content.from_image = from_image;
            if text.trim().is_empty() {
                content.title.clear();
            }
            // The window's corners, at the scale of the monitor it opens on (not the deck's).
            let s = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h) }.max(96) as f32 / 96.0;
            crate::win::ui::ghost::fly(crate::win::ui::ghost::Kind::Open, from, (r, 8.0 * s), color, content, Some(Box::new(move || note::reveal(h))));
        }
        _ => {
            note::create(id, r, note::Show::Now);
        }
    }
    deck::refresh(); // shows the "open" dot
}

pub fn toggle_note(id: u64) {
    if let Some(h) = note_hwnd(id)
        && unsafe { GetForegroundWindow() } == h {
            unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
            return;
        }
    open_note(id);
}

pub fn new_note() {
    let id = store_mut(|s| s.create("", ""));
    let from = deck::plus_rect().unwrap_or_else(|| deck::home_rect(id));
    refresh();
    open_note_from(id, Some(from));
}

/// Open the next/previous deck note in place of `id`. Returns true if the caller should close.
pub fn step_note(id: u64, dir: i32, r: RECT) -> bool {
    let ids = store(|s| s.active_ids());
    if ids.len() < 2 {
        return false;
    }
    let i = ids.iter().position(|x| *x == id).unwrap_or(0) as i32;
    let next = ids[(i + dir).rem_euclid(ids.len() as i32) as usize];
    if let Some(h) = note_hwnd(next) {
        unsafe { SetForegroundWindow(h) };
        return false;
    }
    note::create(next, r, note::Show::Now);
    true
}

/// At quit: the open notes that stay open, to bring back next time.
pub(super) fn remember_open() {
    let open: Vec<u64> = ui(|u| u.notes.keys().copied().collect());
    store_mut(|s| {
        let all = s.d.settings.keep_open;
        let mut ids: Vec<u64> = open.into_iter().filter(|id| s.note(*id).is_some_and(|n| !n.archived && (all || n.pinned))).collect();
        ids.sort_unstable();
        s.d.settings.reopen = ids;
    });
}

/// At start: the notes open at quit that stay open, where they were, behind whatever the
/// user is doing (at sign-in, that's never them).
pub(super) fn reopen_at_start() {
    let ids = store(|s| s.d.settings.reopen.iter().copied().filter(|id| s.note(*id).is_some_and(|n| !n.archived)).collect::<Vec<_>>());
    for id in ids {
        note::create(id, placement(id), note::Show::Quiet);
    }
    if !ui(|u| u.notes.is_empty()) {
        deck::refresh(); // their "open" dots
    }
}

pub(super) fn reopen_last() {
    let last = store(|s| s.d.settings.last_note.filter(|id| s.note(*id).is_some_and(|n| !n.archived)));
    // The last one gone or archived: the note edited most recently.
    let recent = || store(|s| s.d.notes.iter().filter(|n| !n.archived).max_by_key(|n| n.updated).map(|n| n.id));
    match last.or_else(recent) {
        Some(id) => open_note(id),
        None => new_note(),
    }
}

pub(super) fn set_color(id: u64, c: u8) {
    store_mut(|s| {
        if let Some(n) = s.note_mut(id) {
            n.color = c % theme::COLORS.len() as u8;
        }
    });
    save();
    if let Some(h) = note_hwnd(id) {
        note::retheme(h);
    }
    allnotes::recolor(id);
    refresh();
}

pub(super) fn toggle_pin(id: u64) {
    let pinned = store_mut(|s| {
        s.note_mut(id).map(|n| {
            n.pinned = !n.pinned;
            n.pinned
        })
    });
    save();
    if let (Some(p), Some(h)) = (pinned, note_hwnd(id)) {
        unsafe {
            SetWindowPos(h, if p { HWND_TOPMOST } else { HWND_NOTOPMOST }, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            windows_sys::Win32::Graphics::Gdi::InvalidateRect(h, std::ptr::null(), 0);
        }
    }
}

pub fn set_archived(ids: &[u64], archived: bool) {
    for &id in ids {
        if let Some(h) = note_hwnd(id)
            && archived {
                note::flush(h);
                note::dissolve(h);
                note::discard(h);
            }
    }
    store_mut(|s| {
        let top = s.d.notes.iter().filter(|n| !n.archived).map(|n| n.order).min().unwrap_or(0);
        for &id in ids {
            if let Some(n) = s.note_mut(id) {
                if n.archived && !archived {
                    n.order = top - 1;
                }
                n.archived = archived;
            }
        }
    });
    save();
    refresh();
}

/// Archive notes from a click or key, with the Undo toast, as deleting has.
pub fn archive(ids: &[u64]) {
    let before: Vec<(u64, i64)> = store(|s| ids.iter().filter_map(|id| s.note(*id).filter(|n| !n.archived).map(|n| (n.id, n.order))).collect());
    if before.is_empty() {
        return;
    }
    set_archived(ids, true);
    let count = before.len();
    // The toast undoes one thing: what was deleted before stays deleted.
    ui(|u| {
        u.pending.clear();
        u.unarchive = before;
    });
    toast::show(count, true);
}

pub fn delete(ids: Vec<u64>) {
    for &id in &ids {
        if let Some(h) = note_hwnd(id) {
            note::flush(h);
            note::dissolve(h);
            note::discard(h);
        }
    }
    let removed: Vec<(usize, Note)> = store_mut(|s| ids.iter().filter_map(|id| s.remove(*id)).collect());
    if removed.is_empty() {
        return;
    }
    let count = ui(|u| {
        u.unarchive.clear();
        u.pending.extend(removed);
        u.pending.len()
    });
    save();
    refresh();
    toast::show(count, false);
}

/// Delete every note, archived ones too, once the user has said yes. The usual Undo toast
/// follows, so even a confirmed slip can be taken back. The question drops from `at`
/// (under the button, when the keyboard pressed it), else at the pointer.
pub fn delete_all(owner: HWND, at: Option<POINT>) {
    let ids: Vec<u64> = store(|s| s.d.notes.iter().map(|n| n.id).collect());
    if !ids.is_empty() && confirm_delete_all(owner, ids.len(), at) {
        delete(ids);
    }
}

/// "Delete All 25 Notes" or Cancel, in a menu at the pointer (on the button that asked):
/// themed like the rest of the app, and nothing is chosen until one is clicked, so Enter
/// or Esc never deletes anything.
fn confirm_delete_all(owner: HWND, n: usize, at: Option<POINT>) -> bool {
    use crate::win::ui::menu::{Item, action, danger};
    const ID_DELETE: u32 = 1;
    // Cancel first, under the pointer: a click that lands a little low still deletes nothing.
    let items = vec![action(2, t!("button_cancel"), "", ""), Item::Separator, danger(ID_DELETE, &tn!("menu_delete_all_notes", n), "", IC_DELETE)];
    let p = at.unwrap_or_else(super::menus::cursor);
    popup(items, p.x, p.y, owner) == ID_DELETE
}

/// Whether the Undo toast has something to take back (it is showing).
pub fn can_undo() -> bool {
    ui(|u| !u.pending.is_empty() || !u.unarchive.is_empty())
}

/// The toast's Undo: the notes just deleted come back, or the ones just archived return to
/// the deck where they were.
pub fn undo_delete() {
    let unarchive = ui(|u| std::mem::take(&mut u.unarchive));
    if !unarchive.is_empty() {
        toast::hide();
        store_mut(|s| {
            for (id, order) in &unarchive {
                if let Some(n) = s.note_mut(*id) {
                    n.archived = false;
                    n.order = *order;
                }
            }
        });
        save();
        refresh();
        crate::win::ui::allnotes::select(&unarchive.iter().map(|u| u.0).collect::<Vec<_>>());
        return;
    }
    let mut pending = ui(|u| std::mem::take(&mut u.pending));
    toast::hide();
    if pending.is_empty() {
        return;
    }
    pending.sort_by_key(|(i, _)| *i);
    let ids: Vec<u64> = pending.iter().map(|(_, n)| n.id).collect();
    store_mut(|s| {
        for (i, n) in pending {
            s.insert_at(i, n);
        }
    });
    save();
    refresh();
    // What came back is what All Notes shows.
    crate::win::ui::allnotes::select(&ids);
}

pub fn finalize_delete() {
    let gone: Vec<Note> = ui(|u| {
        u.unarchive.clear();
        u.pending.drain(..).map(|(_, n)| n).collect()
    });
    toast::hide();
    if gone.is_empty() {
        return;
    }
    // Gone for good: their saved versions too, and then the files only they linked.
    crate::win::ui::note::versions::forget(&gone.iter().map(|n| n.id).collect::<Vec<_>>());
    delete_attachments(&gone);
}

/// Take the attachments notes `gone` linked out of the folder, unless another note or a
/// saved version (another note's, put back later) still links them.
fn delete_attachments(gone: &[Note]) {
    use crate::common::attach;
    let names: std::collections::BTreeSet<String> = gone.iter().flat_map(|n| attach::linked(&n.rtf).into_iter().chain(attach::linked(&n.text))).collect();
    if names.is_empty() {
        return;
    }
    // Only these notes' own files, judged against all the notes there are: not when the
    // notes are the backup's (or none, or not written this session), nor while notes set
    // aside (unread, or another writer's) may come back linking them.
    if !store(|s| s.whole()) || crate::common::store::others_kept(&data_dir()) {
        log::info("attachments of deleted notes kept: the notes in use may not be all there are");
        return;
    }
    let Some(versions) = crate::win::ui::note::versions::markdown() else {
        log::warn("attachments kept: versions.json couldn't be read");
        return;
    };
    // Each copy in gets a name of its own, so another note links one of these only through
    // what's saved (an assistant's change, a version put back), never just in its window.
    let orphans = store(|s| attach::orphans(&names, s.d.notes.iter().flat_map(|n| [n.rtf.as_str(), n.text.as_str()]).chain(versions.iter().map(String::as_str))));
    let dir = attach::dir();
    let (mut removed, mut failed) = (0, None);
    for name in &orphans {
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => failed = Some(e.kind()),
        }
    }
    match failed {
        Some(kind) => log::warn(&format!("deleted {removed} attachments of deleted notes; some couldn't be ({kind:?})")),
        None if removed > 0 => log::info(&format!("deleted {removed} attachments of deleted notes")),
        None => {}
    }
}

/// At start: take out of the attachments folder the files no note links any more (left
/// by a picture or file taken out of a note, or by notes deleted before files went with
/// them). It errs on keeping a file: a note in the archive, one deleted with its Undo
/// still showing, a saved version and what's on the clipboard (files copied in Explorer
/// too) all count as linking it; a file younger than a day, or than the notes' last save,
/// stays; and nothing goes unless notes.json was read whole (and is written this
/// session), versions.json can be read and no notes set aside are beside it, or when it
/// would take every file there. What goes is moved to `attachments\.trash\<day>\`, kept
/// there 30 days (`attach::TRASH_DAYS`). The notes' text is copied here, as it stands now;
/// the folder is read and the files moved on a thread of its own, which ends when it's done.
pub(super) fn sweep_attachments() {
    use crate::common::attach;
    let dir = attach::dir();
    // Most starts: nothing to look at.
    if !std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
        return;
    }
    let Some((mut kept, path)) = store(|s| {
        (s.whole() && !s.d.notes.is_empty()).then(|| (s.d.notes.iter().flat_map(|n| [n.rtf.clone(), n.text.clone()]).collect::<Vec<String>>(), s.path.clone()))
    }) else {
        return;
    };
    kept.extend(ui(|u| u.pending.iter().flat_map(|(_, n)| [n.rtf.clone(), n.text.clone()]).collect::<Vec<_>>()));
    // Cut out of a note in an earlier session, and not pasted back yet; or copied in
    // Explorer, as files, from the folder itself.
    kept.extend(crate::win::util::clipboard_rtf());
    kept.extend(crate::win::util::clipboard_text());
    kept.extend(crate::win::util::clipboard_file_names().iter().map(|n| attach::url(n)));
    let spawned = std::thread::Builder::new().name("attachment sweep".into()).spawn(move || sweep_in(&dir, &path, kept));
    if spawned.is_err() {
        log::warn("attachments not swept: no thread for it");
    }
}

/// `sweep_attachments`, off the UI thread: `kept` is every note's RTF and text; `notes`
/// is notes.json.
fn sweep_in(dir: &std::path::Path, notes: &std::path::Path, mut kept: Vec<String>) {
    use crate::common::attach;
    use std::os::windows::fs::MetadataExt;
    let t = crate::os::local_now();
    let gone = attach::purge_trash(dir, crate::common::util::local_day(&t));
    if gone > 0 {
        log::info(&format!("emptied {gone} days of swept attachments older than {} days", attach::TRASH_DAYS));
    }
    // Notes set aside unread at some start, or another writer's set aside before a save,
    // may come back, with the files they link.
    let data = notes.parent().unwrap_or(dir);
    if crate::common::store::others_kept(data) {
        log::info("attachments not swept: notes set aside (unreadable, or another copy's) are kept beside the notes");
        return;
    }
    let Some(versions) = crate::win::ui::note::versions::markdown() else {
        log::warn("attachments not swept: versions.json couldn't be read");
        return;
    };
    kept.extend(versions);
    let saved = std::fs::metadata(notes).and_then(|m| m.modified()).ok();
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    let (mut in_folder, mut old) = (0, std::collections::BTreeSet::new());
    for e in entries.flatten() {
        // Plain files only, by a name a note could link: never a folder, a link to
        // elsewhere, or what Windows or a sync client keeps there (desktop.ini).
        let (Ok(kind), Ok(meta)) = (e.file_type(), e.metadata()) else { continue };
        let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
        if !kind.is_file() || meta.file_attributes() & HIDDEN_OR_SYSTEM != 0 || attach::name_of(&attach::url(&name)).as_deref() != Some(name.as_str()) {
            continue;
        }
        in_folder += 1;
        if attach::sweepable(meta.created().ok(), meta.modified().ok(), saved, now) {
            old.insert(name);
        }
    }
    if old.is_empty() {
        return;
    }
    let gone = attach::sweep(&old, in_folder, kept.iter().map(String::as_str));
    drop(kept);
    let day = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    let (mut removed, mut failed) = (0, None);
    for name in &gone {
        match attach::to_trash(dir, name, &day) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => failed = Some(e.kind()),
        }
    }
    match failed {
        Some(kind) => log::warn(&format!("swept {removed} attachments no note links into the trash; some couldn't be ({kind:?})")),
        None if removed > 0 => log::info(&format!("swept {removed} attachments no note links into the trash")),
        None => {}
    }
}

pub fn show_all_notes(archived: bool) {
    allnotes::show(archived);
}

pub fn note_command(id: u64, cmd: u32) {
    match cmd {
        NC_OPEN => open_note(id),
        NC_PIN => toggle_pin(id),
        NC_DECK => {
            store_mut(|s| {
                if let Some(n) = s.note_mut(id) {
                    n.off_deck = !n.off_deck;
                }
            });
            save();
            refresh();
        }
        NC_ARCHIVE => {
            if store(|s| s.note(id).is_some_and(|n| n.archived)) {
                set_archived(&[id], false);
            } else {
                archive(&[id]);
            }
        }
        NC_DELETE => delete(vec![id]),
        NC_NEW => new_note(),
        NC_CYCLE_COLOR => {
            let c = store(|s| s.note(id).map(|n| n.color)).unwrap_or(0);
            set_color(id, theme::next_color(c));
        }
        c if (NC_COLOR0..NC_COLOR0 + theme::COLORS.len() as u32).contains(&c) => set_color(id, (c - NC_COLOR0) as u8),
        // Listed again, newest first, as the submenu listed them.
        c if (NC_VERSION0..NC_VERSION0 + NC_VERSION_ROWS).contains(&c) => {
            if let Some(v) = crate::win::ui::note::versions::list(id).get((c - NC_VERSION0) as usize) {
                crate::win::ui::note::restore_version(id, v);
            }
        }
        _ => {
            if let Some(h) = note_hwnd(id) {
                unsafe { PostMessageW(h, note::WM_NOTE_CMD, cmd as usize, 0) };
            }
        }
    }
}
