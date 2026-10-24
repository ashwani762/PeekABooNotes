// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The note, colour and tray menus, and the app commands they send.

use super::*;

/// The colour row on its own (the note header's colour button).
pub fn color_menu(id: u64) -> Vec<menu::Item> {
    let color = store(|s| s.note(id).map(|n| n.color));
    vec![menu::Item::Swatches { base: NC_COLOR0, current: color }]
}

/// Right-click in a note's text (editor `e`): the formatting row and the editing
/// commands, each greyed out when it has nothing to act on.
pub fn edit_menu(e: HWND) -> Vec<menu::Item> {
    use menu::{Item, Tool, ToolStyle, action};
    let (a, b) = rich::sel(e);
    let selected = a != b;
    vec![
        Item::Tools(vec![
            Tool { id: NC_BOLD, glyph: "B", style: ToolStyle::Bold },
            Tool { id: NC_ITALIC, glyph: "I", style: ToolStyle::Italic },
            Tool { id: NC_UNDERLINE, glyph: "U", style: ToolStyle::Underline },
            Tool { id: NC_STRIKE, glyph: "S", style: ToolStyle::Strike },
            Tool { id: NC_BULLETS, glyph: "\u{2022}", style: ToolStyle::Plain },
            Tool { id: NC_NUMBERS, glyph: "1.", style: ToolStyle::Plain },
            Tool { id: NC_TASK, glyph: "\u{E930}", style: ToolStyle::Icon },
        ]),
        Item::Separator,
        action(NC_CUT, t!("menu_cut"), &crate::common::i18n::keys(&["ctrl", "X"]), IC_CUT).enabled(selected),
        action(NC_COPY, t!("menu_copy"), &crate::common::i18n::keys(&["ctrl", "C"]), IC_COPY).enabled(selected),
        action(NC_PASTE, t!("menu_paste"), &crate::common::i18n::keys(&["ctrl", "V"]), IC_PASTE).enabled(crate::win::util::clipboard_can_paste()),
        action(NC_SELECT_ALL, t!("menu_select_all"), &crate::common::i18n::keys(&["ctrl", "A"]), IC_SELECT),
        Item::Separator,
        action(NC_LINK, t!("menu_link"), &crate::common::i18n::keys(&["ctrl", "K"]), IC_LINK),
    ]
}

/// A note's own menu: its colour, place and fate. From the note window itself (`in_note`:
/// its More button or a right-click on its header) it ends with moving between notes;
/// elsewhere (the deck, All Notes) it starts with Open.
pub fn note_menu(id: u64, in_note: bool) -> Vec<menu::Item> {
    use menu::{Item, action, danger};
    let (archived, pinned, color, off_deck) = store(|s| s.note(id).map(|n| (n.archived, n.pinned, n.color, n.off_deck))).unwrap_or_default();
    let mut v = Vec::new();
    if !in_note {
        v.push(action(NC_OPEN, t!("menu_open"), "", IC_OPEN));
        v.push(Item::Separator);
    }
    v.push(Item::Swatches { base: NC_COLOR0, current: Some(color) });
    v.push(Item::Separator);
    v.push(Item::Action { id: NC_PIN, label: t!("menu_keep_on_top").into(), key: crate::common::i18n::keys(&["ctrl", "shift", "P"]), icon: IC_PIN, checked: pinned, danger: false, disabled: false });
    if !archived {
        v.push(menu::check(NC_DECK, t!("menu_show_in_deck"), "", !off_deck));
    }
    v.push(action(NC_ARCHIVE, if archived { t!("menu_restore") } else { t!("menu_archive") }, &if archived { String::new() } else { crate::common::i18n::keys(&["ctrl", "shift", "A"]) }, IC_ARCHIVE));
    v.push(danger(NC_DELETE, t!("menu_delete"), &crate::common::i18n::keys(&["ctrl", "shift", "del"]), IC_DELETE));
    if in_note {
        v.push(Item::Separator);
        v.push(action(NC_PREV, t!("menu_previous_note"), &crate::common::i18n::keys(&["ctrl", "["]), IC_BACK));
        v.push(action(NC_NEXT, t!("menu_next_note"), &crate::common::i18n::keys(&["ctrl", "]"]), IC_FORWARD));
        v.push(action(NC_NEW, t!("menu_new_note"), &crate::common::i18n::keys(&["ctrl", "N"]), IC_ADD));
    }
    v
}

