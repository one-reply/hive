//! Task execution lives above the window so switching projects never suspends a run.
use std::collections::{HashMap, HashSet, hash_map::RandomState};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::store::Store;
use super::terminal_session::TerminalSession;
use crate::app::AppState;
use gpui_kit::*;
use hive_core::agent::launch::ResumeSession;
use hive_core::command::Cancellation;
use hive_core::host::Host;
use hive_core::pipeline::{Lane, Phase, Report, Run, Template, Ticket, now_ms, workspace};

mod execution;
mod models;
mod review;
mod runtime;
mod sessions;
mod templates;

pub(crate) use workspace::ReviewRepo;

type PatchPreview = (
    usize,
    String,
    Result<Vec<gpui_kit::component::diff::DiffFile>, String>,
);

pub(crate) struct RevealRun(pub u64);
impl EventEmitter<RevealRun> for Pipelines {}

struct Live {
    session: Entity<TerminalSession>,
    _subscriptions: Vec<Subscription>,
}

pub(crate) struct Pipelines {
    store: Entity<Store>,
    pub(crate) models: models::Models,
    live: HashMap<Ticket, Live>,
    /// Only a user's explicit recovery action populates this map.
    resumes: HashMap<Ticket, ResumeSession>,
    suspended: HashSet<Ticket>,
    session_errors: HashMap<Ticket, String>,
    runtime: HashMap<u64, runtime::RunRuntime>,
    next_job: u64,
    agents: Entity<super::agents::Agents>,
    _agent_status: Vec<Subscription>,
    previewing: HashSet<(PathBuf, String)>,
    pub(crate) previews: HashMap<(PathBuf, String), Result<workspace::BasePreview, String>>,
    stopped: bool,
}

impl Pipelines {
    pub(crate) fn new(
        store: Entity<Store>,
        agents: &Entity<super::agents::Agents>,
        cx: &mut Context<Self>,
    ) -> Self {
        use super::agents::AgentsEvent;
        use sessions::{TurnOutcome, turn_outcome};
        let agent_status = vec![
            cx.subscribe(agents, |pipelines: &mut Self, _, event, cx| {
                let AgentsEvent::Attention(id, status) = event else {
                    return;
                };
                let Some(ticket) = pipelines.ticket_for_session(*id) else {
                    return;
                };
                match turn_outcome(*status) {
                    TurnOutcome::Failed => pipelines.session_failed(
                        ticket,
                        "The agent reported a failed turn. Open the session to continue or retry the step.".into(),
                        cx,
                    ),
                    // The live session shows the request; the attempt is still in progress.
                    TurnOutcome::AwaitsUser => cx.notify(),
                    TurnOutcome::InProgress => {}
                }
            }),
            cx.observe(agents, |pipelines: &mut Self, _, cx| {
                pipelines.clear_recovered_sessions(cx);
            }),
        ];
        if store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .any(Run::is_interruptible)
        {
            store.update(cx, |s, cx| {
                s.edit_pipelines(cx, |l| {
                    for run in &mut l.runs {
                        run.interrupt_timings(None);
                    }
                })
            });
        }
        let suspended = store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .flat_map(Run::pending)
            .collect();
        let runtime = store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .filter(|run| matches!(run.phase(), Phase::Commands(_)))
            .map(|run| {
                let error =
                    "Hive restarted during a required check. Retry to run the command again.";
                let runtime = runtime::RunRuntime {
                    error: Some(error.into()),
                    ..Default::default()
                };
                (run.id(), runtime)
            })
            .collect();
        Self {
            store,
            models: Default::default(),
            suspended,
            live: HashMap::new(),
            resumes: HashMap::new(),
            session_errors: HashMap::new(),
            runtime,
            next_job: 0,
            agents: agents.clone(),
            _agent_status: agent_status,
            previews: HashMap::new(),
            previewing: HashSet::new(),
            stopped: false,
        }
    }

