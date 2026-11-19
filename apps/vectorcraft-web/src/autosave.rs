//! Autosave in host mode: once the document in front has unsaved changes and the user has left it
//! alone for [`IDLE`], the shell runs the app's own Save, which reaches the host as `save` with
//! `saveAs: false`, in the document's own format and without a dialog. Only a hosted page keeps an
//! [`Autosave`]; the desktop app and an unhosted page never save on their own.
//!
//! The same file is in each Craft app's web shell: the rule is small, and the apps share no crate
//! of their own.

use web_time::{Duration, Instant};

/// How long the user leaves the document alone before it saves.
pub const IDLE: Duration = Duration::from_secs(3);

/// How soon to look again while a save is still running.
const SAVING_POLL: Duration = Duration::from_millis(250);

/// A save is due: unsaved changes, no save running, and the last change [`IDLE`] ago or more.
pub fn due(dirty: bool, last_change: Instant, now: Instant, saving: bool) -> bool {
    dirty && !saving && now.saturating_duration_since(last_change) >= IDLE
}

/// The user is at work this pass: a button or finger held (a stroke, a drag), or keys, text,
/// clicks, scrolls, pinches or pastes. A pointer only moving over the window is not work.
#[cfg(target_arch = "wasm32")]
pub fn user_busy(ctx: &egui::Context) -> bool {
    use egui::Event;
    ctx.input(|i| {
        i.pointer.any_down()
            || i.events.iter().any(|e| {
                matches!(
                    e,
                    Event::Copy
                        | Event::Cut
                        | Event::Paste(_)
                        | Event::PasteImage(_)
                        | Event::MiddleClickPaste { .. }
                        | Event::Text(_)
                        | Event::Key { .. }
                        | Event::PointerButton { .. }
                        | Event::Zoom(_)
                        | Event::Rotate(_)
                        | Event::Ime(_)
                        | Event::Touch { .. }
                        | Event::MouseWheel { .. }
                )
            })
    })
}

/// The document in front: which one it is, and a number that changes with each edit of it (a
/// revision, or the depth of its undo history).
pub type Front = (u64, u64);

/// Follows the document in front from pass to pass and says when to save it.
pub struct Autosave {
    /// The document in front last pass.
    front: Option<Front>,
    dirty: bool,
    last_change: Instant,
    /// An edit made since the last save this module asked for. One autosave per edit: a save that
    /// leaves the document dirty (it failed, or the host refused it) is not repeated until the next
    /// edit.
    pending: bool,
}

impl Autosave {
    pub fn new(now: Instant) -> Self {
        Self { front: None, dirty: false, last_change: now, pending: false }
    }

    /// One pass. `front` is the document in front as it is now, `dirty` says it has unsaved
    /// changes, `busy` that the user is at work ([`user_busy`]), `saving` that a save of it is
    /// still running. → whether to run the app's Save now.
    ///
    /// An edit, or unsaved changes appearing, starts the idle time again, and so does any work by
    /// the user. Another document in front is no edit: one that opens with unsaved changes (an
    /// app that converts what it reads) waits for the user's first edit.
    pub fn step(&mut self, front: Option<Front>, dirty: bool, busy: bool, saving: bool, now: Instant) -> bool {
        let same_document = self.front.map(|(doc, _)| doc) == front.map(|(doc, _)| doc);
        let changed = self.front != front || (dirty && !self.dirty);
        if changed || busy {
            self.last_change = now;
        }
        self.pending = dirty && same_document && (self.pending || changed);
        self.front = front;
        self.dirty = dirty;
        if self.pending && due(dirty, self.last_change, now, saving) {
            self.pending = false;
            return true;
        }
        false
    }

