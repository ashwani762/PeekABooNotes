// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The deck from the keyboard: Ctrl+Alt+K gives it the keys, the arrows move a focus ring
//! down the cards to the "+N" tile and the plus button, Enter opens, Shift+F10 or the Menu
//! key opens the card's menu, Esc hands the keys back. Here is where focus goes; the deck
//! does the rest.

use crate::win::ui::focus;

/// Something in the deck the focus ring can be on. A card is held by its note, not its
/// place, so the ring stays on it when the deck is rebuilt.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Stop {
    Card(u64),
    More,
    Plus,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Move {
    Up,
    Down,
    Home,
    End,
    /// Tab and Shift+Tab.
    Next,
    Prev,
}

/// Where `key` takes the focus from `at` along `order` (the cards top to bottom, then the
/// "+N" tile and the plus button). The arrows stop at the ends, as in All Notes' list;
/// Tab comes round again, as it does in every window. With no focus yet, Down and Tab
/// start at the top, Up and Shift+Tab at the bottom.
pub(super) fn step(order: &[Stop], at: Option<Stop>, key: Move) -> Option<Stop> {
    match key {
        Move::Home => order.first().copied(),
        Move::End => order.last().copied(),
        Move::Up | Move::Down => focus::step(order, at, key == Move::Up, false, |_| true),
        Move::Next | Move::Prev => focus::step(order, at, key == Move::Prev, true, |_| true),
    }
}

/// The deck was rebuilt from `before` to `after` (a note opened, was archived or left the
/// deck): if the focused card has gone, the ring moves to the card that took its place,
/// or else the one above it, so it never jumps to the far end of the deck.
pub(super) fn refocus(before: &[Stop], after: &[Stop], at: Option<Stop>) -> Option<Stop> {
    let at = at?;
    if after.contains(&at) {
        return Some(at);
    }
    let card = |s: &&Stop| matches!(s, Stop::Card(_)) && after.contains(s);
    if let Some(i) = before.iter().position(|&s| s == at)
        && let Some(s) = before[i + 1..].iter().find(card).or_else(|| before[..i].iter().rev().find(card))
    {
        return Some(*s);
    }
    after.iter().find(|s| matches!(s, Stop::Card(_))).or(after.first()).copied()
}

