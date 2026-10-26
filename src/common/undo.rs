// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Undo toast can still take back: every archive, delete, restore or import of
//! the last few seconds, newest on top, each with its own time left. The toast shows the
//! newest; Undo takes back the newest, and the next Undo the one before it, while that one
//! is still in time. A newer action never cuts an older one's time short: each becomes
//! final (a deleted note gone for good) only when its own ten seconds have run out.

/// How long an action can be taken back, in seconds of the toast's clock (which waits
/// while the pointer is on the toast).
pub const SECONDS: f32 = 10.0;

/// An action the toast can take back.
pub trait Action: Sized {
    /// `newer`, done right after this one, joined to it when they're one of a kind (two
    /// deletes in a row read "2 notes deleted"), or handed back to stand on its own.
    fn merge(&mut self, newer: Self) -> Option<Self>;
}

/// One action and the seconds it has left.
pub struct Entry<A> {
    pub action: A,
    pub left: f32,
}

/// The actions still in time, oldest first.
pub struct Stack<A> {
    entries: Vec<Entry<A>>,
}

impl<A: Action> Default for Stack<A> {
    fn default() -> Self {
        Stack { entries: Vec::new() }
    }
}

impl<A: Action> Stack<A> {
    /// `a` was just done: it goes on top, or into the action on top when they're one of a
    /// kind, which then has its full time again.
    pub fn push(&mut self, a: A) {
        let rest = match self.entries.last_mut() {
            Some(top) => top.action.merge(a),
            None => Some(a),
        };
        match rest {
            Some(a) => self.entries.push(Entry { action: a, left: SECONDS }),
            None => {
                if let Some(top) = self.entries.last_mut() {
                    top.left = SECONDS;
                }
            }
        }
    }

    /// The newest action: what the toast shows and Undo takes back.
    pub fn top(&self) -> Option<&Entry<A>> {
        self.entries.last()
    }

    /// Take the newest action off, to undo it.
    pub fn pop(&mut self) -> Option<A> {
        self.entries.pop().map(|e| e.action)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy action: deletes merge, archives merge, imports never do.
    #[derive(Debug, PartialEq)]
    enum Toy {
        Delete(Vec<u32>),
        Archive(Vec<u32>),
        Import(Vec<u32>),
    }

    impl Action for Toy {
        fn merge(&mut self, newer: Toy) -> Option<Toy> {
            match (self, newer) {
                (Toy::Delete(a), Toy::Delete(b)) | (Toy::Archive(a), Toy::Archive(b)) => {
                    a.extend(b);
                    None
                }
                (_, n) => Some(n),
            }
        }
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

    /// One of a kind in a row is one action ("2 notes archived"), with its full time again;
    /// another kind in between keeps them apart.
    #[test]
    fn alike_actions_in_a_row_merge() {
        let mut s = Stack::default();
        s.push(Toy::Archive(vec![1]));
        s.tick(4.0);
        s.push(Toy::Archive(vec![2]));
        assert_eq!(s.top().map(|e| (&e.action, e.left)), Some((&Toy::Archive(vec![1, 2]), SECONDS)));
        s.push(Toy::Delete(vec![3]));
        s.push(Toy::Archive(vec![4]));
        s.push(Toy::Import(vec![5]));
        s.push(Toy::Import(vec![6]));
        assert_eq!(s.drain(), vec![Toy::Archive(vec![1, 2]), Toy::Delete(vec![3]), Toy::Archive(vec![4]), Toy::Import(vec![5]), Toy::Import(vec![6])]);
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
    /// its own time; merging into it after that gives it its full time again.
    #[test]
    fn undo_goes_down_the_stack() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![1]));
        s.tick(2.0);
        s.push(Toy::Archive(vec![2]));
        s.tick(2.0);
        s.pop();
        s.push(Toy::Delete(vec![3]));
        assert_eq!(s.top().map(|e| (&e.action, e.left)), Some((&Toy::Delete(vec![1, 3]), SECONDS)));
        assert_eq!(s.tick(SECONDS), vec![Toy::Delete(vec![1, 3])]);
    }
}
