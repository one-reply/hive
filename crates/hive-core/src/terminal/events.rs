use alacritty_terminal::event::Event;
use async_channel::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Latest terminal state. Exit is sticky even when subsequent wakeups replace a snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalUpdate {
    title: Option<String>,
    exited: bool,
}

impl TerminalUpdate {
    /// The latest shell title, or `None` after a title reset.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    /// Whether the terminal has permanently exited.
    pub fn exited(&self) -> bool {
        self.exited
    }
}

#[derive(Clone)]
pub(super) struct Publisher {
    snapshot: Arc<Mutex<TerminalUpdate>>,
    sender: Sender<TerminalUpdate>,
}

impl Publisher {
    pub(super) fn new() -> (Self, Receiver<TerminalUpdate>) {
        let (sender, receiver) = async_channel::bounded(1);
        (
            Self {
                snapshot: Arc::default(),
                sender,
            },
            receiver,
        )
    }

    pub(super) fn send(&self, event: Event) {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            Event::Title(title) => snapshot.title = Some(title),
            Event::ResetTitle => snapshot.title = None,
            Event::Exit | Event::ChildExit(_) => snapshot.exited = true,
            Event::Wakeup | Event::CursorBlinkingChange | Event::MouseCursorDirty => {}
            _ => return,
        }
        // Publish while locked so two producers cannot reverse snapshot order.
        let _ = self.sender.force_send(snapshot.clone());
    }

    pub(super) fn close(&self) {
        self.send(Event::Exit);
        self.sender.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flood_keeps_one_latest_snapshot_and_never_loses_exit() {
        let (publisher, receiver) = Publisher::new();
        for i in 0..100_000 {
            publisher.send(Event::Title(i.to_string()));
            publisher.send(Event::Wakeup);
        }
        assert_eq!(receiver.len(), 1);
        assert_eq!(receiver.try_recv().unwrap().title(), Some("99999"));
        publisher.send(Event::Exit);
        publisher.send(Event::ResetTitle);
        publisher.send(Event::Wakeup);
        let last = receiver.try_recv().unwrap();
        assert!(last.exited());
        assert_eq!(last.title(), None);
        publisher.close();
        assert!(receiver.try_recv().unwrap().exited());
        assert!(receiver.is_closed());
        drop(receiver);
        publisher.send(Event::Title("closed consumer".into()));
    }
}
