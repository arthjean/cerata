use std::sync::Arc;

use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use parking_lot::Mutex;

pub struct Inbox<T> {
    queue: Mutex<Vec<T>>,
    wake: Option<UnboundedSender<()>>,
}

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

    pub fn take(&self) -> Vec<T> {
        std::mem::take(&mut *self.queue.lock())
    }
}
