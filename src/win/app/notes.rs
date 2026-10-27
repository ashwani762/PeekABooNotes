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
    // Frozen: read in and laid out once (see `rich::reading`).
    rich::bulk(w, || {
        rich::set_rtf(w, rtf);
        rich::to_markdown(w, false)
    })
}

/// Notes whose light text an older version wrote (pictures and files as bare names,
/// checklists in quotes as "> ☐"): bring it up to date from their rich text, once,
/// without touching when they were edited.
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
    rich::bulk(w, || {
        rich::set_rtf(w, rtf);
        rich::to_markdown(w, true)
    })
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
    let (ex, cy, s, work, left) = deck::anchor(Some(id));
    let saved = store(|s| s.note(id).and_then(|n| n.rect));
    if let Some([l, t, r, b]) = saved
        && r - l > 50
        && b - t > 50
    {
        // Back where it was, on the deck's display: a spot on another one (the displays
        // rearranged, or the deck moved since) would open it out of sight there, so it
        // opens beside its card instead. With enough of it on screen to take hold of.
        let rc = RECT { left: l, top: t, right: r, bottom: b };
        unsafe {
            let mon = MonitorFromRect(&rc, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = size_of::<MONITORINFO>() as u32;
            if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 && same_display(mi.rcWork, work) {
                return clamp_to_work(rc, mi.rcWork);
            }
        }
    }
    let (w, h) = ((320.0 * s) as i32, (360.0 * s) as i32);
    let gap = (8.0 * s) as i32;
    let x = if left { ex + gap } else { ex - w - gap };
    let y = (cy - (60.0 * s) as i32).clamp(work.top + gap, (work.bottom - h - gap).max(work.top));
    // Not right on top of a note open there already (new notes made one after another).
    let open: Vec<(i32, i32)> = ui(|u| u.notes.values().copied().collect::<Vec<_>>())
        .into_iter()
        .filter_map(|h| {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            (unsafe { GetWindowRect(h, &mut r) } != 0).then_some((r.left, r.top))
        })
        .collect();
    cascade(RECT { left: x, top: y, right: x + w, bottom: y + h }, &open, (24.0 * s) as i32, work, left)
}

/// Whether work areas `a` and `b` are on the same display: one's centre lies in the other.
fn same_display(a: RECT, b: RECT) -> bool {
    let (cx, cy) = ((a.left + a.right) / 2, (a.top + a.bottom) / 2);
    cx >= b.left && cx < b.right && cy >= b.top && cy < b.bottom
}

/// Where a note first opening at `r` goes when an open note's corner (of `taken`) is there
/// already: a step down and away from the deck (`left`: the deck is on the left), as often
/// as it takes, while it stays in `work`; past that, where it was.
fn cascade(r: RECT, taken: &[(i32, i32)], step: i32, work: RECT, left: bool) -> RECT {
    let near = |x: i32, y: i32| taken.iter().any(|&(tx, ty)| (tx - x).abs() < step / 2 && (ty - y).abs() < step / 2);
    let mut at = r;
    for _ in 0..20 {
        if !near(at.left, at.top) {
            return at;
        }
        let dx = if left { step } else { -step };
        at = RECT { left: at.left + dx, top: at.top + step, right: at.right + dx, bottom: at.bottom + step };
        if at.bottom > work.bottom || at.left < work.left || at.right > work.right {
            break;
        }
    }
    r
}

