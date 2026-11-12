//! Coding agents running in Hive's terminals, grouped by the worktree each terminal opened in.
//!
//! Every terminal that starts in a worktree is watched. Once per `POLL_INTERVAL` one process
//! table snapshot (in the background) tells which terminals run an agent in the foreground.
//! What the agent is doing comes from its own reports when it sends them (Claude Code hooks,
//! OSC 7501), otherwise from its output timing. The sidebar observes this entity.
use hive_core::host::Host;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use hive_core::agent::{
    AgentKind, AgentStatus, ProcessTable,
    api::{self as protocol, HelperEvent},
    conversation, hooks,
    launch::{Launch, ResumeSession},
};
use hive_core::terminal::{Activity, ProgramStatus, Terminal};

use crate::state::terminal_session::{Message, TerminalSession};

mod api;
mod conversations;
mod endpoint;
mod history;
mod installed;

pub(crate) use api::SpawnRequest;
pub(crate) use endpoint::{Endpoint, PaneEndpoint};

/// Short enough that a spinner starts soon after the agent does, long enough that one process
/// table snapshot per tick stays negligible.
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
    /// The name `hive agent start` gave its pane.
    pub(crate) name: Option<SharedString>,
}

pub(crate) enum AgentsEvent {
    /// Show and focus the pane of this session.
    Reveal(EntityId),
    Attention(EntityId, AgentStatus),
    Launch(Host, PathBuf, Option<Launch>),
    /// `hive agent start`: open a named agent beside the caller's pane; the workspace answers
    /// with [`Agents::started`].
    Spawn(SpawnRequest),
}

pub(crate) struct Agents {
    watched: Vec<Watched>,
    by_worktree: HashMap<(Host, PathBuf), Vec<AgentSession>>,
    /// The last poll failure, logged once instead of on every tick.
    last_error: Option<String>,
    endpoint: Option<PaneEndpoint>,
    hooks: Option<Arc<hooks::Server>>,
    /// `hive agent wait` and `send --wait` calls, answered when the agent settles.
    waits: Vec<api::Waiting>,
    /// `hive agent start` calls, answered once the new agent runs.
    starts: Vec<api::Starting>,
    /// Launchable agent CLIs found on this Mac at the last scan.
    installed: Vec<AgentKind>,
    _installed_scan: Task<()>,
    _poll: Task<()>,
    _serve: Option<Task<()>>,
}

struct Watched {
    session: WeakEntity<TerminalSession>,
    id: EntityId,
    probe: Probe,
    /// The terminal program's OSC 7501 reports, collected with the hook reports.
    program: ProgramStatus,
    /// The agent's own last report, from hooks or OSC 7501; it beats the output estimate.
    reported: Option<AgentStatus>,
    /// The agent the last poll saw here. Another one, or none, means `reported` and `helper`
    /// described an agent that is gone.
    sighted: Option<AgentKind>,
    unread: bool,
    helper: Option<Helper>,
}

