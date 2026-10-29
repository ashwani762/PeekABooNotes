// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

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

/// Where the inline editor sets a note's lines, in pixels at scale `s` from the top of its
/// window: (the title line's top, the first body line's top, the step from one one-line
/// paragraph to the next). The preview drawn until the editor is made sets its lines there
/// too, so the editor takes over without the text moving (the title sat 8 to 15 px lower
/// in the preview, its body lines closer together and lighter). Measured once for each
/// typography and scale in a hidden editor set up as a note's is.
pub(super) fn editor_pace(s: f32) -> Option<(f32, f32, f32)> {
    use crate::win::editor::rich;
    type Pace = Option<(f32, f32, f32)>;
    type Key = (u8, i32, i32, u32);
    thread_local! {
        static PACE: std::cell::Cell<Option<(Key, Pace)>> = const { std::cell::Cell::new(None) };
    }
    let key = (rich::font_index(), rich::body_size(), rich::title_size(), s.to_bits());
    if let Some((k, pace)) = PACE.with(|p| p.get())
        && k == key
    {
        return pace;
    }
    let pace = (|| {
        let h = unsafe {
            CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_POPUP | ES_MULTILINE as u32, 0, 0, 600, 300, null_mut(), null_mut(), null_mut(), null_mut())
        };
        if h.is_null() {
            return None;
        }
        rich::setup(h);
        rich::load_markdown(h, "Title\nFirst\nSecond");
        rich::apply_base_format(h, theme::PAPER_TEXT, 0xFFFFFF);
        rich::style_title(h);
        let k = s / (unsafe { GetDpiForWindow(h) }.max(96) as f32 / 96.0);
        let top = |cp: i32| rich::pos_from_char(h, cp).y as f32 * k;
        let (title, first, second) = (top(0), top(6), top(12));
        unsafe { DestroyWindow(h) };
        (second > first && first > title).then_some((title, first, second - first))
    })();
    PACE.with(|p| p.set(Some((key, pace))));
    pace
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

thread_local! {
    static FORCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `sync_editor`, the editor made now even while the card cross-fades: the keys go into it.
pub(super) fn sync_editor_now() {
    FORCE.with(|f| f.set(true));
    sync_editor();
    FORCE.with(|f| f.set(false));
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
                // Still behind its picture: shown now if the keys go into it, or if it moves
                // (the picture stays where it was).
                let (mut was, mut now): (RECT, RECT) = (std::mem::zeroed(), std::mem::zeroed());
                GetWindowRect(have.0, &mut was);
                MoveWindow(have.0, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
                GetWindowRect(have.0, &mut now);
                let moved = (was.left, was.top, was.right, was.bottom) != (now.left, now.top, now.right, now.bottom);
                if FORCE.with(|f| f.get()) || moved {
                    reveal_editor();
                }
            },
            _ => {
                if !have.0.is_null() {
                    with(|l| {
                        l.editor = null_mut();
                        l.editor_id = 0;
                        l.editor_first = None;
                    });
                    unsafe { DestroyWindow(have.0) }; // flushes its edits first
                }
                if let Some((id, r)) = want {
                    // Not while rows close a gap (a note archived or deleted, or back): a
                    // new editor's first paint costs a frame 20 ms or more, a hitch in
                    // their motion. Its note shows as a preview meanwhile. Nor while the
                    // card cross-fades from what it showed before: the editor, a window of
                    // its own, came up whole at once inside the fading card, a hard-edged
                    // inset of the new note in the old one; it comes as the fade ends
                    // (`frame`), unless the fade has only just set off (below). Unless the
                    // keys are going into it now (Enter).
                    let force = FORCE.with(|f| f.get());
                    let fading = with(|l| l.old_detail.is_some()).unwrap_or(false);
                    if force && fading {
                        with(|l| {
                            l.old_detail = None;
                            l.detail_a.snap(1.0);
                        });
                    }
                    // A cross-fade only just set off (Open Here, another note clicked), none
                    // of what it fades to drawn yet: the editor comes now, behind its picture,
                    // and the card fades straight to that. Made after the fade, the card
                    // faded to the note's plain preview first (its picture smaller, its links
                    // plain), and half a second on to the editor's picture over it: two
                    // changes, the second with both pictures showing. So too while rows close
                    // a gap, as a note comes back with Undo: made before the rows' first
                    // frame, the editor only holds their start a little, where made after
                    // them the card faded twice, the second time with both layouts showing.
                    let joins = !force && fading && with(|l| l.detail_a.value <= 0.0).unwrap_or(false);
                    if !force && !joins && (fading || with(|l| super::list::gaps_moving(l)).unwrap_or(false)) {
                        unsafe { SetTimer(hwnd, T_SYNC, 320, None) };
                    } else {
                        let h = crate::win::ui::note::create_embedded(id, hwnd, r);
                        // Unless the keys go into it now, it comes in unseen, behind a picture
                        // of it that the card cross-fades to from the preview, and shows once
                        // that's done (`frame`), on the same pixels. Shown at once, it took
                        // over from the preview with every glyph's edges changing: the
                        // preview is set by DirectWrite, the editor by RichEdit, a change all
                        // over the text half a second after the card had settled.
                        let first = if force { None } else { first_picture(h) };
                        with(|l| {
                            l.editor = h;
                            l.editor_id = id;
                            // A picture of an editor closed before is out of date now.
                            l.editor_pic = None;
                            l.editor_first = first;
                            // Joining a cross-fade under way: it goes on, to the picture.
                            if l.editor_first.is_some() && !joins && !fade_to_editor(l) {
                                l.editor_first = None;
                            }
                        });
                        if with(|l| l.editor_first.is_none()).unwrap_or(true) {
                            unsafe { ShowWindow(h, SW_SHOWNA) };
                            crate::win::ui::note::embedded_shown(h);
                        }
                    }
                }
            }
        }
    }
    SYNCING.with(|s| s.set(false));
}