/// Open a note, growing out of its card in the deck: the card fanned out, else its capsule
/// in the pill (Reopen Last Note, the tray), when the deck is showing it.
pub fn open_note(id: u64) {
    open_note_from(id, deck::card_rect(id).or_else(|| deck::holds(id).then(|| deck::home_rect(id))));
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
    let Some((color, text, placed)) = store(|s| s.note(id).map(|n| (n.color, n.text.clone(), n.rect.is_some()))) else { return };
    let r = placement(id);
    if !placed {
        // Where it first opens (beside its card in the deck) is where it opens again,
        // moved or not: from the pill or the tray it would be beside the pill instead.
        store_mut(|s| {
            if let Some(n) = s.note_mut(id) {
                n.rect = Some([r.left, r.top, r.right, r.bottom]);
            }
        });
        // A new note, empty, goes with the first save of what's typed in it (or with the
        // window, unkept, if nothing is): writing every note now held its card up by the
        // time the file took.
        if !text.trim().is_empty() {
            save();
        }
    }
    match from {
        Some(from) if !crate::win::render::anim::reduced() => {
            // Its picture before the deck tucks it away, when it flies from that card: a new
            // note grows from the plus button (its card already dealt into the deck), a note
            // from the pill from its capsule, and neither looks like the card.
            let same = |a: &RECT| (a.left, a.top, a.right, a.bottom) == (from.0.left, from.0.top, from.0.right, from.0.bottom);
            let from_card = deck::card_rect(id).is_some_and(|(r, _)| same(&r));
            let from_plus = deck::plus_rect().is_some_and(|(r, _)| same(&r));
            let from_image = if from_card {
                deck::card_image(id)
            } else if from_plus {
                deck::plus_image()
            } else {
                None
            };
            // The window first, unseen, and its picture drawn as it's made (a few ms): the card
            // sets off with it, moving in the next frame. Made while the card's first frames
            // showed, it held the card still 100 ms and more, and its picture, from the
            // compositor, came frames later, each try a late frame. Without a picture it's
            // paper until the compositor has one; the words are there only should none come.
            let h = note::create(id, r, note::Show::Later);
            let picture = note::picture(h);
            deck::hand_off(id);
            let title = if text.trim().is_empty() { String::new() } else { crate::common::store::title_of(&text) };
            let content = crate::win::ui::ghost::Content { title, body: crate::common::store::body_of(&text), body_top: None, image: picture, from_image };
            // The window's corners, at the scale of the monitor it opens on (not the deck's).
            let s = crate::win::ui::screen::of_rect(&r).1;
            let ghost = crate::win::ui::ghost::fly(crate::win::ui::ghost::Kind::Open, from, (r, 8.0 * s), color, content, None);
            crate::win::ui::ghost::adopt(ghost, h, Some(Box::new(move || note::reveal(h))));
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
    // The deck is drawn again once the note is open (as an open note, with no card in the
    // stack): drawn before, it showed one more note past its end for a frame ("+103", then
    // "+102").
    open_note_from(id, Some(from));
    allnotes::refresh();
    settings::notes_changed();
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
    // Its capsule in the deck shows the pin.
    deck::refresh();
}

/// Note `id`'s window, if it's open, saved and gone (the note archived or deleted).
fn put_away(id: u64) {
    if let Some(h) = note_hwnd(id) {
        note::flush(h);
        note::dissolve(h);
        note::discard(h);
    }
}

/// Archive or restore notes. A note keeps its place in the deck while it's archived
/// (`Store::reorder` keeps it among the others), so restored it comes back where it was.
pub fn set_archived(ids: &[u64], archived: bool) {
    if archived {
        ids.iter().for_each(|&id| put_away(id));
    }
    store_mut(|s| {
        for &id in ids {
            if let Some(n) = s.note_mut(id) {
                n.archived = archived;
            }
        }
    });
    save();
    refresh();
}

/// What the Undo toast can take back (see `common::undo`): one entry of the stack.
pub enum Undo {
    /// Notes deleted, with their places among the notes, in the order they were taken out.
    Deleted(Vec<(usize, Note)>),
    /// Notes archived (each keeps its place in the deck meanwhile).
    Archived(Vec<u64>),
    /// Notes restored from the archive.
    Restored(Vec<u64>),
    /// Notes an import brought in, how many files it left out as notes here already, and
    /// how many it couldn't read (or held more than a note can). Each import is one of its
    /// own.
    Imported(Vec<u64>, usize, usize),
}

impl crate::common::undo::Action for Undo {
    fn merge(&mut self, newer: Undo) -> Option<Undo> {
        match (self, newer) {
            (Undo::Deleted(a), Undo::Deleted(b)) => {
                a.extend(b);
                None
            }
            (Undo::Archived(a), Undo::Archived(b)) => {
                a.extend(b);
                None
            }
            (Undo::Restored(a), Undo::Restored(b)) => {
                a.extend(b);
                None
            }
            (_, newer) => Some(newer),
        }
    }
}

impl Undo {
    /// What the toast says about it.
    fn message(&self) -> String {
        match self {
            Undo::Deleted(v) => tn!("toast_deleted", v.len()),
            Undo::Archived(v) => tn!("toast_archived", v.len()),
            Undo::Restored(v) => tn!("toast_restored", v.len()),
            Undo::Imported(v, here, unread) => import_said(v.len(), *here, *unread),
        }
    }

    /// The tray menu's row that takes it back.
    fn menu_label(&self) -> &'static str {
        match self {
            Undo::Deleted(_) => t!("menu_undo_delete"),
            Undo::Archived(_) => t!("menu_undo_archive"),
            Undo::Restored(_) => t!("menu_undo_restore"),
            Undo::Imported(..) => t!("menu_undo_import"),
        }
    }
}

/// `a` was just done: it goes on the undo stack, and the toast shows it.
fn done(a: Undo) {
    ui(|u| u.undo.push(a));
    show_undo();
}

/// The toast shows what Undo takes back now, or goes when there's nothing left.
fn show_undo() {
    toast::undo(ui(|u| u.undo.top().map(|e| e.action.message())));
}

/// The tray menu's Undo row, while the toast has something to take back.
pub(super) fn undo_label() -> Option<&'static str> {
    ui(|u| u.undo.top().map(|e| e.action.menu_label()))
}

