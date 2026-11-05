use gpui_kit::{BackgroundExecutor, Task};
use hive_core::files::recovery::{self, Draft};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
    time::Instant,
};

pub(super) struct RecoveryWriter {
    sender: async_channel::Sender<()>,
    pending: Arc<Mutex<HashMap<String, Option<Draft>>>>,
    done: mpsc::Receiver<()>,
    _task: Task<()>,
}

impl RecoveryWriter {
    pub(super) fn new(executor: &BackgroundExecutor) -> (Self, async_channel::Receiver<String>) {
        let (sender, receiver) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let snapshots = pending.clone();
        let (errors, error_receiver) = async_channel::bounded(1);
        let (finished, done) = mpsc::channel();
        let task = executor.spawn(async move {
            let directory = hive_core::state::hive_dir().join("editor-recovery");
            drain(receiver, snapshots, |key, draft| {
                let result = match draft {
                    Some(draft) => recovery::write(&directory, &key, &draft),
                    None => recovery::remove(&directory, &key),
                };
                if let Err(error) = result {
                    tracing::error!(%error, "editor recovery failed");
                    let _ = errors.force_send(error.to_string());
                }
            })
            .await;
            let _ = finished.send(());
        });
        (
            Self {
                sender,
                pending,
                done,
                _task: task,
            },
            error_receiver,
        )
    }

    pub(super) fn submit(&self, key: String, draft: Option<Draft>) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, draft);
        if self.sender.force_send(()).is_err() {
            tracing::error!("recovery writer is closed");
        }
    }

    pub(super) fn close(&self) {
        self.sender.close();
    }

    pub(super) fn wait_until(&self, deadline: Instant) -> bool {
        self.done
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .is_ok()
    }
}

async fn drain(
    wake: async_channel::Receiver<()>,
    pending: Arc<Mutex<HashMap<String, Option<Draft>>>>,
    mut save: impl FnMut(String, Option<Draft>),
) {
    while wake.recv().await.is_ok() {
        let batch = std::mem::take(&mut *pending.lock().unwrap_or_else(|e| e.into_inner()));
        for (key, draft) in batch {
            save(key, draft);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Wake, Waker},
    };

    #[test]
    fn discard_follows_inflight_write_and_replaces_all_queued_snapshots() {
        let (sender, wake) = async_channel::bounded(1);
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let snapshots = pending.clone();
        let (entered_tx, entered) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let root = std::env::temp_dir().join(format!("hive-recovery-queue-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a"), "base").unwrap();
        let draft = Draft {
            host: hive_core::host::Host::Local,
            project: root.clone(),
            root: root.clone(),
            path: "a".into(),
            base: hive_core::files::read(
                &hive_core::host::Host::Local,
                &root,
                std::path::Path::new("a"),
            )
            .unwrap(),
            text: "first".into(),
        };
        std::fs::remove_dir_all(root).unwrap();
        pending
            .lock()
            .unwrap()
            .insert("file".into(), Some(draft.clone()));
        sender.force_send(()).unwrap();
        let worker = std::thread::spawn(move || {
            let mut seen = Vec::new();
            {
                let future = drain(wake, snapshots, |_, draft| {
                    seen.push(draft.map(|d| d.text));
                    if seen.len() == 1 {
                        entered_tx.send(()).unwrap();
                        resumed.recv().unwrap();
                    }
                });
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
            seen
        });
        entered.recv().unwrap();
        for _ in 0..1000 {
            pending
                .lock()
                .unwrap()
                .insert("file".into(), Some(draft.clone()));
            sender.force_send(()).unwrap();
        }
        pending.lock().unwrap().insert("file".into(), None);
        assert_eq!(pending.lock().unwrap().len(), 1);
        sender.close();
        resume.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), vec![Some("first".to_owned()), None]);
    }
}
