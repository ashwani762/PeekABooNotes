// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What each Settings page contains.

use super::*;

/// Which display the deck is on: only worth asking with more than one, or while the
/// one chosen isn't connected, which then says why the deck is on the main display.
/// The displays are the list kept between display changes, not looked for again on
/// every rebuild.
fn display_row() -> Option<Row> {
    let hidden = app::store(|s| s.d.settings.deck_hidden);
    display_choice().map(|r| r.disabled_if(hidden))
}

fn display_choice() -> Option<Row> {
    if display_away() {
        return Some(row_sub(Key::DeckDisplay, t!("settings_deck_display"), t!("settings_deck_display_away"), Ctl::Choice));
    }
    (crate::win::ui::displays::list().len() > 1).then(|| row(Key::DeckDisplay, t!("settings_deck_display"), Ctl::Choice))
}

/// The Deck page's rows, with the deck's display after the screen edge when it's asked.
fn with_display_row(mut rows: Vec<Row>) -> Vec<Row> {
    if let Some(r) = display_row() {
        let at = rows.iter().position(|r| r.key == Some(Key::Edge)).map_or(rows.len(), |i| i + 1);
        rows.insert(at, r);
    }
    rows
}

/// Page `i`'s rows; `edit` is what the shortcuts' rows have to say.
pub(super) fn page(i: usize, edit: &Edit) -> Vec<Group> {
    match i {
        0 => vec![
            Group { header: None, rows: vec![row(Key::Startup, t!("settings_start_with_windows"), Ctl::Toggle)], footer: None },
            Group {
                header: Some(t!("settings_group_appearance")),
                rows: vec![row(Key::Theme, t!("settings_theme"), Ctl::Seg(vec![t!("theme_system"), t!("theme_light"), t!("theme_dark")]))],
                footer: None,
            },
            Group {
                header: Some(t!("settings_group_motion")),
                rows: vec![row_sub(Key::ReduceMotion, t!("settings_reduce_motion"), t!("settings_reduce_motion_sub"), Ctl::Toggle)],
                footer: Some(t!("settings_motion_footer", t!("app_name"))),
            },
        ],
        1 => vec![
            Group {
                header: None,
                rows: vec![
                    row(Key::NoteFont, t!("settings_font"), Ctl::Seg(vec![t!("font_system"), t!("font_serif"), t!("font_mono"), t!("font_hand")])),
                    row(Key::NoteSize, t!("settings_text_size"), Ctl::Seg(vec![t!("size_small"), t!("size_medium"), t!("size_large")])),
                    row_sub(Key::NewColor, t!("settings_new_color"), "", Ctl::Swatches),
                ],
                footer: Some(t!("settings_color_footer").into()),
            },
            Group {
                header: None,
                rows: vec![row_sub(Key::HideUnpinned, t!("settings_hide_unpinned"), t!("settings_hide_unpinned_sub"), Ctl::Toggle)],
                footer: None,
            },
            Group { header: Some(t!("settings_group_preview")), rows: vec![Row { key: None, label: "", sub: None, ctl: Ctl::Preview, off: false, hint: None, disabled: false }], footer: None },
        ],
        2 => {
            // With the deck hidden, where and how it shows does nothing: greyed out. Where new
            // notes go still counts in All Notes.
            let hidden = app::store(|s| s.d.settings.deck_hidden);
            vec![
                Group {
                    header: None,
                    rows: with_display_row(vec![
                        row_sub(Key::ShowDeck, t!("settings_show_deck"), t!("settings_show_deck_sub"), Ctl::Toggle),
                        row(Key::Edge, t!("settings_screen_edge"), Ctl::Seg(vec![t!("edge_left"), t!("edge_right")])).disabled_if(hidden),
                        row_sub(Key::Cards, t!("settings_cards"), t!("settings_cards_sub"), Ctl::Seg(vec!["5", "8", "12"])).disabled_if(hidden),
                        row_sub(Key::NewAt, t!("settings_new_notes"), t!("settings_new_notes_sub"), Ctl::Seg(vec![t!("new_notes_top"), t!("new_notes_bottom")])),
                    ]),
                    footer: None,
                },
                Group {
                    header: None,
                    rows: vec![row_sub(Key::Fullscreen, t!("settings_fullscreen"), t!("settings_fullscreen_sub"), Ctl::Toggle).disabled_if(hidden)],
                    footer: Some(t!("settings_deck_footer").into()),
                },
            ]
        }
        3 => vec![
            Group {
                header: Some(t!("settings_group_anywhere")),
                // Each one's keys change with a click; the defaults are a click away.
                rows: (0..hotkey::COUNT)
                    .map(|i| hotkey(i, edit))
                    .chain(std::iter::once(row(Key::ResetKeys, t!("settings_shortcuts_default"), Ctl::Button(t!("button_reset_defaults")))))
                    .collect(),
                footer: None,
            },
            Group {
                header: Some(t!("settings_group_in_note")),
                rows: vec![
                    keys(t!("shortcut_new_note"), &["ctrl", "N"]),
                    keys(t!("tip_close"), &["esc", "/", "ctrl", "W"]),
                    keys(t!("shortcut_format"), &["ctrl", "B", "/", "I", "/", "U"]),
                    keys(t!("shortcut_strike"), &["ctrl", "shift", "X"]),
                    keys(t!("shortcut_lists"), &["ctrl", "shift", "8", "/", "7"]),
                    keys(t!("shortcut_checklist"), &["ctrl", "shift", "L"]),
                    keys(t!("shortcut_align"), &["ctrl", "L", "/", "E", "/", "R"]),
                    keys(t!("shortcut_quote"), &["ctrl", "'"]),
                    keys(t!("shortcut_code"), &["ctrl", "shift", "M"]),
                    keys(t!("shortcut_table"), &["ctrl", "shift", "T"]),
                    keys(t!("shortcut_tick"), &["ctrl", "enter"]),
                    keys(t!("shortcut_nest"), &["tab", "/", "shift", "tab"]),
                    keys(t!("shortcut_link"), &["ctrl", "K"]),
                    keys(t!("shortcut_next_color"), &["ctrl", "."]),
                    keys(t!("shortcut_keep_on_top"), &["ctrl", "shift", "P"]),
                    keys(t!("action_archive"), &["ctrl", "shift", "A"]),
                    keys(t!("menu_delete"), &["ctrl", "shift", "del"]),
                    keys(t!("shortcut_prev_next"), &["ctrl", "[", "/", "]"]),
                ],
                footer: None,
            },
            Group {
                header: Some(t!("menu_all_notes")),
                rows: vec![
                    keys(t!("shortcut_search"), &["ctrl", "F"]),
                    keys(t!("action_archive"), &["ctrl", "shift", "A"]),
                    keys(t!("menu_delete"), &["del", "/", "ctrl", "shift", "del"]),
                    keys(t!("shortcut_undo_toast"), &["ctrl", "Z"]),
                    keys(t!("shortcut_leave_text"), &["ctrl", "tab"]),
                ],
                footer: None,
            },
            // Once Go to the deck's shortcut has given the deck the keys.
            Group {
                header: Some(t!("settings_page_deck")),
                rows: vec![
                    keys(t!("shortcut_deck_move"), &["\u{2191}", "/", "\u{2193}"]),
                    keys(t!("shortcut_deck_ends"), &["home", "/", "end"]),
                    keys(t!("shortcut_deck_letter"), &["letter"]),
                    keys(t!("menu_open"), &["enter"]),
                    keys(t!("shortcut_deck_menu"), &["shift", "F10"]),
                    keys(t!("shortcut_deck_leave"), &["esc"]),
                ],
                footer: None,
            },
        ],
        _ => {
            let (n, archived) = app::store(|s| (s.d.notes.iter().filter(|n| !n.archived).count(), s.d.notes.iter().filter(|n| n.archived).count()));
            // A portable copy says so: its notes aren't in Documents, where people look.
            let path = data_dir().to_string_lossy().into_owned();
            let counts = t!("settings_data_footer", tn!("notes_count", n), tn!("data_archived", archived));
            // The folder row already says when the copy is portable.
            let (folder_label, footer) = if crate::win::util::portable() { (t!("settings_notes_folder_portable"), counts) } else { (t!("settings_notes_folder"), counts) };
            vec![
                Group {
                    header: None,
                    rows: vec![
                        row_sub(Key::Folder, folder_label, &path, Ctl::Button(t!("button_open"))),
                        row_sub(Key::Import, t!("settings_import"), t!("settings_import_sub"), Ctl::Button(t!("button_import"))),
                        row_sub(Key::Export, t!("settings_export_all"), t!("settings_export_sub"), Ctl::Button(t!("button_export"))),
                    ],
                    footer: Some(footer),
                },
                Group {
                    header: Some(t!("settings_group_assistants")),
                    rows: vec![
                        row_sub(Key::Mcp, t!("settings_mcp"), t!("settings_mcp_sub"), Ctl::Toggle),
                        // Nothing to connect to while assistants aren't let in.
                        row_sub(Key::McpSetup, t!("settings_mcp_setup"), t!("settings_mcp_setup_sub"), Ctl::Button(t!("button_connect"))).disabled_if(!app::store(|s| s.d.settings.mcp)),
                    ],
                    footer: Some(t!("settings_mcp_footer").into()),
                },
                // Destructive, so on its own, below everything else people use.
                Group {
                    header: None,
                    rows: vec![row_sub(Key::DeleteAll, t!("settings_delete_all"), t!("settings_delete_all_sub"), Ctl::Button(t!("button_delete_all")))],
                    footer: None,
                },
                // Last, below Delete All: what's still being tested (languages other than English,
                // while they're read through). The Language row only shows with developer mode on.
                Group {
                    header: Some(t!("settings_group_developer")),
                    rows: std::iter::once(row_sub(Key::Developer, t!("settings_developer"), t!("settings_developer_sub"), Ctl::Toggle))
                        .chain(app::store(|s| s.d.settings.developer).then(|| row(Key::Language, t!("settings_language"), Ctl::Choice)))
                        .collect(),
                    footer: None,
                },
            ]
        }
    }
}