/// Archive notes from a click or key, with the Undo toast, as deleting has.
pub fn archive(ids: &[u64]) {
    let away: Vec<u64> = store(|s| ids.iter().copied().filter(|id| s.note(*id).is_some_and(|n| !n.archived)).collect());
    if away.is_empty() {
        return;
    }
    set_archived(&away, true);
    done(Undo::Archived(away));
}

/// What an import's toast says: the notes it brought in, the files it left out as notes
/// here already, and those it couldn't read ("1 note imported, 2 already here, 1 couldn't
/// be read"); one of them alone is said in full ("3 notes already here").
fn import_said(added: usize, here: usize, unread: usize) -> String {
    let alone = |n: usize| n == added + here + unread;
    let mut parts = Vec::new();
    if added > 0 {
        parts.push(if alone(added) { tn!("toast_imported", added) } else { tn!("toast_imported_some", added) });
    }
    if here > 0 {
        parts.push(if alone(here) { tn!("toast_already_here", here) } else { tn!("result_already_here", here) });
    }
    if unread > 0 {
        parts.push(if alone(unread) { tn!("toast_unread", unread) } else { tn!("result_unread", unread) });
    }
    parts.into_iter().reduce(|a, b| t!("result_and", a, b)).unwrap_or_default()
}

/// Notes `ids` were just imported, `here` files left out as notes here already and
/// `unread` ones that couldn't be read: the toast offers to take the notes out again, and
/// says what was left out. With none brought in, it just says so.
pub(super) fn imported(ids: Vec<u64>, here: usize, unread: usize) {
    if ids.is_empty() {
        if here + unread > 0 {
            toast::notice(import_said(0, here, unread));
        }
        return;
    }
    done(Undo::Imported(ids, here, unread));
}

