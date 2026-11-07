//! Coding agents running in Hive's terminals, grouped by the worktree each terminal opened in.
//!
//! Every terminal that starts in a worktree is watched. Once per `POLL_INTERVAL` one process
//! table snapshot (in the background) tells which terminals run an agent in the foreground;
//! their output timing tells whether it is working. The sidebar observes this entity.
use hive_core::host::Host;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use hive_core::agent::{
    AgentKind, AgentStatus, ProcessTable, conversation, hooks,
    launch::{Launch, ResumeSession},
};
use hive_core::terminal::{Activity, Terminal};

use crate::state::terminal_session::TerminalSession;

mod conversations;

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
    pub(crate) reported: bool,
    pub(crate) unread: bool,
    /// The title the agent gave its terminal, usually its current task.
    pub(crate) title: Option<SharedString>,
}

pub(crate) enum AgentsEvent {
    /// Show and focus the pane of this session.
    Reveal(EntityId),
    Attention(EntityId, AgentStatus),
    Launch(Host, PathBuf, Option<Launch>),
}

pub(crate) struct Agents {
    watched: Vec<Watched>,
    by_worktree: HashMap<(Host, PathBuf), Vec<AgentSession>>,
    /// The last poll failure, logged once instead of on every tick.
    last_error: Option<String>,
    endpoint: Option<PathBuf>,
    _poll: Task<()>,
}

struct Watched {
    session: WeakEntity<TerminalSession>,
    id: EntityId,
    probe: Probe,
    reported: Option<AgentStatus>,
    unread: bool,
}

/// What the background poll needs to know about one terminal.
#[derive(Clone)]
struct Probe {
    host: Host,
    launched_agent: Option<AgentKind>,
    worktree: PathBuf,
    shell_pid: u32,
    activity: Activity,
}

/// A terminal's agent as seen by the poll, before titles are attached on the UI thread.
struct Sighting {
    host: Host,
    id: EntityId,
    worktree: PathBuf,
    kind: AgentKind,
    status: AgentStatus,
    /// The conversation the agent runs, once its CLI has recorded one.
    conversation: Option<ResumeSession>,
}

impl EventEmitter<AgentsEvent> for Agents {}

