//! Lets worker threads wake the front end when they send it something, so
//! the window can sleep instead of checking on a timer.
use std::sync::{mpsc, OnceLock};

static WAKER: OnceLock<fn()> = OnceLock::new();

/// What wakes the front end. Only the first call counts.
pub fn set_waker(waker: fn()) {
    let _ = WAKER.set(waker);
}

fn wake() {
    if let Some(waker) = WAKER.get() {
        waker();
    }
}

/// An [`mpsc::Sender`] that wakes the front end after each message.
pub struct Sender<T>(mpsc::Sender<T>);

impl<T> Sender<T> {
    pub fn send(&self, value: T) -> Result<(), mpsc::SendError<T>> {
        let sent = self.0.send(value);
        wake();
        sent
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// [`mpsc::channel`] with a [`Sender`] that wakes the front end.
pub fn channel<T>() -> (Sender<T>, mpsc::Receiver<T>) {
    let (sender, receiver) = mpsc::channel();
    (Sender(sender), receiver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static WOKEN: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn each_message_wakes_the_front_end() {
        set_waker(|| {
            WOKEN.fetch_add(1, Ordering::SeqCst);
        });
        let (sender, receiver) = channel();
        let copy = sender.clone();
        sender.send(1).unwrap();
        copy.send(2).unwrap();
        assert_eq!(receiver.try_iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(WOKEN.load(Ordering::SeqCst), 2);
        drop(receiver);
        assert!(sender.send(3).is_err());
        assert_eq!(WOKEN.load(Ordering::SeqCst), 3);
    }
}