/// Bring notes back from the archive, to their places in the deck, with the Undo toast,
/// as archiving has.
pub fn restore(ids: &[u64]) {
    let back: Vec<u64> = store(|s| ids.iter().copied().filter(|id| s.note(*id).is_some_and(|n| n.archived)).collect());
    if back.is_empty() {
        return;
    }
    set_archived(&back, false);
    done(Undo::Restored(back));
}

pub fn delete(ids: Vec<u64>) {
    ids.iter().for_each(|&id| put_away(id));
    let removed: Vec<(usize, Note)> = store_mut(|s| ids.iter().filter_map(|id| s.remove(*id)).collect());
    if removed.is_empty() {
        return;
    }
    save();
    refresh();
    done(Undo::Deleted(removed));
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
    ui(|u| !u.undo.is_empty())
}

/// The toast's Undo (its button, Ctrl+Z in All Notes, the tray menu): the newest action
/// still in time is taken back, and the toast shows the one before it, if any. Notes
/// deleted come back where they were, notes archived return to the deck where they were,
/// notes restored go back to the archive, and notes imported go (for good, with the files
/// they brought).
pub fn undo() {
    let Some(a) = ui(|u| u.undo.pop()) else { return };
    show_undo();
    let archive = matches!(a, Undo::Restored(_));
    let ids: Vec<u64> = match a {
        // Each back in its place: archiving left it there (see `set_archived`).
        Undo::Archived(v) | Undo::Restored(v) => {
            if archive {
                v.iter().for_each(|&id| put_away(id));
            }
            store_mut(|s| {
                for id in &v {
                    if let Some(n) = s.note_mut(*id) {
                        n.archived = archive;
                    }
                }
            });
            v
        }
        Undo::Deleted(v) => {
            let ids = v.iter().map(|(_, n)| n.id).collect();
            store_mut(|s| s.put_back(v));
            ids
        }
        Undo::Imported(v, ..) => {
            v.iter().for_each(|&id| put_away(id));
            let gone: Vec<Note> = store_mut(|s| v.iter().filter_map(|id| s.remove(*id)).map(|(_, n)| n).collect());
            save();
            refresh();
            gone_for_good(&gone);
            return;
        }
    };
    save();
    refresh();
    // What came back is what All Notes shows.
    crate::win::ui::allnotes::select(&ids);
}

/// The toast's clock ran `dt` seconds (0 while the pointer holds it): every action whose
/// own time ran out is made final, and the toast shows what's left. The seconds the newest
/// has left, if any.
pub fn undo_clock(dt: f32) -> Option<f32> {
    let due = ui(|u| if dt > 0.0 { u.undo.tick(dt) } else { Vec::new() });
    if !due.is_empty() {
        for a in due {
            make_final(a);
        }
        show_undo();
    }
    ui(|u| u.undo.top().map(|e| e.left))
}

/// Every action the toast could still take back is final now (quitting).
pub fn finalize_undo() {
    for a in ui(|u| u.undo.drain()) {
        make_final(a);
    }
}

/// An action that can't be taken back any more: deleted notes are gone for good, their
/// saved versions too, and then the files only they linked. An archive just stays.
fn make_final(a: Undo) {
    let Undo::Deleted(v) = a else { return };
    gone_for_good(&v.into_iter().map(|(_, n)| n).collect::<Vec<_>>());
}

/// Notes taken out of the store for good: their saved versions go, and then the files
/// only they linked.
fn gone_for_good(gone: &[Note]) {
    crate::win::ui::note::versions::forget(&gone.iter().map(|n| n.id).collect::<Vec<_>>());
    delete_attachments(gone);
}

/// Take the attachments notes `gone` linked out of the folder, unless another note, a
/// note deleted whose Undo is still in time, or a saved version (another note's, put back
/// later) still links them.
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
    let pending: Vec<String> = ui(|u| pending_notes(u).flat_map(|n| [links_of(&n.rtf), n.text.clone()]).collect());
    let orphans = store(|s| attach::orphans(&names, s.d.notes.iter().flat_map(|n| [n.rtf.as_str(), n.text.as_str()]).chain(pending.iter().map(String::as_str)).chain(versions.iter().map(String::as_str))));
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

