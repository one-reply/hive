//! Tracks starting and closing sessions independently of GPUI foreground tasks.
use hive_core::terminal::{Terminal, TerminalControl};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

#[derive(Default)]
pub(crate) struct Terminals {
    state: Mutex<Registry>,
    changed: Condvar,
}

#[derive(Default)]
struct Registry {
    stopping: bool,
    starting: usize,
    controls: Vec<TerminalControl>,
}

impl Terminals {
    pub(crate) fn begin(self: &Arc<Self>) -> Option<StartingSession> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopping {
            return None;
        }
        state.controls.retain(|control| !control.is_closed());
        state.starting += 1;
        Some(StartingSession(self.clone()))
    }

    pub(crate) fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stopping = true;
        for control in &state.controls {
            control.shutdown();
        }
    }

    pub(crate) fn wait_until(&self, deadline: Instant) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (state, _) = self
            .changed
            .wait_timeout_while(
                state,
                deadline.saturating_duration_since(Instant::now()),
                |state| state.starting != 0,
            )
            .unwrap_or_else(|e| e.into_inner());
        if state.starting != 0 {
            return false;
        }
        let controls = state.controls.clone();
        drop(state);
        controls.iter().all(|control| control.wait_until(deadline))
    }
}

pub(crate) struct StartingSession(Arc<Terminals>);

impl StartingSession {
    pub(crate) fn register(&self, terminal: &Terminal) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let control = terminal.control();
        if state.stopping {
            control.shutdown();
        }
        state.controls.push(control);
    }
}

impl Drop for StartingSession {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.starting -= 1;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::terminal::TerminalSize;
    use std::time::Duration;

    #[test]
    fn shutdown_covers_late_start_and_already_closing_session() {
        let registry = Arc::new(Terminals::default());
        let starting = registry.begin().unwrap();
        registry.stop();
        assert!(registry.begin().is_none());
        assert!(!registry.wait_until(Instant::now()));
        let worker = std::thread::spawn(move || {
            let terminal =
                Terminal::spawn(None, Some("/bin/sh"), TerminalSize::new(80, 24)).unwrap();
            let control = terminal.control();
            starting.register(&terminal);
            drop(starting);
            // The terminal can remain in a foreground task's undelivered result.
            assert!(control.wait_until(Instant::now() + Duration::from_secs(3)));
            terminal
        });
        assert!(registry.wait_until(Instant::now() + Duration::from_secs(3)));
        drop(worker.join().unwrap());

        let registry = Arc::new(Terminals::default());
        let starting = registry.begin().unwrap();
        let terminal = Terminal::spawn(None, Some("/bin/sh"), TerminalSize::new(80, 24)).unwrap();
        starting.register(&terminal);
        drop(starting);
        drop(terminal);
        registry.stop();
        assert!(registry.wait_until(Instant::now() + Duration::from_secs(3)));
    }
}
