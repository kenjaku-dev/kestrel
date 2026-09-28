//! Navigation history: back / forward stacks over directory changes.
//!
//! §4.11 binds `Alt+Left` and `Alt+Right` to back and forward. That needs two
//! stacks and one invariant that is easy to get wrong: **forward must be
//! discarded the moment you navigate somewhere new**, or `Back` then `Forward`
//! then a click sends you somewhere you never were.
//!
//! This is a pure data structure — no I/O, no egui, no engine types — so the
//! invariant is testable directly, which matters more here than usual: a
//! history bug is a "the file manager teleports me somewhere random" bug, and
//! that is exactly the class of bug that is painful to reproduce by hand.

use std::path::{Path, PathBuf};

/// The most entries one stack will hold.
///
/// Not a spec value: §4.11 says nothing about history depth. 100 is a pragmatic
/// ceiling — deep enough that nobody reaches it in a session, small enough that
/// the cap is invisible. The cap exists because a file manager session is
/// long-lived and an unbounded `Vec<PathBuf>` is a slow leak on a machine
/// browsing a deep tree.
pub const MAX_DEPTH: usize = 100;

/// A back/forward pair of stacks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    /// Where we have been. Never empty while the app is running: index 0 is
    /// always the entry point, so `Back` at the start is a no-op rather than an
    /// underflow.
    past: Vec<PathBuf>,
    /// Where we could go. Cleared on every new navigation.
    future: Vec<PathBuf>,
    /// Index into `past` of the current directory.
    current: usize,
    /// The entry point, kept so a capped history can still reset.
    start: Option<PathBuf>,
}

impl History {
    /// A history positioned at `start`.
    #[must_use]
    pub fn new(start: &Path) -> Self {
        Self {
            past: vec![start.to_path_buf()],
            future: Vec::new(),
            current: 0,
            start: Some(start.to_path_buf()),
        }
    }

    /// The directory currently shown.
    #[must_use]
    pub fn current(&self) -> Option<&Path> {
        self.past.get(self.current).map(PathBuf::as_path)
    }

