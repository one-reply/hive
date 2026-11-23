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
    /// When the agent last reported a status.
    reported_at: Option<Instant>,
    /// The agent reported `working` since it appeared here; see [`ended_turn`].
    worked: bool,
    /// Since when the agent's own last report is `idle`.
    idle_since: Option<Instant>,
    /// The agent the last poll saw here. Another one, or none, means `reported` and `helper`
    /// described an agent that is gone.
    sighted: Option<AgentKind>,
    /// When the process table was read for the last poll that saw no agent here.
    unsighted_at: Option<Instant>,
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
    /// The last poll's conversation lookup, reused while it is fresh.
    discovery: Option<conversation::Discovery>,
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
    /// The lookup behind `conversation`, kept for the next poll.
    discovery: Option<conversation::Discovery>,
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
                let (read_at, sightings) = cx
                    .background_executor()
                    .spawn(async move {
                        let read_at = Instant::now();
                        if probes.is_empty() {
                            (read_at, Ok(Vec::new()))
                        } else {
                            (read_at, look(&probes))
                        }
                    })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.refresh_reports(cx);
                        this.settle_turns(Instant::now(), cx);
                        this.apply(sightings, read_at, cx);
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
            reported_at: None,
            worked: false,
            idle_since: None,
            sighted: None,
            unsighted_at: None,
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
                discovery: None,
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

    /// Ends the turn of each pipeline step whose agent went idle and stayed quiet; see
    /// [`ended_turn`]. The agent never says so itself, so the poll does it for it, through the
    /// same path as a reported `done`.
    fn settle_turns(&mut self, now: Instant, cx: &mut Context<Self>) {
        let ended: Vec<EntityId> = self
            .watched
            .iter()
            .filter(|watched| {
                let step = watched.session.upgrade().is_some_and(|session| {
                    let session = session.read(cx);
                    !session.exited && session.report_grant.is_some()
                });
                ended_turn(
                    step,
                    watched.sighted.or(watched.probe.launched_agent),
                    watched.worked,
                    watched.reported,
                    watched
                        .idle_since
                        .map(|since| now.saturating_duration_since(since)),
                    watched.probe.activity.is_streaming(now),
                )
            })
            .map(|watched| watched.id)
            .collect();
        for id in ended {
            self.report_status(id, Some(AgentStatus::Done), cx);
        }
    }

    /// The poll looks at the panes once their agents have been quiet for the whole grace.
    #[cfg(test)]
    pub(crate) fn settle_quiet_turns_for_test(&mut self, cx: &mut Context<Self>) {
        self.settle_turns(Instant::now() + TURN_END_GRACE, cx);
    }

    /// The agent in terminal `id` reports `status`, as its hooks or OSC 7501 would.
    #[cfg(test)]
    pub(crate) fn report_for_test(
        &mut self,
        id: EntityId,
        status: AgentStatus,
        cx: &mut Context<Self>,
    ) {
        self.report_status(id, Some(status), cx);
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
        let Some(session) = self.live_session(id, cx) else {
            return;
        };
        // The pane of a pipeline step, which owes Hive a stage report.
        let step = session.read(cx).report_grant.is_some();
        let Some(watched) = self.watched.iter_mut().find(|watched| watched.id == id) else {
            return;
        };
        let Some(status) = status else {
            if watched.reported.take().is_some() {
                cx.notify();
            }
            watched.worked = false;
            watched.idle_since = None;
            return;
        };
        let kind = watched.sighted.or(watched.probe.launched_agent);
        let status = recorded_status(step, kind, watched.reported, status);
        watched.reported_at = Some(Instant::now());
        watched.worked |= status == AgentStatus::Working;
        if watched.reported != Some(status) {
            cx.notify();
            watched.idle_since = (status == AgentStatus::Idle).then(Instant::now);
        }
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
    /// `read_at` is when the poll read the process table that `seen` comes from.
    fn note_sightings(
        &mut self,
        seen: &HashMap<EntityId, (AgentKind, Option<ResumeSession>)>,
        read_at: Instant,
        cx: &mut Context<Self>,
    ) {
        let mut gone = Vec::new();
        for watched in &mut self.watched {
            let kind = seen.get(&watched.id).map(|(kind, _)| *kind);
            let left = watched.sighted.is_some() && watched.sighted != kind;
            if left || kind.is_some() && reported_unseen(watched.reported_at, watched.unsighted_at)
            {
                watched.reported = None;
                watched.worked = false;
                watched.idle_since = None;
            }
            if left {
                gone.extend(watched.helper.take());
            }
            if kind.is_none() {
                watched.unsighted_at = Some(read_at);
            }
            watched.sighted = kind;
        }
        for helper in gone {
            self.tell_parent(&helper, HelperEvent::Exited, cx);
        }
    }

    fn apply(
        &mut self,
        sightings: Result<Vec<Sighting>, String>,
        read_at: Instant,
        cx: &mut Context<Self>,
    ) {
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
        for watched in &mut self.watched {
            watched.probe.discovery = sightings
                .iter()
                .find(|sighting| sighting.id == watched.id)
                .and_then(|sighting| sighting.discovery.clone());
        }
        let seen: HashMap<EntityId, (AgentKind, Option<ResumeSession>)> = sightings
            .iter()
            .map(|sighting| (sighting.id, (sighting.kind, sighting.conversation.clone())))
            .collect();
        self.track_conversations(&seen, cx);
        self.note_sightings(&seen, read_at, cx);
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
            let discovery = if probe.host.is_local() {
                let table = table.as_ref()?;
                let job = table.foreground_job(probe.shell_pid)?;
                Some(conversation::Discovery::refresh(
                    probe.discovery.as_ref(),
                    kind,
                    job,
                    now,
                    || conversation::discover(kind, &table.foreground_pids(probe.shell_pid)),
                ))
            } else {
                None
            };
            let conversation = discovery
                .as_ref()
                .and_then(|discovery| discovery.found().cloned());
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
                discovery,
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

/// How long a pipeline step's agent stays `idle`, with a silent terminal, before its turn
/// counts as over. An agent that goes idle in the middle of a turn reports `working` again or
/// keeps printing well within it; the poll runs every [`POLL_INTERVAL`], so the request shows
/// up to one tick later.
const TURN_END_GRACE: Duration = Duration::from_secs(3);

/// Whether an agent's kind ends its turns by going quiet instead of saying `done`. Claude
/// Code says `done` itself; its `idle` is a session start, which also follows `/clear` and a
/// compaction in the middle of a turn.
fn rests_silently(kind: Option<AgentKind>) -> bool {
    kind != Some(AgentKind::ClaudeCode)
}

/// Whether the last report came from an agent the poll cannot see, such as Codex behind
/// `ssh`: a poll that read the process table after the report found no agent in the
/// terminal. An agent that appears later did not make it. A report newer than that read may
/// be the new agent's own first word, which often comes before the poll sees it.
fn reported_unseen(reported_at: Option<Instant>, unsighted_at: Option<Instant>) -> bool {
    reported_at
        .zip(unsighted_at)
        .is_some_and(|(reported, unsighted)| reported < unsighted)
}

/// Whether a pipeline step's agent ended its turn without saying so.
///
/// A step ends with a stage report, so an agent back at its prompt finished a response
/// without one. Codex 0.167 was measured ending every turn with `idle`, never `done`; left as
/// idle such a step looks busy until its time budget runs out. The turn counts as over once
/// the agent has been `working` since it appeared (an earlier `idle` is its start) and has
/// then stayed `idle` with a silent terminal for [`TURN_END_GRACE`]. Outside a pipeline
/// nothing is owed, and idle stays idle.
fn ended_turn(
    step: bool,
    kind: Option<AgentKind>,
    worked: bool,
    reported: Option<AgentStatus>,
    idle_for: Option<Duration>,
    streaming: bool,
) -> bool {
    step && worked
        && rests_silently(kind)
        && reported == Some(AgentStatus::Idle)
        && !streaming
        && idle_for.is_some_and(|idle| idle >= TURN_END_GRACE)
}

/// The status to record for what an agent reported about itself. In a pipeline step a
/// finished response stays finished when the agent then says `idle`: a newer Codex does so
/// once the user has looked at its `done`, and one whose turn [`ended_turn`] repeats it.
fn recorded_status(
    step: bool,
    kind: Option<AgentKind>,
    previous: Option<AgentStatus>,
    status: AgentStatus,
) -> AgentStatus {
    let finished = previous == Some(AgentStatus::Done) && status == AgentStatus::Idle;
    if step && finished && rests_silently(kind) {
        AgentStatus::Done
    } else {
        status
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
    use super::{
        AgentKind, AgentStatus, AgentsEvent, Duration, HelperEvent, Instant, TURN_END_GRACE,
        ended_turn, helper_event, recorded_status, update_status,
    };
    use gpui_kit as gpui;

    const STATUSES: [AgentStatus; 6] = [
        AgentStatus::Working,
        AgentStatus::Idle,
        AgentStatus::WaitingInput,
        AgentStatus::WaitingApproval,
        AgentStatus::Done,
        AgentStatus::Failed,
    ];
    const KINDS: [Option<AgentKind>; 4] = [
        None,
        Some(AgentKind::Codex),
        Some(AgentKind::OpenCode),
        Some(AgentKind::ClaudeCode),
    ];

    #[test]
    fn a_step_agent_ends_its_turn_by_staying_idle_and_silent_after_working() {
        let codex = Some(AgentKind::Codex);
        let idle = Some(AgentStatus::Idle);
        let quiet = Some(TURN_END_GRACE);
        assert!(ended_turn(true, codex, true, idle, quiet, false));
        assert!(ended_turn(true, None, true, idle, quiet, false));

        // Codex is idle before it takes its prompt: the step has only started.
        assert!(!ended_turn(true, codex, false, idle, quiet, false));
        // Idle a moment ago may be the middle of a turn; so may idle with output still coming.
        let moment = Some(TURN_END_GRACE - Duration::from_millis(1));
        assert!(!ended_turn(true, codex, true, idle, moment, false));
        assert!(!ended_turn(true, codex, true, idle, quiet, true));
        assert!(!ended_turn(true, codex, true, idle, None, false));
        // Claude Code goes idle on a session start and says `done` itself.
        let claude = Some(AgentKind::ClaudeCode);
        assert!(!ended_turn(true, claude, true, idle, quiet, false));
        // An ordinary pane owes no report.
        assert!(!ended_turn(false, codex, true, idle, quiet, false));

        // Nothing but a reported idle is rest, whatever else holds.
        for reported in STATUSES.map(Some).into_iter().chain([None]) {
            for kind in KINDS {
                let rests = reported == idle && kind != claude;
                assert_eq!(
                    ended_turn(true, kind, true, reported, quiet, false),
                    rests,
                    "{kind:?} {reported:?}"
                );
            }
        }
    }

    #[test]
    fn a_steps_finished_response_stays_finished_when_its_agent_says_idle() {
        use AgentStatus::{Done, Idle};
        let codex = Some(AgentKind::Codex);
        assert_eq!(recorded_status(true, codex, Some(Done), Idle), Done);
        // Without a finished response before it, idle is recorded as said.
        for previous in STATUSES.map(Some).into_iter().chain([None]) {
            for status in STATUSES {
                for (step, kind) in [(true, codex), (false, codex), (true, KINDS[3])] {
                    let kept = step && kind == codex && previous == Some(Done) && status == Idle;
                    assert_eq!(
                        recorded_status(step, kind, previous, status),
                        if kept { Done } else { status },
                        "step={step} {kind:?} {previous:?} -> {status:?}"
                    );
                }
            }
        }
    }

    /// A `/bin/cat` pane watched as one `kind` was launched in, with the statuses its agent is
    /// recorded as and the attention it raised. The entity holds the pane weakly, so the
    /// caller keeps it.
    struct Pane {
        agents: gpui::Entity<super::Agents>,
        session: gpui::Entity<super::TerminalSession>,
        attention: std::rc::Rc<std::cell::RefCell<Vec<AgentStatus>>>,
        _subscription: gpui::Subscription,
    }

    impl Pane {
        fn new(kind: AgentKind, step: bool, cx: &mut gpui::TestAppContext) -> Self {
            use super::{Agents, Host, Path, Terminal, TerminalSession};
            use gpui_kit::AppContext as _;
            use hive_core::agent::api::ReportGrant;
            use hive_core::terminal::TerminalSize;

            let terminal =
                Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
            let session = cx.new(|_| {
                let mut session = TerminalSession::for_test(terminal);
                session.report_grant = step.then(|| ReportGrant::generate().unwrap());
                session
            });
            let agents = cx.new(|cx| Agents::new(None, cx));
            agents.update(cx, |agents, cx| {
                let terminal = session.read(cx).terminal().unwrap();
                agents.watch(&session, &Host::Local, Some(kind), Path::new("/"), terminal);
            });
            let attention = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
            let sink = attention.clone();
            let subscription = cx.update(|cx| {
                cx.subscribe(&agents, move |_, event: &AgentsEvent, _| {
                    if let AgentsEvent::Attention(_, status) = event {
                        sink.borrow_mut().push(*status);
                    }
                })
            });
            Self {
                agents,
                session,
                attention,
                _subscription: subscription,
            }
        }

        /// Applies the agent's own report and returns the status it now has.
        fn report(
            &self,
            status: Option<AgentStatus>,
            cx: &mut gpui::TestAppContext,
        ) -> Option<AgentStatus> {
            let id = self.session.entity_id();
            self.agents.update(cx, |agents, cx| {
                agents.report_status(id, status, cx);
                agents.recipient(id).status
            })
        }

        /// The poll found `seen` in the pane, from a process table read at `read_at`; returns
        /// the agent's status.
        fn sight(
            &self,
            seen: Option<AgentKind>,
            read_at: Instant,
            cx: &mut gpui::TestAppContext,
        ) -> Option<AgentStatus> {
            let id = self.session.entity_id();
            self.agents.update(cx, |agents, cx| {
                let seen = seen.map(|kind| (id, (kind, None))).into_iter().collect();
                agents.note_sightings(&seen, read_at, cx);
                agents.recipient(id).status
            })
        }

        /// The poll's look at the pane `after` its last report; returns the agent's status.
        fn poll(&self, after: Duration, cx: &mut gpui::TestAppContext) -> Option<AgentStatus> {
            let id = self.session.entity_id();
            self.agents.update(cx, |agents, cx| {
                agents.settle_turns(Instant::now() + after, cx);
                agents.recipient(id).status
            })
        }
    }

    #[gpui::test]
    fn a_step_agent_that_stays_idle_after_working_needs_attention_once_per_turn(
        cx: &mut gpui::TestAppContext,
    ) {
        use AgentStatus::{Done, Idle, WaitingApproval, Working};
        let pane = Pane::new(AgentKind::Codex, true, cx);
        let (at_once, quiet) = (Duration::ZERO, TURN_END_GRACE);

        // Codex is idle before it takes its prompt: however long, the step has only started.
        assert_eq!(pane.report(Some(Idle), cx), Some(Idle));
        assert_eq!(pane.poll(quiet, cx), Some(Idle));
        assert_eq!(pane.report(Some(Working), cx), Some(Working));

        // The turn ends once the agent has stayed idle and silent, not the moment it says so.
        assert_eq!(pane.report(Some(Idle), cx), Some(Idle));
        assert_eq!(pane.poll(at_once, cx), Some(Idle));
        assert!(pane.attention.borrow().is_empty());
        assert_eq!(pane.poll(quiet, cx), Some(Done));
        assert_eq!(*pane.attention.borrow(), [Done]);
        // Saying idle again, or the next poll, is the same finished response.
        assert_eq!(pane.report(Some(Idle), cx), Some(Done));
        assert_eq!(pane.poll(quiet, cx), Some(Done));
        assert_eq!(*pane.attention.borrow(), [Done]);

        // Idle in the middle of a turn: the agent works again before the grace is over.
        assert_eq!(pane.report(Some(Working), cx), Some(Working));
        assert_eq!(pane.report(Some(Idle), cx), Some(Idle));
        assert_eq!(pane.report(Some(Working), cx), Some(Working));
        assert_eq!(pane.poll(quiet, cx), Some(Working));
        assert_eq!(*pane.attention.borrow(), [Done]);

        // A dialog the user dismissed, then rest.
        assert_eq!(
            pane.report(Some(WaitingApproval), cx),
            Some(WaitingApproval)
        );
        assert_eq!(pane.report(Some(Idle), cx), Some(Idle));
        assert_eq!(pane.poll(quiet, cx), Some(Done));
        // A newer Codex says `done`, then goes idle once the user looked at it.
        assert_eq!(pane.report(Some(Done), cx), Some(Done));
        assert_eq!(pane.report(Some(Idle), cx), Some(Done));
        assert_eq!(*pane.attention.borrow(), [Done, WaitingApproval, Done]);

        // The program withdrew its status: whatever runs next starts over.
        assert_eq!(pane.report(None, cx), None);
        assert_eq!(pane.report(Some(Idle), cx), Some(Idle));
        assert_eq!(pane.poll(quiet, cx), Some(Idle));
        assert_eq!(pane.attention.borrow().len(), 3);
    }

    /// Codex behind `ssh devbox` reports through the pane, but the poll sees only `ssh`. After
    /// it, a local agent must not start out with Codex's last word.
    #[gpui::test]
    fn a_report_from_an_agent_the_poll_never_saw_is_not_passed_to_the_next_one(
        cx: &mut gpui::TestAppContext,
    ) {
        use AgentStatus::{WaitingApproval, Working};
        let later = || Instant::now() + Duration::from_millis(1);
        let pane = Pane::new(AgentKind::Codex, false, cx);
        assert_eq!(
            pane.report(Some(WaitingApproval), cx),
            Some(WaitingApproval)
        );
        assert_eq!(pane.sight(None, later(), cx), Some(WaitingApproval));
        assert_eq!(pane.sight(Some(AgentKind::ClaudeCode), later(), cx), None);

        // A new agent often speaks before the poll sees it: a read older than its report
        // does not make the report a stranger's.
        let pane = Pane::new(AgentKind::Codex, false, cx);
        let before_the_report = Instant::now();
        assert_eq!(pane.sight(None, before_the_report, cx), None);
        assert_eq!(pane.report(Some(Working), cx), Some(Working));
        assert_eq!(
            pane.sight(Some(AgentKind::Codex), later(), cx),
            Some(Working)
        );
        // An agent the poll keeps seeing keeps its own word.
        assert_eq!(
            pane.sight(Some(AgentKind::Codex), later(), cx),
            Some(Working)
        );
    }

    #[gpui::test]
    fn idle_after_work_stays_idle_outside_a_pipeline_and_for_claude_code(
        cx: &mut gpui::TestAppContext,
    ) {
        use AgentStatus::{Idle, Working};
        for (kind, step) in [(AgentKind::Codex, false), (AgentKind::ClaudeCode, true)] {
            let pane = Pane::new(kind, step, cx);
            for report in [Working, Idle, Working, Idle] {
                assert_eq!(pane.report(Some(report), cx), Some(report), "{kind:?}");
            }
            assert_eq!(pane.poll(TURN_END_GRACE, cx), Some(Idle), "{kind:?}");
            assert!(pane.attention.borrow().is_empty(), "{kind:?}");
        }
    }

    #[gpui::test]
    fn background_request_changes_notify_observers_without_a_terminal_view(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::{Agents, Host, Path, Terminal, TerminalSession};
        use gpui_kit::AppContext as _;
        use hive_core::terminal::TerminalSize;

        let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
        let session = cx.new(|_| TerminalSession::for_test(terminal));
        let id = session.entity_id();
        let agents = cx.new(|cx| Agents::new(None, cx));
        agents.update(cx, |agents, cx| {
            agents.watch(
                &session,
                &Host::Local,
                None,
                Path::new("/"),
                session.read(cx).terminal().unwrap(),
            );
        });
        let changes = cx.new(|cx| {
            cx.observe(
                &agents,
                move |changes: &mut Vec<Option<AgentStatus>>, agents, cx| {
                    changes.push(agents.read(cx).recipient(id).status);
                },
            )
            .detach();
            Vec::new()
        });
        use AgentStatus::{WaitingApproval, Working};
        for status in [
            Some(WaitingApproval),
            Some(WaitingApproval),
            Some(Working),
            None,
        ] {
            agents.update(cx, |agents, cx| agents.report_status(id, status, cx));
        }
        changes.read_with(cx, |changes, _| {
            assert_eq!(changes, &[Some(WaitingApproval), Some(Working), None]);
        });
    }

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
