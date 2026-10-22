//! The inline editor: a note editor hosted inside the detail card.

use super::*;

/// Where the inline editor sits inside the detail card (library client coordinates).
pub(super) fn editor_rect(l: &Lib, lay: &Layout) -> RECT {
    let s = l.s;
    let card = lay.card;
    let sw_top = swatch_rects(lay, s)[0].top as f32;
    // RichEdit insets its text by its selection-bar margin; offset so the text lines up
    // with the card's footer and title column at 28px.
    let (x0, x1) = (card.x + 19.0 * s, card.x + card.w - 18.0 * s);
    let (y0, y1) = (card.y + 18.0 * s, sw_top - 52.0 * s);
    rect(x0 as i32, y0 as i32, (x1 - x0).max(10.0) as i32, (y1 - y0).max(10.0) as i32)
}

/// The note to edit inline: the single selected note, unless it's open in its own window
/// (one note is never edited in two places at once).
pub(super) fn wanted_editor(l: &Lib) -> Option<(u64, RECT)> {
    let lay = layout(l);
    if !lay.wide || l.sel.len() != 1 {
        return None;
    }
    let id = *l.sel.last()?;
    if app::ui(|u| u.notes.contains_key(&id)) {
        return None;
    }
    Some((id, editor_rect(l, &lay)))
}

pub(super) fn editor_focused(l: &Lib) -> bool {
    if l.editor.is_null() {
        return false;
    }
    unsafe {
        let f = GetFocus();
        f == l.editor || IsChild(l.editor, f) != 0
    }
}

/// Create, move or retire the inline editor to match the selection. Runs outside the
/// library's state borrow (creating/destroying windows re-enters `refresh`).
pub(super) fn sync_editor() {
    if SYNCING.with(|s| s.replace(true)) {
        return;
    }
    if let Some((hwnd, want, have)) = with(|l| (l.hwnd, wanted_editor(l), (l.editor, l.editor_id))) {
        match want {
            Some((id, r)) if have.1 == id && !have.0.is_null() => unsafe {
                MoveWindow(have.0, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
            },
            _ => {
                if !have.0.is_null() {
                    with(|l| {
                        l.editor = null_mut();
                        l.editor_id = 0;
                    });
                    unsafe { DestroyWindow(have.0) }; // flushes its edits first
                }
                if let Some((id, r)) = want {
                    let h = crate::ui::note::create_embedded(id, hwnd, r);
                    with(|l| {
                        l.editor = h;
                        l.editor_id = id;
                    });
                }
            }
        }
    }
    SYNCING.with(|s| s.set(false));
}

/// Save and remove the inline editor (before actions that change or move the note).
pub(super) fn close_editor() {
    if let Some(h) = with(|l| {
        let h = l.editor;
        l.editor = null_mut();
        l.editor_id = 0;
        h
    })
        && !h.is_null() {
            SYNCING.with(|s| s.set(true));
            unsafe { DestroyWindow(h) };
            SYNCING.with(|s| s.set(false));
        }
}
