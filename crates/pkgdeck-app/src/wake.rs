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
    use std::cell::Cell;

    thread_local! {
        // Per thread, so wakes from tests running alongside don't count here.
        static WOKEN: Cell<usize> = const { Cell::new(0) };
    }

    #[test]
    fn each_message_wakes_the_front_end() {
        set_waker(|| WOKEN.set(WOKEN.get() + 1));
        let (sender, receiver) = channel();
        let copy = sender.clone();
        sender.send(1).unwrap();
        copy.send(2).unwrap();
        // A sender on another thread, like the ones other tests use.
        std::thread::spawn(|| channel().0.send(0))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!(receiver.try_iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(WOKEN.get(), 2);
        drop(receiver);
        assert!(sender.send(3).is_err());
        assert_eq!(WOKEN.get(), 3);
    }
}
