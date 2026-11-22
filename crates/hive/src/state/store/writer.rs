use async_channel::{Receiver, Sender};
use gpui_kit::{BackgroundExecutor, Task};
use hive_core::state::{State, StateError};
use std::cell::Cell;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::Poll;
use std::time::{Duration, Instant};

/// Waits before a failed save is tried again. What usually fails a save (a full disk, a
/// folder that is briefly unwritable) clears within seconds or needs the user; a few spaced
/// attempts cover the first without spinning on the second.
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

/// Appended to the error the user is shown, so a failed save does not read as a lost change.
const RETRY_NOTICE: &str = "Hive will try again shortly and when you quit.";

type Acknowledgments = Arc<Mutex<VecDeque<(u64, Sender<Result<(), String>>)>>>;
struct Snapshot {
    sequence: u64,
    state: State,
}

pub(super) struct Writer {
    snapshots: Sender<Snapshot>,
    sequence: Cell<u64>,
    acknowledgments: Acknowledgments,
    done: mpsc::Receiver<()>,
    _task: Task<()>,
}

impl Writer {
    pub(super) fn with_save(
        executor: &BackgroundExecutor,
        save: impl Fn(&State) -> Result<(), StateError> + Send + 'static,
    ) -> (Self, Receiver<String>) {
        let (snapshots, receiver) = async_channel::bounded(1);
        let (errors, error_receiver) = async_channel::bounded(1);
        let (done_sender, done) = mpsc::channel();
        let acknowledgments = Acknowledgments::default();
        let pending = acknowledgments.clone();
        let timer = executor.clone();
        let task = executor.spawn(async move {
            write_snapshots(receiver, errors, &pending, save, |delay| timer.timer(delay)).await;
            let _ = done_sender.send(());
        });
        (
            Self {
                snapshots,
                sequence: Cell::default(),
                acknowledgments,
                done,
                _task: task,
            },
            error_receiver,
        )
    }

    pub(super) fn submit(&self, state: State) {
        // A save is already running or queued; only the latest pending state matters.
        self.enqueue(state, None);
    }

    pub(super) fn checkpoint(&self, state: State) -> Receiver<Result<(), String>> {
        let (tx, rx) = async_channel::bounded(1);
        self.enqueue(state, Some(tx));
        rx
    }

    fn enqueue(&self, state: State, acknowledgment: Option<Sender<Result<(), String>>>) {
        let sequence = self.sequence.get() + 1;
        self.sequence.set(sequence);
        if let Some(sender) = acknowledgment {
            self.acknowledgments
                .lock()
                .expect("snapshot writer lock")
                .push_back((sequence, sender));
        }
        if self
            .snapshots
            .force_send(Snapshot { sequence, state })
            .is_err()
        {
            acknowledge(
                &self.acknowledgments,
                sequence,
                Err("Workspace writer stopped".into()),
            );
        }
    }

    pub(super) fn close(&self) {
        self.snapshots.close();
    }

    pub(super) fn wait_until(&self, deadline: Instant) -> bool {
        self.done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }
}

/// What the writer wakes up for.
enum Wake {
    /// A newer state to save; it holds everything an unsaved older one did. Boxed: the other
    /// wake-ups carry nothing.
    Snapshot(Box<Snapshot>),
    /// The wait after a failed save is over.
    Retry,
    /// The sender was closed and its last snapshot taken: the session is quitting.
    Closed,
}

/// Saves snapshots in the order they arrive. A save that fails is announced once, then tried
/// again after each of [`RETRY_DELAYS`] unless a newer snapshot replaces it first. Once those
/// run out it waits for the next change, or for the sender to close, when it gets a last
/// attempt: without it the final change of a session would exist only in the error toast.
async fn write_snapshots<W: Future<Output = ()>>(
    snapshots: Receiver<Snapshot>,
    errors: Sender<String>,
    pending: &Acknowledgments,
    save: impl Fn(&State) -> Result<(), StateError>,
    wait: impl Fn(Duration) -> W,
) {
    // The newest state that is not on disk, and how many times it has been retried.
    let mut unsaved: Option<(State, usize)> = None;
    loop {
        let retry_in = unsaved
            .as_ref()
            .and_then(|(_, retries)| RETRY_DELAYS.get(*retries));
        let wake = match retry_in {
            Some(delay) => next(&snapshots, wait(*delay)).await,
            None => snapshots
                .recv()
                .await
                .map_or(Wake::Closed, |snapshot| Wake::Snapshot(Box::new(snapshot))),
        };
        match wake {
            Wake::Snapshot(snapshot) => {
                let result = save(&snapshot.state).map_err(|error| error.to_string());
                acknowledge(pending, snapshot.sequence, result.clone());
                unsaved = match result {
                    Ok(()) => None,
                    Err(error) => {
                        tracing::error!(%error, "failed to save state");
                        let _ = errors.force_send(format!("{error}. {RETRY_NOTICE}"));
                        Some((snapshot.state, 0))
                    }
                };
            }
            Wake::Retry => {
                if let Some((state, retries)) = unsaved.take() {
                    unsaved = retry(&save, state).map(|state| (state, retries + 1));
                }
            }
            Wake::Closed => break,
        }
    }
    // Closing the sender drains its last snapshot above; what is still unsaved gets one more
    // attempt before the task, and with it the quit, finishes.
    if let Some((state, _)) = unsaved {
        retry(&save, state);
    }
}