    /// How long until a pass should look again while a save is pending (an idle page runs no
    /// passes of its own): when the idle time ends, or soon while a save still runs.
    pub fn wake_in(&self, now: Instant) -> Option<Duration> {
        let left = IDLE.saturating_sub(now.saturating_duration_since(self.last_change));
        self.pending.then(|| if left.is_zero() { SAVING_POLL } else { left })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    /// An Autosave that has seen document 1 open, clean, at `t`.
    fn opened(t: Instant) -> Autosave {
        let mut a = Autosave::new(t);
        assert!(!a.step(Some((1, 0)), false, false, false, t));
        a
    }

    #[test]
    fn due_once_the_last_change_is_three_seconds_old() {
        let t = Instant::now();
        assert!(!due(true, t, at(t, 2_999), false));
        assert!(due(true, t, at(t, 3_000), false));
        assert!(due(true, t, at(t, 60_000), false));
    }

    #[test]
    fn never_due_while_saving_or_clean() {
        let t = Instant::now();
        assert!(!due(true, t, at(t, 5_000), true));
        assert!(!due(false, t, at(t, 5_000), false));
        assert!(!due(false, t, at(t, 5_000), true));
    }

    #[test]
    fn a_clock_that_seems_to_run_back_is_no_idle_time() {
        let t = Instant::now();
        assert!(!due(true, at(t, 1_000), t, false));
    }

    #[test]
    fn one_edit_then_idle_saves_once() {
        let t = Instant::now();
        let mut a = opened(t);
        assert!(!a.step(Some((1, 1)), true, true, false, at(t, 100)), "the edit");
        assert!(!a.step(Some((1, 1)), true, false, false, at(t, 3_099)));
        assert!(a.step(Some((1, 1)), true, false, false, at(t, 3_100)));
        assert!(!a.step(Some((1, 1)), true, false, false, at(t, 9_000)), "no second save of the same edit");
    }

    #[test]
    fn an_open_is_not_saved() {
        let t = Instant::now();
        let mut a = Autosave::new(t);
        assert!(!a.step(None, false, false, false, t), "no document yet");
        assert!(!a.step(Some((7, 0)), false, false, false, at(t, 10)), "it arrives clean");
        assert!(!a.step(Some((7, 0)), false, false, false, at(t, 10_000)));
        assert_eq!(a.wake_in(at(t, 10_000)), None);
    }

    #[test]
    fn a_document_that_opens_dirty_waits_for_an_edit() {
        let t = Instant::now();
        let mut a = Autosave::new(t);
        assert!(!a.step(None, false, false, false, t));
        assert!(!a.step(Some((7, 3)), true, false, false, at(t, 10)), "converted as it was read");
        assert!(!a.step(Some((7, 3)), true, false, false, at(t, 10_000)));
        assert_eq!(a.wake_in(at(t, 10_000)), None);
        assert!(!a.step(Some((7, 4)), true, true, false, at(t, 11_000)), "the user's first edit");
        assert!(a.step(Some((7, 4)), true, false, false, at(t, 14_000)));
    }

    #[test]
    fn another_document_in_front_starts_afresh() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, false, false, at(t, 100));
        assert!(!a.step(Some((2, 0)), false, false, false, at(t, 1_000)), "another one opens");
        assert!(!a.step(Some((2, 0)), false, false, false, at(t, 10_000)));
    }

    #[test]
    fn edits_keep_putting_it_off() {
        let t = Instant::now();
        let mut a = opened(t);
        for i in 0..10u64 {
            assert!(!a.step(Some((1, i + 1)), true, false, false, at(t, i * 1_000)), "edit {i}");
        }
        assert!(!a.step(Some((1, 10)), true, false, false, at(t, 11_999)));
        assert!(a.step(Some((1, 10)), true, false, false, at(t, 12_000)));
    }

    #[test]
    fn a_held_stroke_puts_it_off() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, true, false, at(t, 10));
        for ms in (500..8_000).step_by(500) {
            assert!(!a.step(Some((1, 1)), true, true, false, at(t, ms)), "pointer held at {ms} ms");
        }
        assert!(!a.step(Some((1, 1)), true, false, false, at(t, 10_000)));
        assert!(a.step(Some((1, 1)), true, false, false, at(t, 10_500)));
    }

    #[test]
    fn waits_while_a_save_runs_then_saves_if_still_dirty() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, false, false, t);
        assert!(!a.step(Some((1, 1)), true, false, true, at(t, 4_000)));
        assert_eq!(a.wake_in(at(t, 4_000)), Some(SAVING_POLL));
        assert!(a.step(Some((1, 1)), true, false, false, at(t, 4_500)));
    }

    #[test]
    fn a_save_that_cleans_the_document_ends_the_wait() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, false, false, t);
        assert!(!a.step(Some((1, 1)), false, false, false, at(t, 1_000)), "saved with Ctrl+S meanwhile");
        assert!(!a.step(Some((1, 1)), false, false, false, at(t, 5_000)));
        assert_eq!(a.wake_in(at(t, 5_000)), None);
    }

    #[test]
    fn a_failed_save_waits_for_the_next_edit() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, false, false, t);
        assert!(a.step(Some((1, 1)), true, false, false, at(t, 3_000)));
        assert!(!a.step(Some((1, 1)), true, false, false, at(t, 9_000)), "still dirty, but nothing new");
        assert!(!a.step(Some((1, 2)), true, false, false, at(t, 9_100)));
        assert!(a.step(Some((1, 2)), true, false, false, at(t, 12_100)));
    }

    #[test]
    fn wakes_when_the_idle_time_ends() {
        let t = Instant::now();
        let mut a = opened(t);
        a.step(Some((1, 1)), true, false, false, t);
        assert_eq!(a.wake_in(at(t, 1_000)), Some(Duration::from_secs(2)));
    }
}
