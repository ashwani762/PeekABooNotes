// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Keyboard focus: the Tab order through the window's controls, and what keys do there.

use super::*;

/// Every Tab stop, in order: the list's column top to bottom, the detail pane top to
/// bottom, then the toolbar beside the title.
const TAB_ORDER: [Focus; 12] = [
    Focus::Search,
    Focus::Segments,
    Focus::List,
    Focus::Elsewhere,
    Focus::Archive,
    Focus::Delete,
    Focus::Editor,
    Focus::OpenHere,
    Focus::Swatches,
    Focus::New,
    Focus::Share,
    Focus::Settings,
];

/// Whether `f` is on screen now (the detail pane folds away in a narrow window).
fn live(l: &Lib, lay: &Layout, f: Focus) -> bool {
    let detail = lay.wide && !l.sel.is_empty();
    match f {
        Focus::List => !l.rows.is_empty(),
        Focus::Elsewhere => elsewhere_link(l, lay).is_some(),
        Focus::Archive | Focus::Delete => detail,
        Focus::Editor => !l.editor.is_null(),
        Focus::OpenHere => detail && show_window_rect(l, lay).is_some(),
        Focus::Swatches => detail && l.sel.len() == 1 && show_window_rect(l, lay).is_none(),
        _ => true,
    }
}

/// What has the keys: a child control, or the window's own focused control.
fn focused(l: &Lib) -> Option<Focus> {
    let f = unsafe { GetFocus() };
    if f == l.search {
        Some(Focus::Search)
    } else if editor_focused(l) {
        Some(Focus::Editor)
    } else if f == l.hwnd {
        Some(l.kbd)
    } else {
        None
    }
}

/// Where Tab (or Shift+Tab) goes from here.
pub(super) fn next_focus(l: &Lib, back: bool) -> Option<Focus> {
    let lay = layout(l);
    crate::win::ui::focus::step(&TAB_ORDER, focused(l), back, true, |f| live(l, &lay, f))
}

/// The control wearing the focus ring: only once the keyboard has moved focus, and only
/// while the window itself, not one of its child controls, has the keys.
pub(super) fn ring_on(l: &Lib) -> Option<Focus> {
    (l.cues && unsafe { GetFocus() } == l.hwnd).then_some(l.kbd)
}

/// The button a focus stands for, to press it or draw its ring.
pub(super) fn hit_of(l: &Lib, f: Focus) -> Hit {
    match f {
        Focus::Elsewhere => Hit::Elsewhere,
        Focus::Archive => Hit::Archive,
        Focus::Delete => Hit::Delete,
        Focus::OpenHere => Hit::OpenHere,
        Focus::Swatches => Hit::Swatch(l.swatch_k),
        Focus::New => Hit::New,
        Focus::Share => Hit::Share,
        Focus::Settings => Hit::Settings,
        _ => Hit::None,
    }
}

/// Give `f` the keys. Outside the window's borrow: moving focus re-enters it.
pub(super) fn focus_to(hwnd: HWND, f: Focus) {
    let Some((search, editor)) = with(|l| {
        l.kbd = f;
        l.cues = true;
        match f {
            Focus::List => {
                if let Some(i) = primary_index(l) {
                    ensure_visible(l, i);
                }
            }
            // Onto the note's own colour, as a radio group starts at its choice.
            Focus::Swatches => {
                let color = primary_index(l).map_or(0, |i| l.rows[i].color % theme::COLORS.len() as u8);
                l.swatch_k = theme::ORDER.iter().position(|&o| o == color).unwrap_or(0);
            }
            _ => {}
        }
        (l.search, l.editor)
    }) else {
        return;
    };
    unsafe {
        match f {
            Focus::Search => {
                SetFocus(search);
                SendMessageW(search, EM_SETSEL, 0, -1);
            }
            Focus::Editor => {
                SetFocus(editor);
            }
            _ if GetFocus() != hwnd => {
                SetFocus(hwnd);
            }
            _ => {}
        }
    }
    paint_now(hwnd);
    anim::wake(Client::List);
}

/// Space or Enter on the focused control, when it's one that's pressed.
pub(super) fn activate(l: &mut Lib) -> Option<Action> {
    match l.kbd {
        Focus::Elsewhere => {
            set_mode(l, if l.mode == 1 { 0 } else { 1 });
            l.kbd = Focus::List;
            Some(Action::None)
        }
        Focus::Archive | Focus::Delete | Focus::OpenHere | Focus::Swatches | Focus::New | Focus::Share | Focus::Settings => Some(click(l, hit_of(l, l.kbd))),
        _ => None,
    }
}

/// Left or Right: the segments switch the list, the swatches move along (Space picks one).
pub(super) fn nudge(l: &mut Lib, by: i32) -> bool {
    match l.kbd {
        Focus::Segments => set_mode(l, (l.mode as i32 + by).clamp(0, segments().len() as i32 - 1) as usize),
        Focus::Swatches => l.swatch_k = (l.swatch_k as i32 + by).clamp(0, theme::ORDER.len() as i32 - 1) as usize,
        _ => return false,
    }
    l.cues = true;
    true
}
