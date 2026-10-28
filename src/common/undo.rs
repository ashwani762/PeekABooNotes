// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Undo toast can still take back: every archive, delete, restore or import of
//! the last few seconds, newest on top, each with its own time left. The toast shows them
//! as a stack of cards, the newest on top; Undo takes back the newest, and the next Undo
//! the one before it, while that one is still in time. Dismissing the newest makes it
//! final at once and shows the one before it. A newer action never cuts an older one's
//! time short: each becomes final (a deleted note gone for good) only when its own ten
//! seconds have run out, or it's dismissed.

/// How long an action can be taken back, in seconds of the toast's clock (which waits
/// while the pointer is on the toast).
pub const SECONDS: f32 = 10.0;

/// One action, which card it is on the toast, and the seconds it has left.
pub struct Entry<A> {
    /// Its own for as long as the app runs: the toast follows each card by it.
    pub id: u64,
    pub action: A,
    pub left: f32,
}

/// The actions still in time, oldest first.
pub struct Stack<A> {
    entries: Vec<Entry<A>>,
    next: u64,
}

impl<A> Default for Stack<A> {
    fn default() -> Self {
        Stack { entries: Vec::new(), next: 1 }
    }
}

impl<A> Stack<A> {
    /// `a` was just done: it goes on top, a card of its own with its full time, however
    /// like the one below it is (one action on several notes is one card, "3 notes
    /// deleted"; two deletes are two).
    pub fn push(&mut self, a: A) {
        self.entries.push(Entry { id: self.next, action: a, left: SECONDS });
        self.next += 1;
    }

    /// The newest action: what the toast shows on top and Undo takes back.
    pub fn top(&self) -> Option<&Entry<A>> {
        self.entries.last()
    }

    /// Take the newest action off, to undo it.
    pub fn pop(&mut self) -> Option<A> {
        self.entries.pop().map(|e| e.action)
    }

    /// Take the newest action off to be made final now, as if its time had run out (the
    /// toast's dismiss); the one before it is on top then, with the time it has left.
    pub fn dismiss(&mut self) -> Option<A> {
        self.pop()
    }

    /// The toast's clock ran `dt` seconds: the actions whose time is up leave, oldest
    /// first, to be made final.
    pub fn tick(&mut self, dt: f32) -> Vec<A> {
        for e in &mut self.entries {
            e.left -= dt;
        }
        let (due, kept): (Vec<Entry<A>>, Vec<Entry<A>>) = std::mem::take(&mut self.entries).into_iter().partition(|e| e.left <= 0.0);
        self.entries = kept;
        due.into_iter().map(|e| e.action).collect()
    }

