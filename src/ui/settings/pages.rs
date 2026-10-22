//! What each Settings page contains.

use super::*;

pub(super) fn page(i: usize) -> Vec<Group> {
    match i {
        0 => vec![
            Group { header: None, rows: vec![row(Key::Startup, t!("settings_start_with_windows"), Ctl::Toggle)], footer: None },
            Group {
                header: Some(t!("settings_group_appearance")),
                rows: vec![
                    row(Key::Theme, t!("settings_theme"), Ctl::Seg(vec![t!("theme_system"), t!("theme_light"), t!("theme_dark")])),
                    row(Key::Language, t!("settings_language"), Ctl::Choice),
                ],
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
            Group { header: Some(t!("settings_group_preview")), rows: vec![Row { key: None, label: "", sub: None, ctl: Ctl::Preview }], footer: None },
        ],
        2 => vec![
            Group {
                header: None,
                rows: vec![
                    row_sub(Key::ShowDeck, t!("settings_show_deck"), t!("settings_show_deck_sub"), Ctl::Toggle),
                    row(Key::Edge, t!("settings_screen_edge"), Ctl::Seg(vec![t!("edge_left"), t!("edge_right")])),
                    row_sub(Key::Cards, t!("settings_cards"), t!("settings_cards_sub"), Ctl::Seg(vec!["5", "8", "12"])),
                    row_sub(Key::NewAt, t!("settings_new_notes"), t!("settings_new_notes_sub"), Ctl::Seg(vec![t!("new_notes_top"), t!("new_notes_bottom")])),
                ],
                footer: None,
            },
            Group {
                header: None,
                rows: vec![row_sub(Key::Fullscreen, t!("settings_fullscreen"), t!("settings_fullscreen_sub"), Ctl::Toggle)],
                footer: Some(t!("settings_deck_footer").into()),
            },
        ],
        3 => vec![
            Group {
                header: Some(t!("settings_group_anywhere")),
                rows: vec![
                    keys(t!("shortcut_new_note"), &["ctrl", "alt", "N"]),
                    keys(t!("shortcut_all_notes"), &["ctrl", "alt", "A"]),
                    keys(t!("shortcut_archive"), &["ctrl", "alt", "L"]),
                    keys(t!("shortcut_reopen"), &["ctrl", "alt", "O"]),
                    keys(t!("shortcut_toggle_deck"), &["ctrl", "alt", "H"]),
                    keys(t!("shortcut_move_deck"), &["ctrl", "alt", "E"]),
                ],
                footer: None,
            },
            Group {
                header: Some(t!("settings_group_in_note")),
                rows: vec![
                    keys(t!("shortcut_checklist"), &["ctrl", "shift", "L"]),
                    keys(t!("shortcut_tick"), &["ctrl", "enter"]),
                    keys(t!("shortcut_nest"), &["tab", "/", "shift", "tab"]),
                    keys(t!("shortcut_prev_next"), &["ctrl", "[", "/", "]"]),
                    keys(t!("shortcut_next_color"), &["ctrl", "."]),
                ],
                footer: None,
            },
        ],
        _ => {
            let (n, archived) = app::store(|s| (s.d.notes.iter().filter(|n| !n.archived).count(), s.d.notes.iter().filter(|n| n.archived).count()));
            vec![Group {
                header: None,
                rows: vec![
                    row_sub(Key::Folder, t!("settings_notes_folder"), &data_dir().to_string_lossy(), Ctl::Button(t!("button_open"))),
                    row_sub(Key::Import, t!("settings_import"), t!("settings_import_sub"), Ctl::Button(t!("button_import"))),
                    row_sub(Key::Export, t!("settings_export_all"), t!("settings_export_sub"), Ctl::Button(t!("button_export"))),
                ],
                footer: Some(t!("settings_data_footer", tn!("notes_count", n), tn!("data_archived", archived))),
            }]
        }
    }
}
