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
    let stale: Vec<(u64, String)> =
        store(|s| s.d.notes.iter().filter(|n| n.rtf.contains(crate::common::attach::SCHEME) && !n.text.contains("](attachment:")).map(|n| (n.id, n.rtf.clone())).collect());
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

pub fn rtf_to_markdown(rtf: &str) -> String {
    let w = ui(|u| u.worker);
    rich::set_rtf(w, rtf);
    rich::to_markdown(w, true)
}
/// Markdown (as `rtf_to_markdown` writes it) as a note's RTF.
pub fn markdown_to_rtf(md: &str) -> String {
    let w = ui(|u| u.worker);
    rich::load_markdown(w, md);
    rich::get_rtf(w)
}


pub(super) fn placement(id: u64) -> RECT {
    let saved = store(|s| s.note(id).and_then(|n| n.rect));
    if let Some([l, t, r, b]) = saved {
        let rc = RECT { left: l, top: t, right: r, bottom: b };
        if unsafe { !MonitorFromRect(&rc, MONITOR_DEFAULTTONULL).is_null() } && r - l > 50 && b - t > 50 {
            return rc;
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
            let h = note::create(id, r, false);
            deck::hand_off(id);
            // Shows what the window will (it's already laid out, scrolled to the end).
            let mut content = note::flight_content(h, &text);
            content.from_image = from_image;
            if text.trim().is_empty() {
                content.title.clear();
            }
            let (_, s) = deck::monitor_geo();
            crate::win::ui::ghost::fly(crate::win::ui::ghost::Kind::Open, from, (r, 8.0 * s), color, content, Some(Box::new(move || note::reveal(h))));
        }
        _ => {
            note::create(id, r, true);
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
    note::create(next, r, true);
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

/// At start: the notes open at quit that stay open, where they were.
pub(super) fn reopen_at_start() {
    let ids = store(|s| s.d.settings.reopen.iter().copied().filter(|id| s.note(*id).is_some_and(|n| !n.archived)).collect::<Vec<_>>());
    for id in ids {
        note::create(id, placement(id), true);
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
/// follows, so even a confirmed slip can be taken back.
pub fn delete_all(owner: HWND) {
    let ids: Vec<u64> = store(|s| s.d.notes.iter().map(|n| n.id).collect());
    if !ids.is_empty() && confirm_delete_all(owner, ids.len()) {
        delete(ids);
    }
}

/// "Delete All 25 Notes" or Cancel, in a menu at the pointer (on the button that asked):
/// themed like the rest of the app, and nothing is chosen until one is clicked, so Enter
/// or Esc never deletes anything.
fn confirm_delete_all(owner: HWND, n: usize) -> bool {
    use crate::win::ui::menu::{Item, action, danger};
    const ID_DELETE: u32 = 1;
    // Cancel first, under the pointer: a click that lands a little low still deletes nothing.
    let items = vec![action(2, t!("button_cancel"), "", ""), Item::Separator, danger(ID_DELETE, &tn!("menu_delete_all_notes", n), "", IC_DELETE)];
    let p = super::menus::cursor();
    popup(items, p.x, p.y, owner) == ID_DELETE
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
    ui(|u| {
        // Gone for good: their saved versions too.
        for (_, n) in &u.pending {
            crate::win::ui::note::versions::forget(n.id);
        }
        u.pending.clear();
        u.unarchive.clear();
    });
    toast::hide();
}

pub fn show_all_notes(archived: bool) {
    allnotes::show(archived);
}

pub fn note_command(id: u64, cmd: u32) {
    match cmd {
        NC_OPEN => open_note(id),
        NC_PIN => toggle_pin(id),
        NC_VERSIONS => versions_menu(id),
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
        _ => {
            if let Some(h) = note_hwnd(id) {
                unsafe { PostMessageW(h, note::WM_NOTE_CMD, cmd as usize, 0) };
            }
        }
    }
}
