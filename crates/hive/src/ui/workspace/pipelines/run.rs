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

mod changes;
mod decision;
mod details;
mod elapsed;
mod evidence;
mod handoff;
mod interaction;
use interaction::actionable;
mod sessions;
use sessions::LiveSession;
mod stage;
mod timeline;
use stage::Stage;

use gpui_kit::component::{
    diff::DiffState,
    tab::{Tab, TabBar},
};

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
}

pub(super) struct RunView {
    id: u64,
    run: Option<Rc<Run>>,
    note: Entity<TextareaState>,
    plan: Vec<(Entity<TextareaState>, Entity<TextareaState>)>,
    session: Option<(Ticket, Entity<TerminalView>)>,
    error: Option<String>,
    tab: RunTab,
    editing_plan: bool,
    giving_feedback: bool,
    phase: Option<Phase>,
    selected_stage: Option<Stage>,
    expanded_checks: HashSet<usize>,
    expanded_reviews: HashSet<usize>,
    expanded_reports: HashSet<u64>,
    expanded_events: HashSet<usize>,
    selected_file: Option<(usize, String)>,
    display: Option<Entity<DiffState>>,
    timer: Option<Task<()>>,
    description_open: bool,
    _subscriptions: Vec<Subscription>,
}
impl RunView {
    pub(super) fn new(id: u64, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let store = app.store.clone();
        let pipelines = app.pipelines.clone();
        let agents = app.agents.clone();
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
            cx.observe(&agents, |view, _, cx| {
                if view.session.is_none()
                    && view.run.as_ref().is_some_and(|r| !r.pending().is_empty())
                {
                    cx.notify();
                }
            }),
            cx.observe(&pipelines, |view, pipelines, cx| {
                if view.tab == RunTab::Changes && !pipelines.read(cx).reviews.contains_key(&view.id)
                {
                    view.selected_file = None;
                    view.display = None;
                    let id = view.id;
                    cx.defer(move |cx| pipelines.update(cx, |p, cx| p.load_review(id, cx)));
                }
                cx.notify();
            }),
        ];
        Self {
            id,
            run,
            note: area("", "Feedback or decision rationale", window, cx),
            plan: vec![],
            session: None,
            error: None,
            tab: RunTab::Timeline,
            editing_plan: false,
            giving_feedback: false,
            phase: None,
            selected_stage: None,
            expanded_checks: HashSet::new(),
            expanded_reviews: HashSet::new(),
            expanded_reports: HashSet::new(),
            expanded_events: HashSet::new(),
            selected_file: None,
            display: None,
            timer: None,
            description_open: false,
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
        self.edit(cx, |run| {
            if run.phase != Phase::ApprovePlan || assignments.len() != run.lanes.len() {
                return Err("The proposed plan has changed.".into());
            }
            for (lane, (task, criteria)) in run.lanes.iter_mut().zip(assignments) {
                lane.assignment = task;
                lane.criteria = criteria;
            }
            run.approve().map_err(|error| error.to_string())
        });
    }
    pub(super) fn terminal(&mut self, ticket: Ticket, w: &mut Window, cx: &mut Context<Self>) {
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
            .update(cx, |p, cx| p.load_review(self.id, cx));
    }
}
impl Render for RunView {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.session = None;
                                    cx.notify();
                                }))
                                .smooth_hover(ButtonLook::Ghost, &back_hover, cx),
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
                .child("This run is unavailable.")
                .into_any_element();
        };
        if self.phase != Some(run.phase) {
            self.phase = Some(run.phase);
            self.editing_plan = false;
            self.giving_feedback = false;
            self.plan.clear();
            self.note = area("", "Add feedback for the agent", w, cx);
            self.selected_stage = match run.phase {
                Phase::ApprovePlan => Some(Stage::Approval),
                Phase::Decision(ix) => Some(Stage::Decision(ix)),
                _ => None,
            };
            if matches!(run.phase, Phase::Acceptance | Phase::Accepted) {
                let view = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = view.update(cx, |v, cx| v.refresh_changes(cx));
                });
            }
        }
        let pipelines = AppState::get(cx).pipelines.clone();
        let stopped = matches!(run.phase, Phase::Accepted | Phase::Cancelled);
        let sessions = LiveSession::for_run(&run, cx);
        let request = sessions
            .iter()
            .find(|s| s.request_action().is_some())
            .and_then(|s| s.status);
        let mut phase = phase_label(&run);
        if let Some(status) = request {
            phase = format!("{phase} · {}", status.label());
        }
        let status_color = match run.phase {
            _ if request.is_some() => cx.theme().warning,
            Phase::Accepted => cx.theme().success,
            Phase::ApprovePlan | Phase::Decision(_) | Phase::Acceptance => cx.theme().warning,
            Phase::Cancelled => cx.theme().muted_foreground,
            _ => cx.theme().info,
        };
        let mut top = v_flex().px_5().pt_4().pb_3().gap_2();
        let mut title = h_flex().gap_2().items_start().child(
            div()
                .flex_1()
                .min_w_0()
                .text_3xl()
                .font_weight(FontWeight::MEDIUM)
                .child(task_title(&run.task)),
        );
        if run.task.lines().count() > 1 || run.task.chars().count() > 110 {
            title = title.child(button(
                "task-description",
                if self.description_open {
                    "Hide task"
                } else {
                    "Full task"
                },
                ButtonLook::Ghost,
                w,
                cx,
                |v, _, cx| {
                    v.description_open = !v.description_open;
                    cx.notify();
                },
            ));
        }
        top = top.child(title);
        if self.description_open {
            top = top.child(
                div()
                    .id("task-scroll")
                    .max_h(rems(6.))
                    .overflow_y_scroll()
                    .text_sm()
                    .child(SelectableText::new("full-task", run.task.clone())),
            );
        }
        let mut meta = h_flex()
            .gap_3()
            .flex_wrap()
            .child(
                h_flex()
                    .gap_1p5()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(status_color.opacity(0.12))
                    .child(div().size_1p5().rounded_full().bg(status_color))
                    .child(div().text_xs().text_color(status_color).child(phase)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(Lucide::GitBranch)
                            .size_3p5()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(subtle(run.template.name.clone(), cx)),
            );
        let repos = run
            .lanes
            .iter()
            .map(|l| &l.project)
            .collect::<HashSet<_>>()
            .len();
        meta = meta.child(subtle(
            format!(
                "{repos} {}",
                if repos == 1 {
                    "repository"
                } else {
                    "repositories"
                }
            ),
            cx,
        ));
        let now = elapsed::now_ms();
        let range = elapsed::range(&run.timings, now);
        let recording = !stopped && run.timings.iter().any(|r| r.end_ms.is_none() && !r.failed);
        if recording && self.timer.is_none() {
            self.timer = Some(cx.spawn(async |view, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = view.update(cx, |this, cx| {
                    this.timer.take();
                    cx.notify();
                });
            }));
        } else if !recording {
            self.timer = None;
        }
        if let Some((start, end)) = range {
            meta = meta.child(subtle(
                format!("{} elapsed", elapsed::duration(end.saturating_sub(start))),
                cx,
            ));
            let invocations = run
                .timings
                .iter()
                .filter(|row| row.attempt.is_some())
                .count();
            if invocations > 0 {
                meta = meta.child(subtle(
                    format!(
                        "{invocations} agent {}",
                        if invocations == 1 {
                            "session"
                        } else {
                            "sessions"
                        }
                    ),
                    cx,
                ));
            }
        }
        top = top.child(meta);
        if !stopped {
            top = top.child(self.actions(&run, &sessions, w, cx));
        }
        if let Some(msg) = self.error.clone() {
            top = top.child(div().text_sm().text_color(cx.theme().danger).child(msg));
        }
        if let Some(error) = pipelines.read(cx).error(self.id).map(str::to_owned) {
            top = top.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Icon::new(Lucide::CircleAlert)
                            .size_4()
                            .text_color(cx.theme().warning),
                    )
                    .child(div().text_sm().child(error))
                    .child(button(
                        "retry-run",
                        "Retry interrupted work",
                        ButtonLook::Outline,
                        w,
                        cx,
                        |v, _, cx| v.retry(cx),
                    )),
            );
        }
        let tabs = TabBar::new("pipeline-run-tabs")
            .segmented()
            .small()
            .selected_index(
                RunTab::ALL
                    .iter()
                    .position(|t| *t == self.tab)
                    .unwrap_or_default(),
            )
            .children(RunTab::ALL.map(|tab| Tab::new().label(tab.label()).cursor_pointer()))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if let Some(tab) = RunTab::ALL.get(*ix) {
                    this.tab = *tab;
                }
                if this.tab == RunTab::Changes
                    && !AppState::get(cx)
                        .pipelines
                        .read(cx)
                        .reviews
                        .contains_key(&this.id)
                {
                    this.refresh_changes(cx);
                }
                cx.notify();
            }));
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
            .child(top)
            .child(
                h_flex()
                    .px_5()
                    .pb_2()
                    .flex_none()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(tabs),
            )
            .child(content)
            .into_any_element()
    }
}

fn saved_run(store: &crate::state::store::Store, id: u64) -> Option<&Run> {
    store.state.pipelines.runs.iter().find(|r| r.id == id)
}

fn task_title(task: &str) -> String {
    let full = task.trim().lines().next().unwrap_or_default();
    if full.chars().count() > 110 {
        format!("{}…", full.chars().take(109).collect::<String>())
    } else {
        full.to_owned()
    }
}
