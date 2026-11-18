use async_channel::{Receiver, Sender};
use gpui_kit::{BackgroundExecutor, Task};
use hive_core::state::{State, StateError};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

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
    pub(super) fn new(executor: &BackgroundExecutor) -> (Self, Receiver<String>) {
        Self::with_save(executor, State::save)
    }

    pub(super) fn with_save(
        executor: &BackgroundExecutor,
        save: impl Fn(&State) -> Result<(), StateError> + Send + 'static,
    ) -> (Self, Receiver<String>) {
        let (snapshots, receiver) = async_channel::bounded(1);
        let (errors, error_receiver) = async_channel::bounded(1);
        let (done_sender, done) = mpsc::channel();
        let acknowledgments = Acknowledgments::default();
        let pending = acknowledgments.clone();
        let task = executor.spawn(async move {
            write_snapshots(receiver, errors, &pending, save).await;
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

async fn write_snapshots(
    snapshots: Receiver<Snapshot>,
    errors: Sender<String>,
    pending: &Acknowledgments,
    save: impl Fn(&State) -> Result<(), StateError>,
) {
    // Closing the sender drains its last snapshot before this task finishes.
    while let Ok(snapshot) = snapshots.recv().await {
        let result = save(&snapshot.state).map_err(|error| error.to_string());
        acknowledge(pending, snapshot.sequence, result.clone());
        if let Err(error) = result {
            tracing::error!(%error, "failed to save state");
            let _ = errors.force_send(error);
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
                        return Err(StateError::Io {
                            operation: "write",
                            path: "/test/state.json".into(),
                            source: std::io::Error::other("injected failure"),
                        });
                    }
                    Ok(())
                },
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
}