/// An agent started by another one with `hive agent start`; its parent hears when it is
/// blocked, fails or exits.
#[derive(Clone)]
struct Helper {
    parent: EntityId,
    name: String,
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
    /// `endpoint` is bound before the app state exists, so the first panes already get it.
    pub(crate) fn new(endpoint: Option<Endpoint>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Ok(probes) = this.update(cx, |this, cx| this.probes(cx)) else {
                    return;
                };
                let sightings = cx
                    .background_executor()
                    .spawn(async move {
                        if probes.is_empty() {
                            Ok(Vec::new())
                        } else {
                            look(&probes)
                        }
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh_reports(cx);
                        this.apply(sightings, cx);
                        this.settle_calls(cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        let (pane, hooks, calls) = match endpoint {
            Some(endpoint) => (Some(endpoint.pane), Some(endpoint.hooks), endpoint.calls),
            None => (None, None, None),
        };
        let serve = calls.map(|calls| {
            cx.spawn(async move |this, cx| {
                while let Ok(incoming) = calls.recv().await {
                    if this
                        .update(cx, |this, cx| this.handle(incoming, cx))
                        .is_err()
                    {
                        return;
                    }
                }
            })
        });
        let mut agents = Self {
            watched: Vec::new(),
            by_worktree: HashMap::new(),
            last_error: None,
            endpoint: pane,
            hooks,
            waits: Vec::new(),
            starts: Vec::new(),
            installed: Vec::new(),
            _installed_scan: Task::ready(()),
            _poll: poll,
            _serve: serve,
        };
        agents.refresh_installed(cx);
        agents
    }

    /// Applies hook reports that arrived since the last call. The socket is nonblocking and
    /// each pass is bounded, so this runs on the UI thread: every report is then applied in
    /// arrival order, also when a message delivery asks for the freshest status.
    pub(crate) fn refresh_reports(&mut self, cx: &mut Context<Self>) {
        if let Some(hooks) = &self.hooks {
            match hooks.drain() {
                Ok(reports) => self.apply_reports(reports, cx),
                Err(error) => tracing::warn!(%error, "could not read agent hooks"),
            }
        }
        let programs: Vec<_> = self
            .watched
            .iter()
            .map(|watched| (watched.id, watched.program.drain()))
            .collect();
        for (id, states) in programs {
            for state in states {
                self.report_status(id, AgentStatus::from_report(state), cx);
            }
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
            program: terminal.program_status(),
            reported: None,
            sighted: None,
            unread: false,
            helper: None,
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

    pub(crate) fn endpoint(&self) -> Option<PaneEndpoint> {
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
            if let Some(status) = report.status {
                let id = watched.id;
                self.report_status(id, Some(status), cx);
            }
        }
    }

    /// Applies a status the agent in terminal `id` reported itself; `None` withdraws it.
    fn report_status(&mut self, id: EntityId, status: Option<AgentStatus>, cx: &mut Context<Self>) {
        if self.live_session(id, cx).is_none() {
            return;
        }
        let Some(watched) = self.watched.iter_mut().find(|watched| watched.id == id) else {
            return;
        };
        let Some(status) = status else {
            watched.reported = None;
            return;
        };
        let previous = watched.reported;
        if update_status(&mut watched.reported, &mut watched.unread, status) {
            cx.emit(AgentsEvent::Attention(id, status));
        }
        let helper = watched.helper.clone();
        self.note_status(id, status);
        if let (Some(helper), Some(event)) = (helper, helper_event(previous, status)) {
            self.tell_parent(&helper, event, cx);
        }
    }

    /// Types the note about `helper` into its parent's prompt, if the parent still runs an
    /// agent. Nobody waits for the outcome, so a failed delivery is only logged.
    fn tell_parent(&self, helper: &Helper, event: HelperEvent, cx: &mut Context<Self>) {
        let Some(session) = self.live_session(helper.parent, cx) else {
            return;
        };
        let Some(parent) = self.agent(helper.parent) else {
            return;
        };
        let message = Message {
            agent: parent.kind,
            text: protocol::helper_report(&helper.name, event),
            // A busy parent takes the note only after its turn.
            deadline: Instant::now() + protocol::WAIT_TIMEOUT,
            done: Box::new(|result, _| {
                if let Err(error) = result {
                    tracing::info!(%error, "helper report not delivered");
                }
            }),
        };
        session.update(cx, |session, cx| session.queue_message(message, cx));
    }

    /// Forgets closed terminals and returns what the poll needs for the rest.
    fn probes(&mut self, cx: &mut Context<Self>) -> Vec<(EntityId, Probe)> {
        let (closed, open): (Vec<_>, Vec<_>) = std::mem::take(&mut self.watched)
            .into_iter()
            .partition(|watched| {
                watched
                    .session
                    .upgrade()
                    .is_none_or(|session| session.read(cx).exited)
            });
        self.watched = open;
        for watched in closed {
            if let Some(helper) = watched.helper.filter(|_| watched.sighted.is_some()) {
                self.tell_parent(&helper, HelperEvent::Exited, cx);
            }
        }
        self.watched
            .iter()
            .map(|watched| (watched.id, watched.probe.clone()))
            .collect()
    }

    /// Drops what belonged to an agent that left its terminal: its reports and its parent.
    fn note_sightings(
        &mut self,
        seen: &HashMap<EntityId, (AgentKind, Option<ResumeSession>)>,
        cx: &mut Context<Self>,
    ) {
        let mut gone = Vec::new();
        for watched in &mut self.watched {
            let kind = seen.get(&watched.id).map(|(kind, _)| *kind);
            if watched.sighted.is_some() && watched.sighted != kind {
                watched.reported = None;
                gone.extend(watched.helper.take());
            }
            watched.sighted = kind;
        }
        for helper in gone {
            self.tell_parent(&helper, HelperEvent::Exited, cx);
        }
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
        self.note_sightings(&seen, cx);
        let mut by_worktree: HashMap<(Host, PathBuf), Vec<AgentSession>> = HashMap::new();
        for sighting in sightings {
            let session = self
                .watched
                .iter()
                .find(|watched| watched.id == sighting.id)
                .and_then(|watched| watched.session.upgrade());
            let (title, name) = session.map_or((None, None), |session| {
                let session = session.read(cx);
                (
                    session.title.clone().map(SharedString::from),
                    session.name.clone().map(SharedString::from),
                )
            });
            let watched = self.watched.iter().find(|w| w.id == sighting.id);
            let reported = watched.and_then(|w| w.reported);
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
                    name,
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

/// Whether reaching `status` from `previous` is news for the user: a finished or failed turn,
/// a permission prompt, or a question. A question counts only in the middle of a turn:
/// Antigravity reports `blocked:auth` while it checks its sign-in on start, and Claude Code
/// asks for input when it has sat idle after finishing.
fn is_news(previous: Option<AgentStatus>, status: AgentStatus) -> bool {
    match status {
        AgentStatus::WaitingInput => previous == Some(AgentStatus::Working),
        AgentStatus::WaitingApproval | AgentStatus::Done | AgentStatus::Failed => true,
        AgentStatus::Working | AgentStatus::Idle => false,
    }
}

/// What a helper's parent hears about a reported status change.
fn helper_event(previous: Option<AgentStatus>, status: AgentStatus) -> Option<HelperEvent> {
    if previous == Some(status) || !is_news(previous, status) {
        return None;
    }
    match status {
        AgentStatus::WaitingApproval | AgentStatus::WaitingInput => Some(HelperEvent::Blocked),
        AgentStatus::Failed => Some(HelperEvent::Failed),
        AgentStatus::Done | AgentStatus::Working | AgentStatus::Idle => None,
    }
}

/// Records `status` and returns whether it is news worth a notification.
fn update_status(
    current: &mut Option<AgentStatus>,
    unread: &mut bool,
    status: AgentStatus,
) -> bool {
    if *current == Some(status) {
        return false;
    }
    let news = is_news(current.replace(status), status);
    // A status that is no news on its own, such as Claude's idle prompt after `Done`, keeps the
    // mark of the turn the user has not looked at yet.
    *unread = news || (*unread && status.needs_attention());
    news
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, HelperEvent, helper_event, update_status};

    #[test]
    fn parents_hear_about_blocked_and_failed_helpers_once_but_not_finished_turns() {
        use AgentStatus::*;
        let working = Some(Working);
        assert_eq!(
            helper_event(working, WaitingApproval),
            Some(HelperEvent::Blocked)
        );
        assert_eq!(
            helper_event(None, WaitingApproval),
            Some(HelperEvent::Blocked)
        );
        assert_eq!(helper_event(Some(WaitingApproval), WaitingApproval), None);
        assert_eq!(
            helper_event(working, WaitingInput),
            Some(HelperEvent::Blocked)
        );
        assert_eq!(helper_event(Some(Done), WaitingInput), None);
        assert_eq!(helper_event(working, Failed), Some(HelperEvent::Failed));
        assert_eq!(helper_event(working, Done), None);
        assert_eq!(helper_event(working, Idle), None);
        assert_eq!(helper_event(Some(Idle), Working), None);
    }

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

    #[test]
    fn a_sign_in_check_on_start_is_not_news() {
        // Antigravity's start, as captured from `agy`: `blocked:kind=auth`, then `idle`.
        let mut status = None;
        let mut unread = false;
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingInput
        ));
        assert!(!unread);
        assert!(!update_status(&mut status, &mut unread, AgentStatus::Idle));
        assert!(!unread);
    }

    #[test]
    fn a_question_in_the_middle_of_a_turn_is_news() {
        let mut status = Some(AgentStatus::Working);
        let mut unread = false;
        assert!(update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingInput
        ));
        assert!(unread);
    }

    #[test]
    fn an_idle_prompt_after_a_finished_turn_keeps_its_mark_without_news() {
        let mut status = Some(AgentStatus::Working);
        let mut unread = false;
        assert!(update_status(&mut status, &mut unread, AgentStatus::Done));
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingInput
        ));
        assert!(unread);
        unread = false;
        assert!(!update_status(&mut status, &mut unread, AgentStatus::Idle));
        assert!(!update_status(
            &mut status,
            &mut unread,
            AgentStatus::WaitingInput
        ));
        assert!(!unread);
    }
}