/// Show a menu at screen point (`x`, `y`); returns the chosen command, 0 if dismissed.
/// Focus goes back to whichever of our windows had it.
pub fn popup(items: Vec<menu::Item>, x: i32, y: i32, owner: HWND) -> u32 {
    let prev = unsafe { GetForegroundWindow() };
    let cmd = menu::show(items, POINT { x, y });
    unsafe {
        let back = if !owner.is_null() && IsWindowVisible(owner) != 0 { owner } else { prev };
        if !back.is_null() && IsWindow(back) != 0 && back != ui(|u| u.main) {
            SetForegroundWindow(back);
        }
    }
    cmd
}

pub(super) fn cursor() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    p
}

/// A note's menu at the mouse; `before` runs once a command is picked, ahead of it.
pub fn note_menu_at_cursor(id: u64, owner: HWND, before: impl FnOnce()) {
    let p = cursor();
    let cmd = popup(note_menu(id, false), p.x, p.y, owner);
    if cmd != 0 {
        before();
        note_command(id, cmd);
    }
}

/// A note's menu opened from the deck: right-aligned or not, dismissed by `leave`.
pub fn note_menu_beside(id: u64, p: POINT, align_right: bool, leave: Box<dyn Fn(POINT) -> bool>) {
    let cmd = menu::show_with(note_menu(id, false), p, align_right, Some(leave));
    if cmd != 0 {
        note_command(id, cmd);
    }
}

pub fn tray_menu() {
    use menu::{Item, action, check};
    let hidden = store(|s| s.d.settings.deck_hidden);
    let has_undo = ui(|u| !u.pending.is_empty());
    let mut v = vec![
        action(CMD_NEW, t!("menu_new_note"), &crate::common::i18n::keys(&["ctrl", "alt", "N"]), IC_ADD),
        action(CMD_ALL, t!("menu_all_notes"), &crate::common::i18n::keys(&["ctrl", "alt", "A"]), IC_LIST),
        action(CMD_ARCHIVE, t!("menu_archive"), &crate::common::i18n::keys(&["ctrl", "alt", "L"]), IC_ARCHIVE),
        action(CMD_REOPEN, t!("menu_reopen_last"), &crate::common::i18n::keys(&["ctrl", "alt", "O"]), IC_REFRESH),
    ];
    if has_undo {
        v.push(action(CMD_UNDO, t!("menu_undo_delete"), "", IC_UNDO));
    }
    v.extend([
        Item::Separator,
        check(CMD_TOGGLE_DECK, t!("menu_show_deck"), &crate::common::i18n::keys(&["ctrl", "alt", "H"]), !hidden),
        action(CMD_SWAP_EDGE, t!("menu_move_deck"), &crate::common::i18n::keys(&["ctrl", "alt", "E"]), IC_SWAP),
        Item::Separator,
        action(CMD_SETTINGS, t!("menu_settings"), "", IC_SETTINGS),
        Item::Separator,
        action(CMD_QUIT, &t!("menu_quit", t!("app_name")), "", IC_POWER),
    ]);
    let p = cursor();
    let cmd = popup(v, p.x, p.y, null_mut());
    command(cmd);
}

pub(super) fn command(cmd: u32) {
    match cmd {
        CMD_NEW => new_note(),
        CMD_ALL => show_all_notes(false),
        CMD_ARCHIVE => show_all_notes(true),
        CMD_REOPEN => reopen_last(),
        CMD_TOGGLE_DECK => update_settings(|s| s.deck_hidden = !s.deck_hidden),
        CMD_SWAP_EDGE => update_settings(|s| s.edge_left = !s.edge_left),
        CMD_UNDO => undo_delete(),
        CMD_QUIT => quit(),
        CMD_SETTINGS => settings::show(),
        _ => {}
    }
}
