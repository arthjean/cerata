use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Default)]
pub struct EpochCounter(Arc<AtomicU64>);

impl EpochCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn advance(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn token(&self) -> EpochToken {
        EpochToken {
            counter: self.clone(),
            epoch: self.current(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EpochToken {
    counter: EpochCounter,
    epoch: u64,
}

impl EpochToken {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn is_current(&self) -> bool {
        self.counter.current() == self.epoch
    }

    pub fn is_stale(&self) -> bool {
        !self.is_current()
    }

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
        let counter = EpochCounter::new();
        let job_view = counter.clone().token();
        counter.advance();
        assert!(job_view.is_stale());
        assert!(EpochToken::canceled().is_stale());
    }
}
