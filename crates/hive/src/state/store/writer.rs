use async_channel::{Receiver, Sender};
use gpui_kit::{BackgroundExecutor, Task};
use hive_core::state::{State, StateError};
use std::sync::mpsc;
use std::time::Instant;

pub(super) struct Writer {
    snapshots: Sender<State>,
    done: mpsc::Receiver<()>,
    _task: Task<()>,
}

impl Writer {
    pub(super) fn new(executor: &BackgroundExecutor) -> (Self, Receiver<String>) {
        let (snapshots, receiver) = async_channel::bounded(1);
        let (errors, error_receiver) = async_channel::bounded(1);
        let (done_sender, done) = mpsc::channel();
        let task = executor.spawn(async move {
            write_snapshots(receiver, errors, State::save).await;
            let _ = done_sender.send(());
        });
        (
            Self {
                snapshots,
                done,
                _task: task,
            },
            error_receiver,
        )
    }

    pub(super) fn submit(&self, state: State) {
        // A save is already running or queued; only the latest pending state matters.
        let _ = self.snapshots.force_send(state);
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
    snapshots: Receiver<State>,
    errors: Sender<String>,
    save: impl Fn(&State) -> Result<(), StateError>,
) {
    // Closing the sender drains its last snapshot before this task finishes.
    while let Ok(state) = snapshots.recv().await {
        if let Err(error) = save(&state) {
            tracing::error!(%error, "failed to save state");
            let _ = errors.force_send(error.to_string());
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
        let worker = std::thread::spawn(move || {
            run(write_snapshots(receiver, errors, |state| {
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
            }));
        });
        let mut state = State::default();
        state.settings.set_git_panel_width(300.);
        sender.force_send(state.clone()).unwrap();
        entered.recv().unwrap();
        for width in 301..400 {
            state.settings.set_git_panel_width(width as f32);
            sender.force_send(state.clone()).unwrap();
        }
        assert_eq!(sender.len(), 1);
        sender.close();
        resume.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(*saved.lock().unwrap(), vec![300., 399.]);
        assert!(
            error_receiver
                .try_recv()
                .unwrap()
                .contains("injected failure")
        );
    }
}