    /// `true` when there is somewhere to go back to.
    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.current > 0
    }

    /// `true` when there is somewhere to go forward to.
    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        self.current + 1 < self.past.len()
    }

    /// How many entries are in the forward stack — shown in the toolbar tooltip.
    #[must_use]
    pub fn forward_len(&self) -> usize {
        self.future.len()
    }

    /// Records a navigation to `dir`.
    ///
    /// A navigation to where we already are is ignored: clicking the active
    /// place in the sidebar should not fill the history with duplicates, or
    /// twenty clicks of "Home" needs twenty `Back` presses to undo one.
    pub fn push(&mut self, dir: &Path) {
        if self.current().is_some_and(|c| c == dir) {
            return;
        }
        // Anything ahead of us is now unreachable. This is the invariant.
        self.future.clear();
        self.past.truncate(self.current + 1);
        self.past.push(dir.to_path_buf());
        self.current = self.past.len() - 1;
        self.start.get_or_insert_with(|| dir.to_path_buf());
        self.enforce_cap();
    }

    /// Moves back one entry, returning where that lands.
    pub fn back(&mut self) -> Option<&Path> {
        if !self.can_go_back() {
            return self.current();
        }
        // Skip over a forward entry that is identical to where we are going,
        // which a capped history can produce.
        while self.current > 0 {
            self.current -= 1;
            if !self.future.is_empty() && self.future.last() == self.past.get(self.current) {
                self.future.pop();
                continue;
            }
            break;
        }
        if let Some(next) = self.past.get(self.current + 1) {
            self.future.push(next.clone());
        }
        self.current()
    }

    /// Moves forward one entry, returning where that lands.
    ///
    /// The cap is re-enforced here as well as in [`History::push`]. A push always
    /// truncates, so `past` only reaches the cap by growing; but `forward` moves
    /// an entry *back* onto `past` from `future`, and without a second
    /// `enforce_cap` a walk of back/forward/push/forward can leave `past` one
    /// entry over. A 2000-step randomised walk in the tests is what found it —
    /// a single forward-only sequence never does.
    pub fn forward(&mut self) -> Option<&Path> {
        let Some(next) = self.future.pop() else {
            return self.current();
        };
        self.past.push(next);
        self.current = self.past.len() - 1;
        self.enforce_cap();
        self.current()
    }

    /// Records a *replacement* of the current entry, without adding depth.
    ///
    /// Used when the same logical location is re-resolved — following a symlink,
    /// or a rescan that canonicalises the path differently. Without this, a
    /// symlink loop in the user's clicking would build unbounded history.
    pub fn replace_current(&mut self, dir: &Path) {
        if let Some(slot) = self.past.get_mut(self.current) {
            *slot = dir.to_path_buf();
        }
        self.start.get_or_insert_with(|| dir.to_path_buf());
    }

    /// Enforces [`MAX_DEPTH`] by dropping the **oldest** entries.
    ///
    /// `current` is re-based by the same amount, so the entry the user is looking
    /// at stays put. That holds for `forward` too: it just pushed onto the end,
    /// so after dropping `excess` from the front, `current - excess` is still
    /// the same entry.
    ///
    /// Oldest-first, not newest-first: a user pressing `Back` wants to reach
    /// where they were a minute ago, and the entry point they booted into is
    /// the one they are least likely to want. `current` is adjusted by the same
    /// amount so the position in the stack stays put.
    fn enforce_cap(&mut self) {
        let Some(excess) = self.past.len().checked_sub(MAX_DEPTH) else {
            return;
        };
        if excess == 0 {
            return;
        }
        self.past.drain(..excess);
        self.current = self.current.saturating_sub(excess);
        self.start = self.past.first().cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    /// Total entries held. Lives in the test module because the app never asks:
    /// the toolbar reports `forward_len`, which is the only depth a user can act
    /// on.
    fn total(h: &History) -> usize {
        h.past.len() + h.future.len()
    }

    #[test]
    fn back_and_forward_walk_the_stack() {
        let mut h = History::new(&p("/"));
        assert_eq!(h.current(), Some(p("/").as_path()));
        assert!(!h.can_go_back());
        assert!(!h.can_go_forward());

        h.push(&p("/home"));
        h.push(&p("/home/achraf"));
        assert_eq!(h.current(), Some(p("/home/achraf").as_path()));
        assert!(h.can_go_back());

        assert_eq!(h.back(), Some(p("/home").as_path()));
        assert_eq!(h.back(), Some(p("/").as_path()));
        assert!(!h.can_go_back());
        // Going back at the start is a no-op, not an underflow.
        assert_eq!(h.back(), Some(p("/").as_path()));

        assert_eq!(h.forward(), Some(p("/home").as_path()));
        assert_eq!(h.forward(), Some(p("/home/achraf").as_path()));
        assert!(!h.can_go_forward());
        assert_eq!(h.forward(), Some(p("/home/achraf").as_path()));
    }

    #[test]
    fn navigating_after_going_back_discards_the_forward_stack() {
        // The invariant. Without it: /a -> /b -> back to /a -> click /c, and
        // Forward still offers /b, so the user lands in a directory they
        // deliberately left.
        let mut h = History::new(&p("/"));
        h.push(&p("/a"));
        h.push(&p("/b"));
        h.back();
        assert!(h.can_go_forward());

        h.push(&p("/c"));
        assert!(
            !h.can_go_forward(),
            "forward must be discarded on navigation"
        );
        assert_eq!(h.forward_len(), 0);
        assert_eq!(h.current(), Some(p("/c").as_path()));
    }

    #[test]
    fn the_cap_holds_and_drops_the_oldest_entries() {
        let mut h = History::new(&p("/"));
        for i in 0..(MAX_DEPTH * 3) {
            h.push(&p(&format!("/dir{i}")));
        }
        assert!(h.past.len() <= MAX_DEPTH, "past grew to {}", h.past.len());
        // The current position is still the newest entry.
        assert_eq!(
            h.current(),
            Some(p(&format!("/dir{}", MAX_DEPTH * 3 - 1)).as_path())
        );
        // The oldest are the ones dropped: the stack starts at a recent dir, not
        // at the boot directory.
        assert!(h.can_go_back(), "the cap must not eat the whole stack");
        assert_ne!(h.past.first(), Some(&p("/")));
    }

    #[test]
    fn the_cap_keeps_exactly_max_depth_entries() {
        let mut h = History::new(&p("/"));
        for i in 0..MAX_DEPTH {
            h.push(&p(&format!("/d{i}")));
        }
        assert_eq!(h.past.len(), MAX_DEPTH, "at the cap, nothing is dropped");
        h.push(&p("/one-too-many"));
        assert_eq!(h.past.len(), MAX_DEPTH, "over the cap, one is dropped");
    }

    #[test]
    fn navigating_to_where_we_already_are_is_ignored() {
        let mut h = History::new(&p("/home"));
        let before = total(&h);
        h.push(&p("/home"));
        h.push(&p("/home"));
        assert_eq!(
            total(&h),
            before,
            "re-navigating to the same dir adds depth"
        );
        assert!(!h.can_go_back());
    }

    #[test]
    fn replace_current_does_not_add_depth() {
        let mut h = History::new(&p("/"));
        h.push(&p("/a"));
        h.replace_current(&p("/a/"));
        assert_eq!(total(&h), 2, "replace must not grow the stack");
        assert_eq!(h.current(), Some(p("/a/").as_path()));
        assert_eq!(h.back(), Some(p("/").as_path()));
    }

    #[test]
    fn a_long_random_walk_never_exceeds_the_cap() {
        // A property check over a deterministic pseudo-random walk: no sequence
        // of push/back/forward can grow the *back stack* past the cap, which is
        // the only reason the cap exists.
        //
        // The bound is on `past` alone, not on `past + future`. A user who walks
        // back 50 times legitimately has 50 forward entries, so the sum can
        // legitimately reach `2 * MAX_DEPTH`; what must not happen is unbounded
        // growth of either stack, and `future` is bounded by how far back the
        // user has gone, which `past` bounds in turn.
        let mut h = History::new(&p("/0"));
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for step in 0..2000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            match seed % 3 {
                0 => h.push(&p(&format!("/{step}"))),
                1 => {
                    h.back();
                }
                _ => {
                    h.forward();
                }
            }
            assert!(
                h.past.len() <= MAX_DEPTH,
                "step {step}: back stack grew to {}",
                h.past.len()
            );
            assert!(
                h.future.len() <= MAX_DEPTH,
                "step {step}: forward stack grew to {}",
                h.future.len()
            );
        }
    }

    #[test]
    fn the_cap_holds_through_a_back_forward_storm() {
        // The specific sequence that found the missing `enforce_cap` in
        // `forward`: fill the back stack to the cap, then walk back and forward
        // repeatedly. Every `forward` pushes onto `past` again.
        let mut h = History::new(&p("/0"));
        for i in 0..MAX_DEPTH {
            h.push(&p(&format!("/{i}")));
        }
        assert_eq!(h.past.len(), MAX_DEPTH);
        for _ in 0..50 {
            h.back();
            assert!(h.past.len() <= MAX_DEPTH);
            h.forward();
            assert!(
                h.past.len() <= MAX_DEPTH,
                "forward pushed the back stack to {}",
                h.past.len()
            );
        }
    }
}
