//! Messages from background threads (save threads, the watcher) to the
//! main thread: a queue, and a wake-up that tells a GPUI task to drain it.
//!
//! GPUI's test scheduler rejects wake-ups from foreign threads, so in test
//! builds the wake-up is off and tests drain the queue themselves; the
//! queue and the draining code are the same in both.

use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use parking_lot::Mutex;

pub struct Inbox<T> {
    queue: Mutex<Vec<T>>,
    wake: Option<UnboundedSender<()>>,
}

/// The receiving end: resolves once per wake-up, `None` once every sender
/// is gone. Never resolves in test builds.
pub struct Wakeups(Option<UnboundedReceiver<()>>);

impl Wakeups {
    pub async fn next(&mut self) -> Option<()> {
        match &mut self.0 {
            Some(receiver) => receiver.next().await,
            None => None,
        }
    }
}

impl<T> Inbox<T> {
    pub fn new() -> (Arc<Self>, Wakeups) {
        let (wake, receiver) = if cfg!(test) {
            (None, None)
        } else {
            let (sender, receiver) = unbounded();
            (Some(sender), Some(receiver))
        };
        let inbox = Arc::new(Self {
            queue: Mutex::new(Vec::new()),
            wake,
        });
        (inbox, Wakeups(receiver))
    }

    pub fn push(&self, message: T) {
        self.queue.lock().push(message);
        if let Some(wake) = &self.wake {
            let _ = wake.unbounded_send(());
        }
    }

    /// Everything queued since the last call, oldest first.
    pub fn take(&self) -> Vec<T> {
        std::mem::take(&mut *self.queue.lock())
    }
}