/// The notes deleted whose Undo is still in time.
fn pending_notes(u: &Ui) -> impl Iterator<Item = &Note> {
    u.undo.iter().flat_map(|a| match a {
        Undo::Deleted(v) => v.as_slice(),
        _ => &[],
    })
    .map(|(_, n)| n)
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
        (s.whole() && !s.d.notes.is_empty()).then(|| (s.d.notes.iter().flat_map(|n| [links_of(&n.rtf), n.text.clone()]).collect::<Vec<String>>(), s.path.clone()))
    }) else {
        return;
    };
    kept.extend(ui(|u| pending_notes(u).flat_map(|n| [links_of(&n.rtf), n.text.clone()]).collect::<Vec<_>>()));
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

/// A note's RTF as the sweep reads it: each picture's group (most of a note with
/// pictures, and never a link) left as an empty one, so the copy handed to the sweep holds
/// only what can link. A group that never ends is left as it is.
fn links_of(rtf: &str) -> String {
    let b = rtf.as_bytes();
    let mut out = String::with_capacity(rtf.len().min(64 * 1024));
    let mut from = 0;
    while let Some(i) = rtf[from..].find("{\\pict").map(|i| from + i) {
        // To its closing brace: from brace to brace (a picture's data has none), past
        // escaped ones.
        let (mut depth, mut j, mut end) = (0, i, None);
        // The next brace: the next closing one, unless an opening one comes first (each
        // looked for on its own, which is quick through a picture's data).
        let next = |j: usize| rtf[j..].find('}').map(|c| rtf[j..j + c].find('{').unwrap_or(c) + j);
        while let Some(k) = next(j) {
            let escaped = k > 0 && b[k - 1] == b'\\' && !(k > 1 && b[k - 2] == b'\\');
            if !escaped {
                depth += if b[k] == b'{' { 1 } else { -1 };
                if depth == 0 {
                    end = Some(k + 1);
                    break;
                }
            }
            j = k + 1;
        }
        let Some(end) = end else { break };
        out.push_str(&rtf[from..i]);
        out.push_str("{}");
        from = end;
    }
    out.push_str(&rtf[from..]);
    out
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
                restore(&[id]);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A new note's spot taken by an open note: a step down and away from the deck, again
    /// for each one there, and back to the spot when the steps would leave the screen.
    #[test]
    fn new_notes_cascade() {
        let work = RECT { left: 0, top: 0, right: 2000, bottom: 1000 };
        let r = RECT { left: 1500, top: 100, right: 1820, bottom: 460 };
        let at = |r: RECT| (r.left, r.top);
        assert_eq!(at(cascade(r, &[], 24, work, false)), (1500, 100));
        assert_eq!(at(cascade(r, &[(1503, 98)], 24, work, false)), (1476, 124));
        assert_eq!(at(cascade(r, &[(1500, 100), (1476, 124)], 24, work, false)), (1452, 148));
        // The deck on the left: away from it is to the right.
        let l = RECT { left: 100, top: 100, right: 420, bottom: 460 };
        assert_eq!(at(cascade(l, &[(100, 100)], 24, work, true)), (124, 124));
        // No room further down: where it was.
        let low = RECT { left: 1500, top: 630, right: 1820, bottom: 990 };
        assert_eq!(at(cascade(low, &[(1500, 630)], 24, work, false)), (1500, 630));
    }

    /// An import's toast says what was left out as notes here already, and what couldn't be
    /// read, beside what came in.
    #[test]
    fn an_import_toast_says_what_was_here_already() {
        crate::common::i18n::init("en");
        assert_eq!(Undo::Imported(vec![1], 0, 0).message(), "Note imported");
        assert_eq!(Undo::Imported(vec![1, 2, 3], 0, 0).message(), "3 notes imported");
        assert_eq!(Undo::Imported(vec![1], 2, 0).message(), "1 note imported, 2 already here");
        assert_eq!(Undo::Imported(vec![1, 2], 1, 0).message(), "2 notes imported, 1 already here");
        assert_eq!(Undo::Imported(vec![1, 2], 0, 1).message(), "2 notes imported, 1 couldn't be read");
        assert_eq!(Undo::Imported(vec![1], 2, 3).message(), "1 note imported, 2 already here, 3 couldn't be read");
        // Nothing came in: what was left out, alone or together.
        assert_eq!(import_said(0, 1, 0), "Note already here");
        assert_eq!(import_said(0, 3, 0), "3 notes already here");
        assert_eq!(import_said(0, 0, 1), "File couldn't be read");
        assert_eq!(import_said(0, 0, 2), "2 files couldn't be read");
        assert_eq!(import_said(0, 2, 1), "2 already here, 1 couldn't be read");
    }

    /// The sweep's copy of a note leaves its pictures' data out, and every link in.
    #[test]
    fn the_sweep_reads_the_links_not_the_pictures() {
        let pict = crate::win::editor::rich::test_picture(40, 30, 600, 450);
        let link = |name: &str| format!("{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt{{\\u8203?}}}}}}", crate::common::attach::url(name));
        let rtf = format!("{{\\rtf1 a \\{{not a group {pict}{}\\par {{\\*\\shppict{pict}}}{}\\par b}}", link("one.png"), link("two {x}.png"));
        let read = links_of(&rtf);
        assert!(read.len() < rtf.len() / 4, "{} of {}", read.len(), rtf.len());
        assert!(!read.contains("\\pict"));
        assert_eq!(crate::common::attach::linked(&read), crate::common::attach::linked(&rtf));
        assert_eq!(read, format!("{{\\rtf1 a \\{{not a group {{}}{}\\par {{\\*\\shppict{{}}}}{}\\par b}}", link("one.png"), link("two {x}.png")));
        // Nothing to leave out, or a group that never ends: as it is.
        assert_eq!(links_of("{\\rtf1 plain}"), "{\\rtf1 plain}");
        assert_eq!(links_of("{\\rtf1 {\\pict 0a0b"), "{\\rtf1 {\\pict 0a0b");
    }

    /// Timing (run with --ignored --nocapture): what the start-up sweep copies of 2,000
    /// notes, a tenth of them with a picture, on the UI thread.
    #[test]
    #[ignore]
    fn probe_sweep_copy() {
        let pict = crate::win::editor::rich::test_picture(200, 150, 3000, 2250);
        let notes: Vec<(String, String)> = (0..2000)
            .map(|i| {
                let body = "Paragraph: we leave early on Friday and come back on Sunday evening.\\par ".repeat(6);
                let p = if i % 10 == 0 { pict.as_str() } else { "" };
                (format!("{{\\rtf1 Note {i}\\par {body}{p}\\par end}}"), format!("Note {i}\nwe leave early on Friday"))
            })
            .collect();
        let time = |f: &dyn Fn() -> Vec<String>| {
            let mut best = std::time::Duration::MAX;
            let mut bytes = 0;
            for _ in 0..5 {
                let t0 = std::time::Instant::now();
                let v = f();
                best = best.min(t0.elapsed());
                bytes = v.iter().map(String::len).sum::<usize>();
            }
            (best, bytes / 1024)
        };
        let cloned = time(&|| notes.iter().flat_map(|(r, t)| [r.clone(), t.clone()]).collect());
        let read = time(&|| notes.iter().flat_map(|(r, t)| [links_of(r), t.clone()]).collect());
        eprintln!("PROBE sweep's copy of 2000 notes: every RTF whole {:?}, {} KB; pictures left out {:?}, {} KB", cloned.0, cloned.1, read.0, read.1);
    }
}
