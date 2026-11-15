//! Typed errors. Messages never contain URLs or page titles, so they are safe to log.

use krypa_engine_api::{PermissionRequestId, SessionId};

use crate::lifecycle::{SessionState, Trigger};

/// Everything the core can refuse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// The id was never issued by this manager.
    #[error("unknown session {0:?}")]
    UnknownSession(SessionId),
    /// The session existed but was closed or discarded.
    #[error("session {0:?} was already destroyed")]
    SessionDestroyed(SessionId),
    /// The lifecycle table forbids this transition.
    #[error("session {session:?}: illegal transition {trigger:?} from {from:?}")]
    IllegalTransition {
        /// Session concerned.
        session: SessionId,
        /// State it was in.
        from: SessionState,
        /// Refused trigger.
        trigger: Trigger,
    },
    /// The action or event does not make sense in the session's current state.
    #[error("session {session:?}: {what} is not valid in state {state:?}")]
    NotAllowedInState {
        /// Session concerned.
        session: SessionId,
        /// State it was in.
        state: SessionState,
        /// Name of the action or event.
        what: &'static str,
    },
    /// The live-view budget must allow at least one view.
    #[error("live-view budget must be at least 1")]
    InvalidBudget,
    /// The platform does not support what was asked for.
    #[error("platform capability missing: {0}")]
    CapabilityMissing(&'static str),
    /// The URL is not an `http(s)` URL with a host and no credentials.
    #[error("url is not loadable as a top-level page")]
    UrlNotLoadable,
    /// An external-scheme event carried a scheme that must never be handed out.
    #[error("event carried a scheme that must not be opened externally")]
    UnsafeExternalScheme,
    /// An answer to an external-open prompt that is not pending.
    #[error("session {0:?}: no external-open prompt is pending")]
    NoPendingExternalOpen(SessionId),
    /// Progress above 1000 per mille.
    #[error("progress {0} is out of range 0..=1000")]
    InvalidProgress(u16),
    /// Permission response for a prompt that is not pending.
    #[error("session {session:?}: no pending permission request {request:?}")]
    UnknownPermissionRequest {
        /// Session concerned.
        session: SessionId,
        /// Prompt id.
        request: PermissionRequestId,
    },
    /// The same prompt id was reported twice.
    #[error("session {session:?}: permission request {request:?} reported twice")]
    DuplicatePermissionRequest {
        /// Session concerned.
        session: SessionId,
        /// Prompt id.
        request: PermissionRequestId,
    },
}
