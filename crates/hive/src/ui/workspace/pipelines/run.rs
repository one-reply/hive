use super::*;
use crate::terminal::view::TerminalView;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    IndexPath,
    dialog::DialogButtonProps,
    searchable_list::SearchableVec,
    select::{Select, SelectState},
};
use hive_core::agent::AgentStatus;
use hive_core::pipeline::{Phase, Ticket};

mod acceptance;
mod changes;
mod checkpoint;
mod comments;
mod decision;
mod delivery;
mod details;
mod elapsed;
mod evidence;
mod handoff;
mod header;
pub(super) use header::task_title;
mod interaction;
use interaction::{actionable, toggle};
mod reason;
mod sessions;
use sessions::LiveSession;
mod stage;
mod summary;
mod timeline;
mod unsaved;
use stage::{Stage, StageIcon as _, StageStatus};

use gpui_kit::component::{
    diff::DiffState,
    tab::{Tab, TabBar},
};

/// How long the run took from its first recorded stage to its last, for lists of runs.
pub(super) fn length(run: &Run, now: u64) -> Option<String> {
    let (start, end) = elapsed::range(run.timings(), now)?;
    Some(elapsed::duration(end.saturating_sub(start)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunTab {
    Timeline,
    Changes,
    Checks,
    Activity,
}
impl RunTab {
    const ALL: [Self; 4] = [Self::Timeline, Self::Changes, Self::Checks, Self::Activity];
    fn label(self) -> &'static str {
        match self {
            Self::Timeline => "Timeline",
            Self::Changes => "Changes",
            Self::Checks => "Checks",
            Self::Activity => "Activity",
        }
    }
    fn icon(self) -> Lucide {
        match self {
            Self::Timeline => Lucide::ChartGantt,
            Self::Changes => Lucide::FileDiff,
            Self::Checks => Lucide::ListChecks,
            Self::Activity => Lucide::ScrollText,
        }
    }
}

pub(super) struct RunView {
    id: u64,
    run: Option<Rc<Run>>,
    note: Entity<TextareaState>,
    contract: Entity<TextareaState>,
    plan: Vec<(Entity<TextareaState>, Entity<TextareaState>)>,
    session: Option<(Ticket, Entity<TerminalView>)>,
    /// Keeps keyboard focus inside the run when its live session closes.
    focus: FocusHandle,
    error: Option<String>,
    tab: RunTab,
    editing_plan: bool,
    giving_feedback: bool,
    since_feedback: bool,
    phase: Option<Phase>,
    selected_stage: Option<Stage>,
    expanded_checks: HashSet<usize>,
    selected_test: Option<(usize, String)>,
    expanded_reviews: HashSet<usize>,
    expanded_reports: HashSet<u64>,
    expanded_events: HashSet<usize>,
    selected_file: Option<(usize, String)>,
    display: Option<Entity<DiffState>>,
    timer: Option<Task<()>>,
    description_open: bool,
    criteria_expanded: bool,
    summary_expanded: bool,
    _subscriptions: Vec<Subscription>,
}
impl RunView {
    pub(super) fn new(id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let store = app.store.clone();
        let pipelines = app.pipelines.clone();
        let agents = app.agents.clone();
        let pull_requests = app.pull_requests.clone();
        let run = saved_run(store.read(cx), id).cloned().map(Rc::new);
        let subscriptions = vec![
            cx.observe(&store, move |this, store, cx| {
                let state = store.read(cx);
                let saved = saved_run(state, id);
                if this.run.as_deref() != saved {
                    this.run = saved.cloned().map(Rc::new);
                    cx.notify();
                }
            }),
            cx.observe(&pull_requests, |view, _, cx| {
                if view.phase == Some(Phase::Accepted) {
                    cx.notify();
                }
            }),
            cx.observe(&agents, |view, _, cx| {
                if view.session.is_none()
                    && view.run.as_ref().is_some_and(|r| !r.pending().is_empty())
                {
                    cx.notify();
                }
            }),
            cx.observe_in(&pipelines, window, |view, pipelines, window, cx| {
                if let Some((ticket, _)) = &view.session
                    && pipelines.read(cx).session(ticket).is_none()
                {
                    view.close_session(window, cx);
                }
                if view.tab == RunTab::Changes && pipelines.read(cx).review(view.id).is_none() {
                    view.selected_file = None;
                    view.display = None;
                    let id = view.id;
                    let since = view.since_feedback;
                    cx.defer(move |cx| pipelines.update(cx, |p, cx| p.load_review(id, since, cx)));
                }
                cx.notify();
            }),
        ];
        Self {
            id,
            run,
            note: area("", "Feedback or decision rationale", window, cx),
            contract: area("", "Shared contract", window, cx),
            plan: vec![],
            session: None,
            focus: cx.focus_handle(),
            error: None,
            tab: RunTab::Timeline,
            editing_plan: false,
            giving_feedback: false,
            since_feedback: false,
            phase: None,
            selected_stage: None,
            expanded_checks: HashSet::new(),
            selected_test: None,
            expanded_reviews: HashSet::new(),
            expanded_reports: HashSet::new(),
            expanded_events: HashSet::new(),
            selected_file: None,
            display: None,
            timer: None,
            description_open: false,
            criteria_expanded: false,
            summary_expanded: false,
            _subscriptions: subscriptions,
        }
    }
    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Run) -> Result<(), String>) {
        self.error = AppState::get(cx)
            .pipelines
            .clone()
            .update(cx, |p, cx| p.edit(self.id, cx, f))
            .err();
        cx.notify();
    }
    fn approve(&mut self, cx: &mut Context<Self>) {
        if !self.editing_plan {
            self.edit(cx, |r| r.approve().map_err(|error| error.to_string()));
            return;
        }
        let assignments = self
            .plan
            .iter()
            .map(|(task, criteria)| {
                (
                    task.read(cx).value().to_string(),
                    criteria.read(cx).value().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let contract = self.contract.read(cx).value().to_string();
        self.edit(cx, |run| {
            Ok(run.approve_with_contract(assignments, &contract)?)
        });
    }
    pub(super) fn terminal(&mut self, ticket: Ticket, w: &mut Window, cx: &mut Context<Self>) {
        if ticket.run != self.id {
            return;
        }
        let session = AppState::get(cx).pipelines.read(cx).session(&ticket);
        if let Some(session) = session {
            let view = cx.new(|cx| TerminalView::with_session(session, None, w, cx));
            view.focus_handle(cx).focus(w, cx);
            self.session = Some((ticket, view));
            self.error = None;
        } else {
            self.error = Some(
                "The live session is unavailable. Retry or resume the interrupted attempt.".into(),
            );
        }
        cx.notify();
    }
    fn close_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session = None;
        self.focus.focus(window, cx);
        cx.notify();
    }
    fn retry(&mut self, cx: &mut Context<Self>) {
        self.error = AppState::get(cx)
            .pipelines
            .clone()
            .update(cx, |p, cx| p.retry(self.id, cx))
            .err();
        cx.notify();
    }
    fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        self.selected_file = None;
        self.display = None;
        AppState::get(cx)
            .pipelines
            .clone()
            .update(cx, |p, cx| p.load_review(self.id, self.since_feedback, cx));
    }
}
impl Render for RunView {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _measure = crate::ui::workspace::measure80::run_view(); // MEASURE-80
        if let Some((ticket, session)) = &self.session {
            let back_hover = Hover::new("session-back", w, cx);
            return v_flex()
                .size_full()
                .child(
                    h_flex()
                        .p_2()
                        .gap_2()
                        .child(
                            Button::new("session-back")
                                .cursor_pointer()
                                .small()
                                .icon(Lucide::ArrowLeft)
                                .label("Back to run")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_session(window, cx);
                                }))
                                .smooth_hover(ButtonLook::Outline, &back_hover, cx),
                        )
                        .child(subtle(
                            format!("Live session · Attempt {}", ticket.attempt),
                            cx,
                        )),
                )
                .child(div().flex_1().min_h_0().child(session.clone()))
                .into_any_element();
        }
        let Some(run) = self.run.clone() else {
            return div()
                .p_4()
                .track_focus(&self.focus)
                .child("This run is unavailable.")
                .into_any_element();
        };
        if self.phase != Some(run.phase()) {
            self.phase = Some(run.phase());
            self.selected_test = None;
            self.since_feedback = !run.feedback_trees().is_empty()
                && matches!(run.phase(), Phase::Acceptance | Phase::Accepted);
            self.editing_plan = false;
            self.giving_feedback = false;
            self.plan.clear();
            self.note = area("", "Add feedback for the agent", w, cx);
            self.selected_stage = match run.phase() {
                Phase::ApprovePlan => Some(Stage::Approval),
                Phase::Decision(ix) => Some(Stage::Decision(ix)),
                _ => None,
            };
            if run.phase() == Phase::Accepted {
                let id = self.id;
                cx.defer(move |cx| {
                    AppState::get(cx)
                        .pipelines
                        .clone()
                        .update(cx, |p, cx| p.refresh_delivery(id, cx))
                });
            }
            if matches!(run.phase(), Phase::Acceptance | Phase::Accepted) {
                let view = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = view.update(cx, |v, cx| v.refresh_changes(cx));
                });
            }
        }
        let stopped = matches!(run.phase(), Phase::Accepted | Phase::Cancelled);
        let sessions = LiveSession::for_run(&run, cx);
        let now = elapsed::now_ms();
        let range = elapsed::range(run.timings(), now);
        let recording = !stopped
            && run
                .timings()
                .iter()
                .any(|r| r.end_ms.is_none() && !r.failed);
        self.tick_while(recording, cx);
        let top = self.header(&run, &sessions, range, stopped, w, cx);
        let navigation = self.navigation(&sessions, stopped, w, cx);
        let content = match self.tab {
            RunTab::Timeline => {
                let scale = range.map(|range| elapsed::chart_range(range, recording));
                self.timeline(&run, &sessions, now, scale, w, cx)
            }
            RunTab::Changes => self.changes(&run, w, cx),
            RunTab::Checks => self.checks(&run, w, cx),
            RunTab::Activity => self.activity(&run, w, cx),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .track_focus(&self.focus)
            .child(top)
            .child(navigation)
            .children(self.completion_summary(&run, w, cx))
            .child(content)
            .into_any_element()
    }
}

fn saved_run(store: &crate::state::store::Store, id: u64) -> Option<&Run> {
    store.state.pipelines.runs.iter().find(|r| r.id() == id)
}
