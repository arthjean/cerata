//! Workspace epochs: a counter bumped on every workspace transition so that
//! asynchronous work started for an older workspace can tell it is stale.
//!
//! Ported from Writer `state.rs` (`workspace_epoch`, `with_workspace_snapshot`)
//! and generalized: every background job (indexing, title batches, watcher
//! events) carries an [`EpochToken`] and checks it before publishing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared, monotonically increasing epoch counter of one window's workspace.
#[derive(Debug, Clone, Default)]
pub struct EpochCounter(Arc<AtomicU64>);

impl EpochCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Starts a new epoch and returns it. Every token of an older epoch
    /// becomes stale, which cancels the jobs that hold them.
    pub fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Token for the current epoch.
    pub fn token(&self) -> EpochToken {
        EpochToken {
            counter: self.clone(),
            epoch: self.current(),
        }
    }
}

/// A job's view of the epoch it was started in.
#[derive(Debug, Clone)]
pub struct EpochToken {
    counter: EpochCounter,
    epoch: u64,
}

impl EpochToken {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// `true` while no newer epoch has started.
    pub fn is_current(&self) -> bool {
        self.counter.current() == self.epoch
    }

    pub fn is_stale(&self) -> bool {
        !self.is_current()
    }

    /// A token that is already stale, for tests and pre-canceled jobs.
    pub fn canceled() -> Self {
        let counter = EpochCounter::new();
        let token = counter.token();
        counter.advance();
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advancing_the_counter_cancels_older_tokens_only() {
        let counter = EpochCounter::new();
        let first = counter.token();
        assert!(first.is_current());
        let second_epoch = counter.advance();
        let second = counter.token();
        assert!(first.is_stale());
        assert_eq!(second.epoch(), second_epoch);
        assert!(second.is_current());
    }

    #[test]
    fn a_clone_of_the_counter_sees_the_same_epochs() {
        // Writer `test_live_cancel_token_survives_after_swap`: a job holding a
        // clone keeps seeing cancellation after the slot moves on.
        let counter = EpochCounter::new();
        let job_view = counter.clone().token();
        counter.advance();
        assert!(job_view.is_stale());
        assert!(EpochToken::canceled().is_stale());
    }
}