/// The next snapshot, or `retry` running out first. A snapshot wins when both are ready.
async fn next(snapshots: &Receiver<Snapshot>, retry: impl Future<Output = ()>) -> Wake {
    let mut snapshot = pin!(snapshots.recv());
    let mut retry = pin!(retry);
    std::future::poll_fn(|cx| {
        if let Poll::Ready(received) = snapshot.as_mut().poll(cx) {
            return Poll::Ready(
                received.map_or(Wake::Closed, |snapshot| Wake::Snapshot(Box::new(snapshot))),
            );
        }
        retry.as_mut().poll(cx).map(|()| Wake::Retry)
    })
    .await
}

/// Tries once more to save `state`; hands it back when that fails too.
fn retry(save: impl Fn(&State) -> Result<(), StateError>, state: State) -> Option<State> {
    match save(&state) {
        Ok(()) => {
            tracing::info!("state saved on a later attempt");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "saving state failed again");
            Some(state)
        }
    }
}

// A later durable snapshot also acknowledges any earlier snapshots it replaced in the queue.
fn acknowledge(pending: &Acknowledgments, sequence: u64, result: Result<(), String>) {
    let mut queue = pending.lock().expect("snapshot writer lock");
    while queue
        .front()
        .is_some_and(|(through, _)| *through <= sequence)
    {
        if let Some((_, sender)) = queue.pop_front() {
            let _ = sender.try_send(result.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Wake, Waker};

    fn run(future: impl Future<Output = ()>) {
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        while future.as_mut().poll(&mut context).is_pending() {
            std::thread::park();
        }
    }

    #[test]
    fn slow_writer_coalesces_changes_and_drains_last_snapshot_after_error() {
        let (sender, receiver) = async_channel::bounded(1);
        let (errors, error_receiver) = async_channel::bounded(1);
        let (entered_sender, entered) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let saved = Arc::new(Mutex::new(Vec::new()));
        let written = saved.clone();
        let acknowledgments = Acknowledgments::default();
        let (first, first_result) = async_channel::bounded(1);
        let (middle, middle_result) = async_channel::bounded(1);
        acknowledgments
            .lock()
            .unwrap()
            .extend([(300, first), (320, middle)]);
        let worker = std::thread::spawn(move || {
            run(write_snapshots(
                receiver,
                errors,
                &acknowledgments,
                |state| {
                    written
                        .lock()
                        .unwrap()
                        .push(state.settings.git_panel_width());
                    if written.lock().unwrap().len() == 1 {
                        entered_sender.send(()).unwrap();
                        resumed.recv().unwrap();
                        return Err(failure());
                    }
                    Ok(())
                },
                never,
            ));
        });
        let mut state = State::default();
        state.settings.set_git_panel_width(300.);
        assert!(
            sender
                .force_send(Snapshot {
                    sequence: 300,
                    state: state.clone()
                })
                .is_ok()
        );
        entered.recv().unwrap();
        for width in 301..400 {
            state.settings.set_git_panel_width(width as f32);
            assert!(
                sender
                    .force_send(Snapshot {
                        sequence: width,
                        state: state.clone()
                    })
                    .is_ok()
            );
        }
        assert_eq!(sender.len(), 1);
        sender.close();
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(*saved.lock().unwrap(), vec![300., 399.]);
        assert!(first_result.try_recv().unwrap().is_err());
        assert_eq!(middle_result.try_recv().unwrap(), Ok(()));
        assert!(
            error_receiver
                .try_recv()
                .unwrap()
                .contains("injected failure")
        );
    }

    fn failure() -> StateError {
        StateError::Io {
            operation: "write",
            path: "/test/state.json".into(),
            source: std::io::Error::other("injected failure"),
        }
    }

    /// A wait that never ends: only a new snapshot or closing moves the writer on.
    fn never(_: Duration) -> std::future::Pending<()> {
        std::future::pending()
    }

    /// A writer under test. Every attempt is announced with the width it tried to save, and
    /// every wait it asks for is recorded, so no test reads a clock.
    struct Attempts {
        sender: Sender<Snapshot>,
        attempted: mpsc::Receiver<f32>,
        waits: Arc<Mutex<Vec<Duration>>>,
        errors: Receiver<String>,
        worker: std::thread::JoinHandle<()>,
    }

    /// `fails` is asked about each attempt by its number, from 1. A `patient` writer's waits
    /// never end, so only a new snapshot or closing moves it on; otherwise they end at once.
    fn attempts(fails: impl Fn(usize) -> bool + Send + 'static, patient: bool) -> Attempts {
        let (sender, receiver) = async_channel::bounded(1);
        let (errors, error_receiver) = async_channel::bounded(1);
        let (announce, attempted) = mpsc::channel();
        let waits = Arc::new(Mutex::new(Vec::new()));
        let recorded = waits.clone();
        let count = std::sync::atomic::AtomicUsize::new(0);
        let worker = std::thread::spawn(move || {
            run(write_snapshots(
                receiver,
                errors,
                &Acknowledgments::default(),
                |state| {
                    let attempt = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    let result = if fails(attempt) {
                        Err(failure())
                    } else {
                        Ok(())
                    };
                    announce.send(state.settings.git_panel_width()).unwrap();
                    result
                },
                |delay| {
                    recorded.lock().unwrap().push(delay);
                    async move {
                        if patient {
                            std::future::pending::<()>().await
                        }
                    }
                },
            ));
        });
        Attempts {
            sender,
            attempted,
            waits,
            errors: error_receiver,
            worker,
        }
    }

    impl Attempts {
        /// The width of the next attempt. A writer that makes none fails the test instead of
        /// hanging it; the bound is never waited out by one that works.
        fn next(&self) -> f32 {
            self.attempted
                .recv_timeout(Duration::from_secs(30))
                .expect("the writer makes another attempt")
        }

        fn submit(&self, sequence: u64) {
            let mut state = State::default();
            state.settings.set_git_panel_width(sequence as f32);
            assert!(self.sender.force_send(Snapshot { sequence, state }).is_ok());
        }

        /// Closes the writer as quitting does and returns what it tried after that.
        fn quit(self) -> Vec<f32> {
            self.sender.close();
            self.worker.join().unwrap();
            self.attempted.try_iter().collect()
        }
    }

    #[test]
    fn a_failed_save_is_tried_again_after_each_wait_and_once_more_on_quit() {
        let writer = attempts(|_| true, false);
        writer.submit(300);
        // The first attempt and one after each wait: the same change every time.
        for _ in 0..=RETRY_DELAYS.len() {
            assert_eq!(writer.next(), 300.);
        }
        let (waits, errors) = (writer.waits.clone(), writer.errors.clone());
        // Out of waits, nothing more is tried however long the session goes on: closing is
        // the only thing that produces another attempt, and it produces exactly one.
        assert_eq!(writer.quit(), [300.]);
        assert_eq!(*waits.lock().unwrap(), RETRY_DELAYS);
        // One notice for the change, not one per attempt.
        assert!(errors.try_recv().unwrap().contains("injected failure"));
        assert!(errors.try_recv().is_err());
    }

    #[test]
    fn a_retry_that_saves_ends_the_retries_and_quit_writes_nothing_more() {
        let writer = attempts(|attempt| attempt <= 2, false);
        writer.submit(300);
        for _ in 0..3 {
            assert_eq!(writer.next(), 300.);
        }
        let waits = writer.waits.clone();
        assert!(writer.quit().is_empty());
        assert_eq!(*waits.lock().unwrap(), RETRY_DELAYS[..2]);
    }

    #[test]
    fn a_newer_change_replaces_the_one_that_could_not_be_saved() {
        let writer = attempts(|attempt| attempt == 1, true);
        writer.submit(300);
        assert_eq!(writer.next(), 300.);
        writer.submit(301);
        assert_eq!(writer.next(), 301.);
        // The newer state holds everything the older one did: 300 is not written again.
        assert!(writer.quit().is_empty());
    }

    #[test]
    fn quitting_during_a_wait_saves_the_change_at_once() {
        let writer = attempts(|attempt| attempt == 1, true);
        writer.submit(300);
        assert_eq!(writer.next(), 300.);
        assert_eq!(writer.quit(), [300.]);
    }
}
