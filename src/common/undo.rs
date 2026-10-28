// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Undo toast can still take back: every archive, delete, restore or import of
//! the last few seconds, newest on top, each with its own time left. The toast shows the
//! newest; Undo takes back the newest, and the next Undo the one before it, while that one
//! is still in time. A newer action never cuts an older one's time short: each becomes
//! final (a deleted note gone for good) only when its own ten seconds have run out.
//!
//! The last one whose time ran out isn't final yet, though: with the toast gone, Undo (the
//! tray menu's, Ctrl+Z in All Notes) can still take it back, until another action comes
//! along or another's time runs out, and it becomes final then.

/// How long an action can be taken back, in seconds of the toast's clock (which waits
/// while the pointer is on the toast).
pub const SECONDS: f32 = 10.0;

/// An action the toast can take back.
pub trait Action: Sized {
    /// `newer`, done right after this one, joined to it when they're one of a kind (two
    /// deletes in a row read "2 notes deleted"), or handed back to stand on its own.
    fn merge(&mut self, newer: Self) -> Option<Self>;

    /// Whether Undo can still take it back once its time has run out (and the toast has
    /// gone). An import's notes may have been worked on since: taking them out so late
    /// would lose that, so it doesn't.
    fn lingers(&self) -> bool {
        true
    }
}

/// One action and the seconds it has left.
pub struct Entry<A> {
    pub action: A,
    pub left: f32,
}

/// The actions still in time, oldest first, and below them the last one whose time ran out.
pub struct Stack<A> {
    entries: Vec<Entry<A>>,
    /// The last action whose time ran out, which Undo can still take back (it's older than
    /// every entry).
    last: Option<A>,
}

impl<A: Action> Default for Stack<A> {
    fn default() -> Self {
        Stack { entries: Vec::new(), last: None }
    }
}

impl<A: Action> Stack<A> {
    /// `a` was just done: it goes on top, or into the action on top when they're one of a
    /// kind, which then has its full time again. The action whose time had run out, if
    /// any, can't be taken back any more: it's handed back, to be made final.
    pub fn push(&mut self, a: A) -> Option<A> {
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
        self.last.take()
    }

    /// The newest action still in time: what the toast shows.
    pub fn top(&self) -> Option<&Entry<A>> {
        self.entries.last()
    }

    /// What Undo takes back next: the newest action still in time, or else the last one
    /// whose time ran out.
    pub fn next(&self) -> Option<&A> {
        self.entries.last().map(|e| &e.action).or(self.last.as_ref())
    }

    /// Take the newest action off, to undo it: one still in time, or else the last one
    /// whose time ran out.
    pub fn pop(&mut self) -> Option<A> {
        self.entries.pop().map(|e| e.action).or_else(|| self.last.take())
    }

    /// The toast's clock ran `dt` seconds: the actions whose time is up leave, oldest
    /// first, to be made final; but the newest of them that lingers stays for Undo, in
    /// place of the one that did before (which is made final with them).
    pub fn tick(&mut self, dt: f32) -> Vec<A> {
        for e in &mut self.entries {
            e.left -= dt;
        }
        let (due, kept): (Vec<Entry<A>>, Vec<Entry<A>>) = std::mem::take(&mut self.entries).into_iter().partition(|e| e.left <= 0.0);
        self.entries = kept;
        if due.is_empty() {
            return Vec::new();
        }
        let mut due: Vec<A> = due.into_iter().map(|e| e.action).collect();
        let stays = due.iter().rposition(A::lingers).map(|i| due.remove(i));
        if stays.is_some() {
            due.splice(0..0, self.last.take());
            self.last = stays;
        }
        due
    }

    /// Every action, oldest first, all made final now (quitting).
    pub fn drain(&mut self) -> Vec<A> {
        self.last.take().into_iter().chain(self.entries.drain(..).map(|e| e.action)).collect()
    }

    /// How many actions are still in time (the toast's).
    pub fn in_time(&self) -> usize {
        self.entries.len()
    }

    /// Whether Undo has nothing to take back.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.last.is_none()
    }

    /// Every action Undo can still take back, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &A> {
        self.last.iter().chain(self.entries.iter().map(|e| &e.action))
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

        fn lingers(&self) -> bool {
            !matches!(self, Toy::Import(_))
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
    /// the newer ones stay in time. The last to run out waits for Undo below them, until the
    /// next one runs out.
    #[test]
    fn each_action_has_its_own_time() {
        let mut s = Stack::default();
        s.push(Toy::Delete(vec![1]));
        s.tick(6.0);
        s.push(Toy::Archive(vec![2]));
        s.tick(3.0);
        s.push(Toy::Delete(vec![3]));
        assert!(s.tick(0.9).is_empty());
        // The first delete's ten seconds are up: it waits under the archive and the second
        // delete, still in time.
        assert!(s.tick(0.2).is_empty());
        assert_eq!(s.top().map(|e| &e.action), Some(&Toy::Delete(vec![3])));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![&Toy::Delete(vec![1]), &Toy::Archive(vec![2]), &Toy::Delete(vec![3])]);
        assert!(s.tick(5.0).is_empty());
        // The archive's are up: it takes the delete's place, which is final now.
        assert_eq!(s.tick(1.0), vec![Toy::Delete(vec![1])]);
        assert_eq!(s.tick(3.0), vec![Toy::Archive(vec![2])]);
        assert!(s.top().is_none() && !s.is_empty());
        // Two whose time runs out in the same tick: the newer waits, the older is final, and
        // so is the one that waited before them (taken by the first push).
        assert_eq!(s.push(Toy::Delete(vec![4])), Some(Toy::Delete(vec![3])));
        s.push(Toy::Archive(vec![5]));
        assert_eq!(s.tick(SECONDS), vec![Toy::Delete(vec![4])]);
        assert_eq!(s.drain(), vec![Toy::Archive(vec![5])]);
        assert!(s.is_empty());
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
        assert!(s.tick(SECONDS).is_empty());
        assert_eq!(s.pop(), Some(Toy::Delete(vec![1, 3])));
    }

    /// The live test's case: an archive whose toast has gone (its time ran out) is still
    /// taken back by Undo, until another action comes along; then that one is.
    #[test]
    fn undo_reaches_the_last_action_after_its_toast() {
        let mut s = Stack::default();
        s.push(Toy::Archive(vec![7]));
        assert!(s.tick(SECONDS + 1.0).is_empty(), "not final yet");
        assert!(s.top().is_none(), "the toast has nothing to show");
        assert_eq!(s.next(), Some(&Toy::Archive(vec![7])));
        assert_eq!(s.pop(), Some(Toy::Archive(vec![7])));
        assert!(s.is_empty() && s.pop().is_none());
        // Another action comes along: the one that waited is final then, and Undo takes the
        // new one.
        s.push(Toy::Delete(vec![8]));
        s.tick(SECONDS);
        assert_eq!(s.push(Toy::Archive(vec![9])), Some(Toy::Delete(vec![8])));
        assert_eq!(s.pop(), Some(Toy::Archive(vec![9])));
        assert!(s.pop().is_none());
        // An import doesn't wait: its notes may have been worked on since.
        s.push(Toy::Delete(vec![10]));
        s.tick(SECONDS);
        s.push(Toy::Import(vec![11]));
        assert_eq!(s.tick(SECONDS), vec![Toy::Import(vec![11])]);
        assert!(s.is_empty());
    }
}
