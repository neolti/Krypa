//! Session lifecycle states and the explicit legal-transition table.

use serde::Serialize;

/// Where a session is in its life. `Suspended` and `Terminated` are different things:
/// a suspended session still owns a live web view, a terminated one does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SessionState {
    /// No web view has been created yet.
    NotRunning,
    /// A web view exists and its first document has not been committed yet.
    Loading,
    /// The live view the user is looking at.
    Foreground,
    /// A live view that is not on screen.
    Background,
    /// A live view, not on screen, which Krypa told to pause where the platform can.
    /// This never means JavaScript is paused; see `EngineCapabilities::view_suspend`.
    Suspended,
    /// The web view was destroyed (evicted or lost). The URL is kept for restore.
    Terminated,
    /// The view is alive but its page failed to load or its content process died. Only an
    /// explicit user action or the next foreground leaves this state.
    Failed,
}

impl SessionState {
    /// Every state, for exhaustive tests and tables.
    pub const ALL: [Self; 7] = [
        Self::NotRunning,
        Self::Loading,
        Self::Foreground,
        Self::Background,
        Self::Suspended,
        Self::Terminated,
        Self::Failed,
    ];

    /// Whether a web view exists in this state (counts against the live-view budget).
    #[must_use]
    pub const fn has_live_view(self) -> bool {
        !matches!(self, Self::NotRunning | Self::Terminated)
    }
}

/// What can happen to a session's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Trigger {
    /// A web view was created.
    ViewCreated,
    /// First document committed while this is the foreground session.
    CommittedForeground,
    /// First document committed while another session is foreground.
    CommittedBackground,
    /// Brought on screen.
    SentToForeground,
    /// Taken off screen.
    SentToBackground,
    /// Told to pause.
    Suspend,
    /// Told to resume.
    Resume,
    /// A page load failed.
    LoadFailed,
    /// The content process died.
    ProcessCrashed,
    /// Reload or new load started from `Failed`.
    RetryStarted,
    /// Krypa destroyed the view to save memory (never the foreground one).
    Evict,
    /// The platform destroyed the view on its own.
    ViewLost,
}

impl Trigger {
    /// Every trigger, for exhaustive tests.
    pub const ALL: [Self; 12] = [
        Self::ViewCreated,
        Self::CommittedForeground,
        Self::CommittedBackground,
        Self::SentToForeground,
        Self::SentToBackground,
        Self::Suspend,
        Self::Resume,
        Self::LoadFailed,
        Self::ProcessCrashed,
        Self::RetryStarted,
        Self::Evict,
        Self::ViewLost,
    ];
}

/// A transition that the table does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("illegal lifecycle transition: {trigger:?} in state {from:?}")]
pub struct IllegalTransition {
    /// State the session was in.
    pub from: SessionState,
    /// Trigger that was refused.
    pub trigger: Trigger,
}

impl SessionState {
    /// The single source of truth for legal transitions.
    ///
    /// # Errors
    /// [`IllegalTransition`] if `trigger` is not legal in `self`.
    pub const fn next(self, trigger: Trigger) -> Result<Self, IllegalTransition> {
        use SessionState::{
            Background, Failed, Foreground, Loading, NotRunning, Suspended, Terminated,
        };
        use Trigger as T;
        let to = match (self, trigger) {
            (NotRunning | Terminated, T::ViewCreated) => Loading,
            (Loading, T::CommittedForeground) => Foreground,
            (Loading, T::CommittedBackground) => Background,
            (Background | Suspended, T::SentToForeground) => Foreground,
            (Foreground, T::SentToBackground) => Background,
            (Background, T::Suspend) => Suspended,
            (Suspended, T::Resume) => Background,
            (Loading | Foreground | Background, T::LoadFailed) => Failed,
            (Loading | Foreground | Background | Suspended, T::ProcessCrashed) => Failed,
            (Failed, T::RetryStarted) => Loading,
            (Loading | Background | Suspended | Failed, T::Evict) => Terminated,
            (Loading | Foreground | Background | Suspended | Failed, T::ViewLost) => Terminated,
            _ => {
                return Err(IllegalTransition {
                    from: self,
                    trigger,
                })
            }
        };
        Ok(to)
    }
}