/// Inline editor `h`, just made, hidden and drawn: its text where it sits in the window and
/// its pixels, as it will paint them once shown. None when it can't be drawn so (it's then
/// shown at once).
fn first_picture(h: HWND) -> Option<(RECT, Vec<u32>)> {
    use crate::win::editor::view;
    unsafe {
        ShowWindow(h, SW_HIDE);
        let edit = GetWindow(h, GW_CHILD);
        if edit.is_null() {
            return None;
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(edit, &mut r);
        windows_sys::Win32::Graphics::Gdi::MapWindowPoints(null_mut(), GetParent(h), &mut r as *mut RECT as *mut _, 2);
        let (w, ht) = (r.right - r.left, r.bottom - r.top);
        if w <= 0 || ht <= 0 {
            return None;
        }
        let mut c = Canvas::new(w, ht);
        view::compose(edit, &mut c, 0, w, ht, view::Parts::All).then(|| (r, c.copy_out(rect(0, 0, w, ht))))
    }
}

/// Show the inline editor that came in behind its picture (see `sync_editor`), if it's
/// still hidden: its picture's cross-fade is over, or the keys go into it, or what it shows
/// changed.
pub(super) fn reveal_editor() {
    if let Some(h) = with(|l| l.editor_first.take().map(|_| l.editor)).flatten()
        && !h.is_null()
    {
        unsafe { ShowWindow(h, SW_SHOWNA) };
        crate::win::ui::note::embedded_shown(h);
    }
}

/// Inline editor `h` as it is on screen, and where in the window.
pub(super) fn editor_picture(h: HWND) -> Option<(RECT, crate::win::render::gfx::Image)> {
    // Hidden behind its picture, which the window's canvas holds already.
    if unsafe { IsWindowVisible(h) } == 0 {
        return None;
    }
    crate::win::render::gfx::capture_now(h).map(|img| {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetWindowRect(h, &mut r);
            windows_sys::Win32::Graphics::Gdi::MapWindowPoints(null_mut(), GetParent(h), &mut r as *mut RECT as *mut _, 2);
        }
        (r, img)
    })
}

/// Save and remove the inline editor (before actions that change or move the note).
pub(super) fn close_editor() {
    if let Some(h) = with(|l| {
        let h = l.editor;
        l.editor = null_mut();
        l.editor_id = 0;
        l.editor_first = None;
        h
    })
        && !h.is_null() {
            // Its picture, as it was on screen, for the card's cross-fade to whatever comes
            // next (see `swap_detail`).
            let pic = editor_picture(h);
            with(|l| l.editor_pic = pic);
            SYNCING.with(|s| s.set(true));
            unsafe { DestroyWindow(h) };
            SYNCING.with(|s| s.set(false));
        }
}