impl Agents {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            let server = cx
                .background_executor()
                .spawn(async { hooks::Server::bind().map(Arc::new) })
                .await;
            let server = match server {
                Ok(server) => {
                    let path = server.path().to_owned();
                    if this
                        .update(cx, |this, _| this.endpoint = Some(path))
                        .is_err()
                    {
                        return;
                    }
                    Some(server)
                }
                Err(error) => {
                    tracing::error!(%error, "agent hooks unavailable; using estimated status");
                    None
                }
            };
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Ok(probes) = this.update(cx, |this, cx| this.probes(cx)) else {
                    return;
                };
                let server = server.clone();
                let (sightings, reports) = cx
                    .background_executor()
                    .spawn(async move {
                        let reports = server.map(|server| server.drain()).transpose();
                        let sightings = if probes.is_empty() {
                            Ok(Vec::new())
                        } else {
                            look(&probes)
                        };
                        (sightings, reports)
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        match reports {
                            Ok(Some(reports)) => this.apply_reports(reports, cx),
                            Err(error) => tracing::warn!(%error, "could not read agent hooks"),
                            _ => {}
                        }
                        this.apply(sightings, cx)
                    })
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
            endpoint: None,
            _poll: poll,
        }
    }

    /// Starts watching a terminal that opened in `worktree`.
    pub(crate) fn watch(
        &mut self,
        session: &Entity<TerminalSession>,
        host: &Host,
        launched_agent: Option<AgentKind>,
        worktree: &Path,
        terminal: &Terminal,
    ) {
        self.watched
            .retain(|watched| watched.id != session.entity_id());
        self.watched.push(Watched {
            reported: None,
            unread: false,
            session: session.downgrade(),
            id: session.entity_id(),
            probe: Probe {
                host: host.clone(),
                launched_agent,
                worktree: worktree.to_path_buf(),
                shell_pid: terminal.shell_pid(),
                activity: terminal.activity(),
            },
        });
    }

    /// Agents in terminals opened in `worktree`, in the order the terminals were opened.
    pub(crate) fn sessions(&self, host: &Host, worktree: &Path) -> &[AgentSession] {
        self.by_worktree
            .get(&(host.clone(), worktree.to_path_buf()))
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn preferred_session(
        &self,
        host: &Host,
        worktree: &Path,
        kind: AgentKind,
        cx: &App,
    ) -> Option<Entity<TerminalSession>> {
        self.sessions(host, worktree)
            .iter()
            .enumerate()
            .filter(|(_, session)| session.kind == kind)
            .filter_map(|(index, session)| {
                let terminal = self
                    .watched
                    .iter()
                    .find(|watched| watched.id == session.session)?
                    .session
                    .upgrade()?;
                (!terminal.read(cx).exited).then_some((index, terminal))
            })
            .max_by_key(|(index, terminal)| {
                (terminal.read(cx).last_focused, std::cmp::Reverse(*index))
            })
            .map(|(_, terminal)| terminal)
    }

    /// Asks the workspace to show the pane running `session`.
    pub(crate) fn reveal(&mut self, session: EntityId, cx: &mut Context<Self>) {
        for watched in &mut self.watched {
            if watched.id == session {
                watched.unread = false;
            }
        }
        for sessions in self.by_worktree.values_mut() {
            for agent in sessions {
                if agent.session == session {
                    agent.unread = false;
                }
            }
        }
        cx.notify();
        cx.emit(AgentsEvent::Reveal(session));
    }

    pub(crate) fn start(
        &mut self,
        host: Host,
        worktree: PathBuf,
        launch: Option<Launch>,
        cx: &mut Context<Self>,
    ) {
        cx.emit(AgentsEvent::Launch(host, worktree, launch));
    }

    pub(crate) fn endpoint(&self) -> Option<PathBuf> {
        self.endpoint.clone()
    }

    pub(crate) fn all(&self) -> impl Iterator<Item = (&(Host, PathBuf), &AgentSession)> {
        self.by_worktree
            .iter()
            .flat_map(|(path, sessions)| sessions.iter().map(move |session| (path, session)))
    }

    fn apply_reports(&mut self, reports: Vec<hooks::Report>, cx: &mut Context<Self>) {
        for report in reports {
            let Some(watched) = self
                .watched
                .iter_mut()
                .find(|w| w.id.as_u64() == report.terminal)
            else {
                continue;
            };
            let Some(session) = watched.session.upgrade() else {
                continue;
            };
            if session.read(cx).exited {
                continue;
            }
            if let Some(id) = report
                .session
                .and_then(|id| ResumeSession::new(AgentKind::ClaudeCode, &id))
            {
                session.update(cx, |session, cx| session.set_resume(Some(id), cx));
            }
            if let Some(status) = report.status
                && update_status(&mut watched.reported, &mut watched.unread, status)
            {
                cx.emit(AgentsEvent::Attention(watched.id, status));
            }
        }
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
        let seen: HashMap<EntityId, (AgentKind, Option<ResumeSession>)> = sightings
            .iter()
            .map(|sighting| (sighting.id, (sighting.kind, sighting.conversation.clone())))
            .collect();
        self.track_conversations(&seen, cx);
        let mut by_worktree: HashMap<(Host, PathBuf), Vec<AgentSession>> = HashMap::new();
        for sighting in sightings {
            let title = self
                .watched
                .iter()
                .find(|watched| watched.id == sighting.id)
                .and_then(|watched| watched.session.upgrade())
                .and_then(|session| session.read(cx).title.clone())
                .map(SharedString::from);
            let watched = self.watched.iter().find(|w| w.id == sighting.id);
            let reported = watched
                .and_then(|w| w.reported)
                .filter(|_| sighting.kind == AgentKind::ClaudeCode);
            by_worktree
                .entry((sighting.host, sighting.worktree))
                .or_default()
                .push(AgentSession {
                    session: sighting.id,
                    kind: sighting.kind,
                    status: reported.unwrap_or(sighting.status),
                    reported: reported.is_some(),
                    unread: reported.is_some() && watched.is_some_and(|w| w.unread),
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
    let table = probes
        .iter()
        .any(|(_, probe)| probe.host.is_local())
        .then(ProcessTable::snapshot)
        .transpose()
        .map_err(|error| error.to_string())?;
    let now = Instant::now();
    let mut sightings: Vec<Sighting> = probes
        .iter()
        .filter_map(|(id, probe)| {
            let kind = if probe.host.is_local() {
                table.as_ref()?.foreground_agent(probe.shell_pid)?
            } else {
                probe.launched_agent?
            };
            let conversation = if probe.host.is_local() {
                conversation::discover(kind, &table.as_ref()?.foreground_pids(probe.shell_pid))
            } else {
                None
            };
            Some(Sighting {
                host: probe.host.clone(),
                id: *id,
                worktree: probe.worktree.clone(),
                kind,
                status: if probe.host.is_local() {
                    AgentStatus::of(&probe.activity, now)
                } else {
                    AgentStatus::Idle
                },
                conversation,
            })
        })
        .collect();
    conversations::forget_shared(
        &mut sightings
            .iter_mut()
            .map(|sighting| &mut sighting.conversation)
            .collect::<Vec<_>>(),
    );
    Ok(sightings)
}

fn update_status(
    current: &mut Option<AgentStatus>,
    unread: &mut bool,
    status: AgentStatus,
) -> bool {
    if *current == Some(status) {
        return false;
    }
    *current = Some(status);
    *unread = status.needs_attention();
    *unread
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, update_status};
    #[test]
    fn repeated_reports_do_not_recreate_dismissed_attention() {
        let mut status = None;
        let mut unread = false;
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::Working
        ));
        assert!(update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingApproval
        ));
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingApproval
        ));
        unread = false;
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingApproval
        ));
        assert!(!unread);
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::Working
        ));
        assert!(update_status(&mut status, &mut unread, AgentStatus::Done));
        assert!(unread);
    }
}