#[cfg(test)]
mod tests {
    use super::SessionState::{
        Background, Failed, Foreground, Loading, NotRunning, Suspended, Terminated,
    };
    use super::{SessionState, Trigger};
    use Trigger as T;

    /// The documented table, written out independently of `next`.
    const LEGAL: [(SessionState, Trigger, SessionState); 26] = [
        (NotRunning, T::ViewCreated, Loading),
        (Terminated, T::ViewCreated, Loading),
        (Loading, T::CommittedForeground, Foreground),
        (Loading, T::CommittedBackground, Background),
        (Background, T::SentToForeground, Foreground),
        (Suspended, T::SentToForeground, Foreground),
        (Foreground, T::SentToBackground, Background),
        (Background, T::Suspend, Suspended),
        (Suspended, T::Resume, Background),
        (Loading, T::LoadFailed, Failed),
        (Foreground, T::LoadFailed, Failed),
        (Background, T::LoadFailed, Failed),
        (Loading, T::ProcessCrashed, Failed),
        (Foreground, T::ProcessCrashed, Failed),
        (Background, T::ProcessCrashed, Failed),
        (Suspended, T::ProcessCrashed, Failed),
        (Failed, T::RetryStarted, Loading),
        (Loading, T::Evict, Terminated),
        (Background, T::Evict, Terminated),
        (Suspended, T::Evict, Terminated),
        (Failed, T::Evict, Terminated),
        (Loading, T::ViewLost, Terminated),
        (Foreground, T::ViewLost, Terminated),
        (Background, T::ViewLost, Terminated),
        (Suspended, T::ViewLost, Terminated),
        (Failed, T::ViewLost, Terminated),
    ];

    #[test]
    fn every_pair_outside_the_table_is_a_typed_error_and_every_listed_pair_is_legal() {
        let mut illegal = 0;
        for from in SessionState::ALL {
            for trigger in Trigger::ALL {
                let listed = LEGAL.iter().find(|(f, t, _)| *f == from && *t == trigger);
                match (from.next(trigger), listed) {
                    (Ok(to), Some((_, _, expected))) => assert_eq!(to, *expected),
                    (Ok(to), None) => panic!("unlisted legal pair {from:?} {trigger:?} -> {to:?}"),
                    (Err(e), None) => {
                        assert_eq!((e.from, e.trigger), (from, trigger));
                        illegal += 1;
                    }
                    (Err(e), Some(_)) => panic!("listed pair refused: {e}"),
                }
            }
        }
        // 7 states x 12 triggers = 84 pairs, 26 of them legal.
        assert_eq!(illegal, 84 - 26);
    }

    #[test]
    fn a_foreground_view_can_never_be_evicted_only_lost_by_the_platform() {
        assert!(Foreground.next(T::Evict).is_err());
        assert_eq!(Foreground.next(T::ViewLost), Ok(Terminated));
    }

    #[test]
    fn suspended_keeps_a_live_view_and_terminated_does_not() {
        assert!(Suspended.has_live_view());
        assert!(!Terminated.has_live_view());
        assert_ne!(Suspended, Terminated);
    }

    #[test]
    fn failed_only_recovers_through_an_explicit_retry() {
        for trigger in Trigger::ALL {
            let to = Failed.next(trigger);
            match trigger {
                T::RetryStarted => assert_eq!(to, Ok(Loading)),
                T::Evict | T::ViewLost => assert_eq!(to, Ok(Terminated)),
                _ => assert!(to.is_err(), "{trigger:?} must not leave Failed"),
            }
        }
    }
}