    fn run<'a>(&self, id: u64, cx: &'a App) -> Result<&'a Run, String> {
        self.store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .find(|run| run.id() == id)
            .ok_or_else(|| "Pipeline run no longer exists.".into())
    }

    pub(crate) fn reveal(&mut self, id: u64, cx: &mut Context<Self>) {
        cx.emit(RevealRun(id));
    }

    pub(crate) fn error(&self, id: u64) -> Option<&str> {
        if let Some(message) = self.runtime(id).and_then(|r| r.error.as_deref()) {
            return Some(message);
        }
        if let Some((_, message)) = self
            .session_errors
            .iter()
            .find(|(ticket, _)| ticket.run == id)
        {
            return Some(message);
        }
        self.suspended.iter().any(|ticket| ticket.run == id)
            .then_some("Hive restarted during this step. Resume a saved session or retry interrupted work.")
    }

    pub(crate) fn attention(&self, cx: &App) -> Vec<(u64, PathBuf, String)> {
        self.store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .filter_map(|run| {
                let status = self
                    .error(run.id())
                    .or(self.agent_request(run, cx))
                    .or(match run.phase() {
                        Phase::ApprovePlan => Some("Approve plan"),
                        Phase::Decision(_) => Some("Review findings"),
                        Phase::Acceptance => Some("Accept changes"),
                        _ => None,
                    })?;
                Some((
                    run.id(),
                    run.owner().to_path_buf(),
                    format!("{} · {status}", run.task()),
                ))
            })
            .collect()
    }

    fn registered(&self, project: &Path, cx: &App) -> bool {
        self.store
            .read(cx)
            .state
            .projects
            .iter()
            .any(|p| p.host.is_local() && p.path == project)
    }

    pub(crate) fn preview(&mut self, project: PathBuf, base: String, cx: &mut Context<Self>) {
        let key = (project.clone(), base.clone());
        if !self.registered(&project, cx) || !self.previewing.insert(key.clone()) {
            return;
        }
        self.previews.remove(&key);
        let work = cx
            .background_executor()
            .spawn(async move { workspace::preview(&project, &base).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                p.previewing.remove(&key);
                p.previews.insert(key, result);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn start(
        &mut self,
        owner: PathBuf,
        task: String,
        template: Template,
        mut lanes: Vec<Lane>,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        // Every stage waits for a checkpoint, so an unsaved run could only fail.
        if !self.store.read(cx).can_save() {
            return Err(
                "Workspace state cannot be saved. Restart Hive before starting pipelines.".into(),
            );
        }
        if !self.registered(&owner, cx) {
            return Err("Choose a registered local project.".into());
        }
        if AppState::get(cx)
            .agents
            .read(cx)
            .endpoint()
            .and_then(|end| end.api)
            .is_none()
        {
            return Err(
                "The Hive agent endpoint is unavailable. Restart Hive before starting pipelines."
                    .into(),
            );
        }
        let mut repository_bases = HashMap::new();
        for lane in &mut lanes {
            if !self.registered(&lane.project, cx) {
                return Err(format!(
                    "{} is not a registered local project.",
                    lane.project.display()
                ));
            }
            let key = (lane.project.clone(), lane.base.clone());
            let preview = self
                .previews
                .get(&key)
                .and_then(|p| p.as_ref().ok())
                .or_else(|| {
                    self.previews.iter().find_map(|((project, _), p)| {
                        p.as_ref()
                            .ok()
                            .filter(|p| project == &lane.project && p.commit == lane.base)
                    })
                })
                .ok_or_else(|| {
                    format!(
                        "Preview the base revision for {} before starting.",
                        lane.name
                    )
                })?;
            if repository_bases
                .insert(&preview.worktrees_dir, &preview.commit)
                .is_some_and(|previous| previous != &preview.commit)
            {
                return Err(format!(
                    "Choose the same base commit for all lanes in {}.",
                    lane.project.display()
                ));
            }
            lane.base.clone_from(&preview.commit);
            lane.worktree.clear();
        }
        // JavaScript MCP clients preserve integer identities only within 53 bits.
        let id = RandomState::new().hash_one((&owner, &task)) & ((1_u64 << 53) - 1);
        if id == 0 || self.run(id, cx).is_ok() {
            return Err("Run ID collision. Start again.".into());
        }
        let mut run = Run::new(id, owner, task, template, lanes)?;
        run.record_stage(None, now_ms());
        self.store
            .update(cx, |s, cx| s.edit_pipelines(cx, |l| l.runs.push(run)));
        self.schedule(id, cx);
        Ok(id)
    }

    /// Apply a user decision atomically; an invalid edit cannot leave half its fields changed.
    pub(crate) fn edit(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Run) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut run = self.run(id, cx)?.clone();
        let previous = run.phase();
        if let Err(error) = change(&mut run) {
            tracing::warn!(run = id, phase = ?previous, %error, "pipeline transition rejected");
            return Err(error);
        }
        run.record_stage(Some(previous), now_ms());
        if run.phase() != Phase::Accepted {
            self.clear_review(id);
        }
        self.store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |l| {
                if let Some(record) = l.runs.iter_mut().find(|r| r.id() == id) {
                    *record = run;
                }
            })
        });
        self.schedule(id, cx);
        Ok(())
    }

    pub(crate) fn report(
        &mut self,
        from: u64,
        ticket: &Ticket,
        report: Report,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self
            .session(ticket)
            .is_none_or(|s| s.entity_id().as_u64() != from)
        {
            return Err("The report came from a different pipeline session.".into());
        }
        let failed = matches!(&report, Report::Review { findings, .. } if !findings.is_empty());
        self.edit(ticket.run, cx, |r| {
            r.report(ticket, report)?;
            r.finish_attempt_at(*ticket, failed, now_ms());
            Ok(())
        })?;
        self.session_errors.remove(ticket);
        self.stop_session(ticket, cx);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        self.edit(id, cx, |r| Ok(r.cancel()?))?;
        self.suspended.retain(|t| t.run != id);
        self.session_errors.retain(|t, _| t.run != id);
        self.resumes.retain(|t, _| t.run != id);
        let tickets: Vec<_> = self.live.keys().copied().filter(|t| t.run == id).collect();
        for ticket in tickets {
            self.stop_session(&ticket, cx);
        }
        self.drop_runtime(id);
        cx.notify();
        Ok(())
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        self.stopped = true;
        if self
            .store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .any(Run::is_interruptible)
        {
            let at = now_ms();
            self.store.update(cx, |s, cx| {
                s.edit_pipelines(cx, |l| {
                    for run in &mut l.runs {
                        run.interrupt_timings(Some(at));
                    }
                })
            });
        }
        for worker in self.runtime.values_mut().filter_map(|r| r.worker.take()) {
            worker.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
