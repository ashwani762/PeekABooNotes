//! Applying settings: appearance, motion and note typography.

use super::*;

pub fn update_settings(f: impl FnOnce(&mut crate::store::Settings)) {
    store_mut(|s| f(&mut s.d.settings));
    save();
    deck::refresh();
    settings::refresh();
}

/// The appearance in effect: the Settings choice, or Windows' own.
pub(super) fn wanted_dark() -> bool {
    match store(|s| s.d.settings.theme) {
        1 => false,
        2 => true,
        _ => theme::system_is_dark(),
    }
}

/// Note typeface and size from Settings, applied to every open editor.
pub fn apply_typography() {
    let (font, size) = store(|s| (s.d.settings.note_font, s.d.settings.note_size));
    rich::set_typography(font, size);
    let wins: Vec<HWND> = ui(|u| u.notes.values().copied().collect());
    for h in wins {
        note::retheme(h);
    }
    allnotes::retheme_editor();
}

/// Switch the interface language and redraw everything that shows text.
pub fn apply_language() {
    crate::i18n::init(&store(|s| s.d.settings.language.clone()));
    allnotes::relabel();
    settings::relabel();
    deck::refresh();
    tray_relabel();
}

/// Reduce motion when Windows' animation effects are off or Settings asks for it.
pub fn apply_motion() {
    let reduce = store(|s| s.d.settings.reduce_motion);
    crate::render::anim::set_reduced_motion(reduce || !theme::animations_enabled());
}

pub fn apply_theme() {
    let d = wanted_dark();
    if d == dark() {
        return;
    }
    ui(|u| u.dark = d);
    let wins: Vec<HWND> = ui(|u| u.notes.values().copied().collect());
    for h in wins {
        note::retheme(h);
    }
    allnotes::retheme();
    settings::retheme();
    deck::refresh();
}