    /// Every action, oldest first, all made final now (quitting).
    pub fn drain(&mut self) -> Vec<A> {
        self.entries.drain(..).map(|e| e.action).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The actions still in time, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &A> {
        self.entries.iter().map(|e| &e.action)
    }

    /// The cards as the toast stacks them: the newest first, then each one before it.
    pub fn newest_first(&self) -> impl Iterator<Item = &Entry<A>> {
        self.entries.iter().rev()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy action.
    #[derive(Debug, PartialEq)]
    enum Toy {
        Delete(Vec<u32>),
        Archive(Vec<u32>),
        Import(Vec<u32>),
    }

    /// The cards the toast would show, newest first: what each is and its time left.
    fn cards(s: &Stack<Toy>) -> Vec<(&Toy, f32)> {
        s.newest_first().map(|e| (&e.action, e.left)).collect()
    }

    /// The live critique's case: delete X, then archive Y within the ten seconds. Undo
    /// takes back the archive, the next Undo the delete; nothing became final on the way.
    #[test]
    fn a_newer_action_never_takes_an_older_one_away() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![138]));
        assert!(s.tick(3.0).is_empty());
        s.push(Toy::Archive(vec![143]));
        assert!(s.tick(2.0).is_empty());
        assert_eq!(s.pop(), Some(Toy::Archive(vec![143])));
        // The delete shows again, with the time it had left.
        assert_eq!(s.top().map(|e| e.left), Some(SECONDS - 5.0));
        assert_eq!(s.pop(), Some(Toy::Delete(vec![138])));
        assert!(s.pop().is_none() && s.is_empty());
    }

    /// Each action is a card of its own, one of a kind in a row too, each with its own
    /// time; one action on several notes stays one card.
    #[test]
    fn every_action_is_a_card_of_its_own() {
        let mut s = Stack::default();
        s.push(Toy::Archive(vec![1]));
        s.tick(4.0);
        s.push(Toy::Archive(vec![2]));
        assert_eq!(cards(&s), vec![(&Toy::Archive(vec![2]), SECONDS), (&Toy::Archive(vec![1]), SECONDS - 4.0)]);
        s.push(Toy::Delete(vec![3, 4, 5]));
        s.push(Toy::Delete(vec![6]));
        s.push(Toy::Import(vec![7]));
        s.push(Toy::Import(vec![8]));
        assert_eq!(
            s.drain(),
            vec![Toy::Archive(vec![1]), Toy::Archive(vec![2]), Toy::Delete(vec![3, 4, 5]), Toy::Delete(vec![6]), Toy::Import(vec![7]), Toy::Import(vec![8])]
        );
    }

    /// The toast stacks the cards newest first, each known by an id of its own that stays
    /// with it as the ones above it go, and is never given to another.
    #[test]
    fn the_stack_is_newest_first_and_each_card_keeps_its_id() {
        let mut s = Stack::default();
        for k in 1..=4 {
            s.push(Toy::Delete(vec![k]));
            s.tick(1.0);
        }
        let ids: Vec<u64> = s.newest_first().map(|e| e.id).collect();
        assert_eq!(cards(&s).iter().map(|c| c.0).collect::<Vec<_>>(), vec![&Toy::Delete(vec![4]), &Toy::Delete(vec![3]), &Toy::Delete(vec![2]), &Toy::Delete(vec![1])]);
        assert!(ids.windows(2).all(|w| w[0] > w[1]), "{ids:?}");
        // Undo takes the top; the rest keep their ids and their places under it.
        s.pop();
        assert_eq!(s.newest_first().map(|e| e.id).collect::<Vec<_>>(), ids[1..].to_vec());
        // A new action after that is a new card, not the one undone again.
        s.push(Toy::Archive(vec![9]));
        assert!(!ids.contains(&s.top().unwrap().id));
        // The oldest has the least time left, and goes first.
        assert_eq!(s.newest_first().map(|e| e.left).collect::<Vec<_>>(), vec![SECONDS, SECONDS - 2.0, SECONDS - 3.0, SECONDS - 4.0]);
        assert_eq!(s.tick(SECONDS - 4.0), vec![Toy::Delete(vec![1])]);
    }

    /// Dismissing the top makes it final now and shows the one below it, with the time
    /// it has left; dismissing them all leaves nothing to undo.
    #[test]
    fn dismiss_makes_the_top_final_and_shows_the_one_below() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![1]));
        s.tick(3.0);
        s.push(Toy::Archive(vec![2]));
        s.tick(1.0);
        s.push(Toy::Delete(vec![3]));
        assert_eq!(s.dismiss(), Some(Toy::Delete(vec![3])));
        assert_eq!(cards(&s), vec![(&Toy::Archive(vec![2]), SECONDS - 1.0), (&Toy::Delete(vec![1]), SECONDS - 4.0)]);
        // Nothing else became final on the way.
        assert!(s.tick(0.5).is_empty());
        assert_eq!(s.dismiss(), Some(Toy::Archive(vec![2])));
        assert_eq!(s.dismiss(), Some(Toy::Delete(vec![1])));
        assert!(s.dismiss().is_none() && s.is_empty() && s.top().is_none());
    }

    /// Each action becomes final when its own time runs out, oldest first, and not before;
    /// the newer ones stay in time.
    #[test]
    fn each_action_has_its_own_time() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![1]));
        s.tick(6.0);
        s.push(Toy::Archive(vec![2]));
        s.tick(3.0);
        s.push(Toy::Delete(vec![3]));
        assert!(s.tick(0.9).is_empty());
        // The first delete's ten seconds are up; the archive and the second delete wait.
        assert_eq!(s.tick(0.2), vec![Toy::Delete(vec![1])]);
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![&Toy::Archive(vec![2]), &Toy::Delete(vec![3])]);
        assert!(s.tick(5.0).is_empty());
        assert_eq!(s.tick(1.0), vec![Toy::Archive(vec![2])]);
        assert_eq!(s.tick(3.0), vec![Toy::Delete(vec![3])]);
        assert!(s.is_empty());
        // Two whose time runs out in the same tick go together, oldest first.
        s.push(Toy::Delete(vec![4]));
        s.push(Toy::Archive(vec![5]));
        assert_eq!(s.tick(SECONDS), vec![Toy::Delete(vec![4]), Toy::Archive(vec![5])]);
    }

    /// Undoing the newest leaves the one below on top, still to be undone or made final in
    /// its own time; an action after that goes on top of it as a card of its own.
    #[test]
    fn undo_goes_down_the_stack() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![1]));
        s.tick(2.0);
        s.push(Toy::Archive(vec![2]));
        s.tick(2.0);
        s.pop();
        s.push(Toy::Delete(vec![3]));
        assert_eq!(cards(&s), vec![(&Toy::Delete(vec![3]), SECONDS), (&Toy::Delete(vec![1]), SECONDS - 4.0)]);
        assert_eq!(s.tick(SECONDS - 4.0), vec![Toy::Delete(vec![1])]);
        assert_eq!(s.tick(4.0), vec![Toy::Delete(vec![3])]);
    }
}
