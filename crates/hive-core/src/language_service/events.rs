use super::{Error, Event};
use std::{
    future::{Future as _, poll_fn},
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::{self, Thread},
    time::Duration,
};

struct Notify(Thread);
impl Wake for Notify {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub(super) fn send(
    sender: &async_channel::Sender<Event>,
    event: Event,
    stopped: &AtomicBool,
) -> Result<(), Error> {
    let waker = Waker::from(Arc::new(Notify(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut send = pin!(delivery(sender, event, stopped));
    loop {
        match send.as_mut().poll(&mut context) {
            Poll::Ready(result) => return result,
            // The receiver wakes this worker on capacity; the timeout also observes session drop.
            Poll::Pending => thread::park_timeout(Duration::from_millis(25)),
        }
    }
}

async fn delivery(
    sender: &async_channel::Sender<Event>,
    event: Event,
    stopped: &AtomicBool,
) -> Result<(), Error> {
    let mut send = pin!(sender.send(event));
    poll_fn(|cx| {
        if stopped.load(Ordering::Acquire) {
            Poll::Ready(Err(Error::Cancelled))
        } else {
            send.as_mut()
                .poll(cx)
                .map(|result| result.map_err(|_| Error::Cancelled))
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_event_queue_applies_backpressure_without_losing_events() {
        let (sender, receiver) = async_channel::bounded(1);
        sender
            .try_send(Event::Failed(Error::Unavailable("first")))
            .unwrap();
        let stopped = AtomicBool::new(false);
        let mut context = Context::from_waker(Waker::noop());
        let mut pending = pin!(delivery(
            &sender,
            Event::Failed(Error::Unavailable("second")),
            &stopped
        ));
        assert!(pending.as_mut().poll(&mut context).is_pending());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Event::Failed(Error::Unavailable("first"))
        ));
        assert!(matches!(
            pending.as_mut().poll(&mut context),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            Event::Failed(Error::Unavailable("second"))
        ));
    }

    #[test]
    fn cancellation_and_a_closed_receiver_do_not_wait_for_capacity() {
        let (sender, receiver) = async_channel::bounded(1);
        sender.try_send(Event::Failed(Error::TimedOut)).unwrap();
        assert!(matches!(
            send(
                &sender,
                Event::Failed(Error::TimedOut),
                &AtomicBool::new(true)
            ),
            Err(Error::Cancelled)
        ));
        let stopped = AtomicBool::new(false);
        let mut pending = pin!(delivery(&sender, Event::Failed(Error::TimedOut), &stopped));
        let mut context = Context::from_waker(Waker::noop());
        assert!(pending.as_mut().poll(&mut context).is_pending());
        stopped.store(true, Ordering::Release);
        assert!(matches!(
            pending.as_mut().poll(&mut context),
            Poll::Ready(Err(Error::Cancelled))
        ));
        drop(receiver);
        assert!(matches!(
            send(
                &sender,
                Event::Failed(Error::TimedOut),
                &AtomicBool::new(false)
            ),
            Err(Error::Cancelled)
        ));
    }
}
