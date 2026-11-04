//! Coding agents running in Hive's terminals, grouped by the worktree each terminal opened in.
//!
//! Every terminal that starts in a worktree is watched. Once per `POLL_INTERVAL` one process
//! table snapshot (in the background) tells which terminals run an agent in the foreground;
//! their output timing tells whether it is working. The sidebar observes this entity.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::*;
use hive_core::agent::{AgentKind, AgentStatus, ProcessTable};
use hive_core::terminal::{Activity, Terminal};

use crate::state::terminal_session::TerminalSession;

/// Short enough that a spinner starts soon after the agent does, long enough that one `ps`
/// per tick stays negligible.
const POLL_INTERVAL: Duration = Duration::from_millis(800);

/// One agent in one terminal pane.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AgentSession {
    /// The `TerminalSession` it runs in; [`Agents::reveal`] takes it back.
    pub(crate) session: EntityId,
    pub(crate) kind: AgentKind,
    pub(crate) status: AgentStatus,
    /// The title the agent gave its terminal, usually its current task.
    pub(crate) title: Option<SharedString>,
}

pub(crate) enum AgentsEvent {
    /// Show and focus the pane of this session.
    Reveal(EntityId),
}

pub(crate) struct Agents {
    watched: Vec<Watched>,
    by_worktree: HashMap<PathBuf, Vec<AgentSession>>,
    /// The last poll failure, logged once instead of on every tick.
    last_error: Option<String>,
    _poll: Task<()>,
}

struct Watched {
    session: WeakEntity<TerminalSession>,
    id: EntityId,
    probe: Probe,
}

/// What the background poll needs to know about one terminal.
#[derive(Clone)]
struct Probe {
    worktree: PathBuf,
    shell_pid: u32,
    activity: Activity,
}

/// A terminal's agent as seen by the poll, before titles are attached on the UI thread.
struct Sighting {
    id: EntityId,
    worktree: PathBuf,
    kind: AgentKind,
    status: AgentStatus,
}

impl EventEmitter<AgentsEvent> for Agents {}

impl Agents {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Ok(probes) = this.update(cx, |this, cx| this.probes(cx)) else {
                    return;
                };
                let sightings = if probes.is_empty() {
                    Ok(Vec::new())
                } else {
                    cx.background_executor()
                        .spawn(async move { look(&probes) })
                        .await
                };
                if this
                    .update(cx, |this, cx| this.apply(sightings, cx))
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            watched: Vec::new(),
            by_worktree: HashMap::new(),
            last_error: None,
            _poll: poll,
        }
    }

    /// Starts watching a terminal that opened in `worktree`.
    pub(crate) fn watch(
        &mut self,
        session: &Entity<TerminalSession>,
        worktree: &Path,
        terminal: &Terminal,
    ) {
        self.watched.push(Watched {
            session: session.downgrade(),
            id: session.entity_id(),
            probe: Probe {
                worktree: worktree.to_path_buf(),
                shell_pid: terminal.shell_pid(),
                activity: terminal.activity(),
            },
        });
    }

    /// Agents in terminals opened in `worktree`, in the order the terminals were opened.
    pub(crate) fn sessions(&self, worktree: &Path) -> &[AgentSession] {
        self.by_worktree.get(worktree).map_or(&[], Vec::as_slice)
    }

    /// Asks the workspace to show the pane running `session`.
    pub(crate) fn reveal(&mut self, session: EntityId, cx: &mut Context<Self>) {
        cx.emit(AgentsEvent::Reveal(session));
    }

    /// Forgets closed terminals and returns what the poll needs for the rest.
    fn probes(&mut self, cx: &App) -> Vec<(EntityId, Probe)> {
        self.watched.retain(|watched| {
            watched
                .session
                .upgrade()
                .is_some_and(|session| !session.read(cx).exited)
        });
        self.watched
            .iter()
            .map(|watched| (watched.id, watched.probe.clone()))
            .collect()
    }

    fn apply(&mut self, sightings: Result<Vec<Sighting>, String>, cx: &mut Context<Self>) {
        let sightings = match sightings {
            Ok(sightings) => {
                self.last_error = None;
                sightings
            }
            Err(error) => {
                if self.last_error.as_ref() != Some(&error) {
                    tracing::warn!(%error, "could not read agent processes");
                    self.last_error = Some(error);
                }
                return;
            }
        };
        let mut by_worktree: HashMap<PathBuf, Vec<AgentSession>> = HashMap::new();
        for sighting in sightings {
            let title = self
                .watched
                .iter()
                .find(|watched| watched.id == sighting.id)
                .and_then(|watched| watched.session.upgrade())
                .and_then(|session| session.read(cx).title.clone())
                .map(SharedString::from);
            by_worktree
                .entry(sighting.worktree)
                .or_default()
                .push(AgentSession {
                    session: sighting.id,
                    kind: sighting.kind,
                    status: sighting.status,
                    title,
                });
        }
        if self.by_worktree != by_worktree {
            self.by_worktree = by_worktree;
            cx.notify();
        }
    }
}

/// Blocking: one process table snapshot resolves every terminal.
fn look(probes: &[(EntityId, Probe)]) -> Result<Vec<Sighting>, String> {
    let table = ProcessTable::snapshot().map_err(|error| error.to_string())?;
    let now = Instant::now();
    Ok(probes
        .iter()
        .filter_map(|(id, probe)| {
            let kind = table.foreground_agent(probe.shell_pid)?;
            Some(Sighting {
                id: *id,
                worktree: probe.worktree.clone(),
                kind,
                status: AgentStatus::of(&probe.activity, now),
            })
        })
        .collect())
}
