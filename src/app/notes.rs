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

pub(super) fn rtf_to_markdown(rtf: &str) -> String {
    let w = ui(|u| u.worker);
    rich::set_rtf(w, rtf);
    rich::to_markdown(w, true)
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
        Some(from) if !crate::render::anim::reduced() => {
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
            crate::ui::ghost::fly(crate::ui::ghost::Kind::Open, from, (r, 8.0 * s), color, content, Some(Box::new(move || note::reveal(h))));
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
    } else {
        note::create(next, r, true);
    }
    true
}

pub(super) fn reopen_last() {
    let last = store(|s| s.d.settings.last_note.filter(|id| s.note(*id).is_some_and(|n| !n.archived)));
    match last.or_else(|| store(|s| s.active_ids().first().copied())) {
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
        u.pending.extend(removed);
        u.pending.len()
    });
    save();
    refresh();
    toast::show(count);
}

pub fn undo_delete() {
    let mut pending = ui(|u| std::mem::take(&mut u.pending));
    toast::hide();
    if pending.is_empty() {
        return;
    }
    pending.sort_by_key(|(i, _)| *i);
    store_mut(|s| {
        for (i, n) in pending {
            s.insert_at(i, n);
        }
    });
    save();
    refresh();
}

pub fn finalize_delete() {
    ui(|u| u.pending.clear());
    toast::hide();
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
            let archived = store(|s| s.note(id).is_some_and(|n| n.archived));
            set_archived(&[id], !archived);
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