/// A typed letter: the next card after the focused one whose title starts with it, coming
/// round from the top, so pressing it again goes on to the next such card. Menus jump to
/// their rows the same way.
pub(super) fn by_letter(cards: &[(Stop, &str)], at: Option<Stop>, ch: char) -> Option<Stop> {
    let lower = |c: char| c.to_lowercase().collect::<String>();
    let want = lower(ch);
    if want.trim().is_empty() {
        return None;
    }
    let n = cards.len();
    let from = at.and_then(|a| cards.iter().position(|(s, _)| *s == a)).map_or(0, |i| i + 1);
    (0..n).map(|k| &cards[(from + k) % n]).find(|(_, title)| title.trim_start().chars().next().is_some_and(|c| lower(c) == want)).map(|(s, _)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Stop::{Card, More, Plus};

    const DECK: [Stop; 5] = [Card(1), Card(2), Card(3), More, Plus];

    #[test]
    fn up_and_down_stop_at_the_ends() {
        assert_eq!(step(&DECK, Some(Card(1)), Move::Down), Some(Card(2)));
        assert_eq!(step(&DECK, Some(Card(3)), Move::Down), Some(More));
        assert_eq!(step(&DECK, Some(More), Move::Down), Some(Plus));
        assert_eq!(step(&DECK, Some(Plus), Move::Down), Some(Plus), "no wrap past the plus button");
        assert_eq!(step(&DECK, Some(Card(2)), Move::Up), Some(Card(1)));
        assert_eq!(step(&DECK, Some(Card(1)), Move::Up), Some(Card(1)), "no wrap past the top card");
    }

    #[test]
    fn with_no_focus_down_starts_at_the_top_and_up_at_the_bottom() {
        assert_eq!(step(&DECK, None, Move::Down), Some(Card(1)));
        assert_eq!(step(&DECK, None, Move::Up), Some(Plus));
        assert_eq!(step(&DECK, None, Move::Next), Some(Card(1)));
        assert_eq!(step(&DECK, None, Move::Prev), Some(Plus));
    }

    #[test]
    fn home_and_end() {
        assert_eq!(step(&DECK, Some(Card(2)), Move::Home), Some(Card(1)));
        assert_eq!(step(&DECK, Some(Card(2)), Move::End), Some(Plus));
        assert_eq!(step(&[], None, Move::Home), None);
    }

    #[test]
    fn tab_comes_round() {
        assert_eq!(step(&DECK, Some(Plus), Move::Next), Some(Card(1)));
        assert_eq!(step(&DECK, Some(Card(1)), Move::Prev), Some(Plus));
        assert_eq!(step(&DECK, Some(Card(2)), Move::Next), Some(Card(3)));
    }

    #[test]
    fn an_empty_deck_still_has_the_plus_button() {
        assert_eq!(step(&[Plus], None, Move::Down), Some(Plus));
        assert_eq!(step(&[Plus], Some(Plus), Move::Up), Some(Plus));
        assert_eq!(step(&[Plus], Some(Plus), Move::Next), Some(Plus));
    }

    #[test]
    fn a_card_that_stays_keeps_the_focus() {
        let after = [Card(2), Card(1), Card(3), Plus];
        assert_eq!(refocus(&DECK, &after, Some(Card(1))), Some(Card(1)));
        assert_eq!(refocus(&DECK, &after, Some(Plus)), Some(Plus));
        assert_eq!(refocus(&DECK, &after, None), None);
    }

    #[test]
    fn a_card_that_goes_hands_the_focus_on() {
        // The middle card is archived: the one below takes its place.
        assert_eq!(refocus(&DECK, &[Card(1), Card(3), More, Plus], Some(Card(2))), Some(Card(3)));
        // The bottom card goes: the one above, never the "+N" tile or the plus button.
        assert_eq!(refocus(&DECK, &[Card(1), Card(2), More, Plus], Some(Card(3))), Some(Card(2)));
        // A new card came in where it was: still a card of the old order first.
        assert_eq!(refocus(&DECK, &[Card(1), Card(9), Card(3), Plus], Some(Card(2))), Some(Card(3)));
        // Every card has gone: the plus button.
        assert_eq!(refocus(&DECK, &[Plus], Some(Card(2))), Some(Plus));
        // Focus on something the old order never had: the top card.
        assert_eq!(refocus(&DECK, &[Card(7), Plus], Some(Card(42))), Some(Card(7)));
    }

    #[test]
    fn letters_jump_to_cards_and_go_round() {
        let cards = [(Card(1), "Shopping"), (Card(2), "plans"), (Card(3), " Studio"), (Card(4), ""), (Card(5), "Éclairs")];
        assert_eq!(by_letter(&cards, None, 's'), Some(Card(1)));
        assert_eq!(by_letter(&cards, Some(Card(1)), 'S'), Some(Card(3)), "case and leading spaces don't matter");
        assert_eq!(by_letter(&cards, Some(Card(3)), 's'), Some(Card(1)), "round from the top");
        assert_eq!(by_letter(&cards, Some(Card(1)), 'p'), Some(Card(2)));
        assert_eq!(by_letter(&cards, Some(Card(2)), 'p'), Some(Card(2)), "the only match stays");
        assert_eq!(by_letter(&cards, None, 'é'), Some(Card(5)));
        assert_eq!(by_letter(&cards, None, 'x'), None);
        assert_eq!(by_letter(&cards, None, ' '), None);
        assert_eq!(by_letter(&[], None, 'a'), None);
    }
}
