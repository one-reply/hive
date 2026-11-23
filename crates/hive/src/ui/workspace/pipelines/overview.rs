//! The status bar's Pipelines block: how many runs wait and work across every project, and
//! on hover a card that lists them with the drafts. A click on a line goes to it.
use super::library::{DETAIL_SEPARATOR, RunRow};
use super::*;
use crate::state::pipelines::{Entry, ListedRun, Overview};
use crate::ui::hover::SmoothHoverBg as _;
use crate::ui::hover_card::{Dismiss, hover_card};
use gpui_kit::component::progress::Progress;
use hive_core::pipeline::{RunProgress, draft::LaunchDraft};

/// Width of the card that lists the runs.
const CARD_WIDTH: Pixels = px(360.);
/// The list scrolls past this height, so the card fits a small window.
const LIST_MAX_HEIGHT: Pixels = px(420.);
/// Height of a live run's progress bar.
const PROGRESS_HEIGHT: Pixels = px(3.);
/// The block's icon while no run waits or works, as faint as the other idle status icons.
const IDLE_ICON_OPACITY: f32 = 0.5;
const UNTITLED_DRAFT: &str = "Untitled draft";

pub(crate) struct PipelinesOverview {
    _subscriptions: [Subscription; 3],
}

impl PipelinesOverview {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let (store, pipelines, agents) =
            (app.store.clone(), app.pipelines.clone(), app.agents.clone());
        Self {
            _subscriptions: [
                cx.observe(&store, |_, _, cx| cx.notify()),
                cx.observe(&pipelines, |_, _, cx| cx.notify()),
                // A run's status follows what its agent last reported.
                cx.observe(&agents, |_, _, cx| cx.notify()),
            ],
        }
    }
}

impl Render for PipelinesOverview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overview = AppState::get(cx).pipelines.read(cx).overview(cx);
        let hover = Hover::new("pipelines-overview", window, cx);
        let trigger = Button::new("pipelines-overview")
            .cursor_pointer()
            .xsmall()
            .accessibility_label(summary(overview.needs_you, overview.running))
            .child(counters(&overview, cx))
            .on_click(|_, window, cx| window.dispatch_action(Box::new(TogglePipelines), cx))
            .smooth_hover(ButtonLook::Ghost, &hover, cx);
        hover_card(
            "pipelines-overview-card",
            Anchor::BottomRight,
            trigger,
            // Read anew for every frame the card is open: a run can move while it is.
            |dismiss, window, cx| {
                let overview = AppState::get(cx).pipelines.read(cx).overview(cx);
                card(&overview, dismiss, window, cx)
            },
            window,
            cx,
        )
    }
}

/// What a screen reader says of the block.
fn summary(needs_you: usize, running: usize) -> String {
    let mut parts = Vec::new();
    if needs_you > 0 {
        let verb = if needs_you == 1 { "needs" } else { "need" };
        parts.push(format!("{needs_you} {verb} you"));
    }
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if parts.is_empty() {
        return "Pipelines: nothing active".into();
    }
    format!("Pipelines: {}", parts.join(", "))
}

/// The icon, then the runs that wait for the user and the runs that work.
fn counters(overview: &Overview, cx: &App) -> Div {
    let theme = cx.theme();
    let idle = overview.needs_you + overview.running == 0;
    h_flex()
        .gap_1p5()
        .items_center()
        .child(Icon::new(Lucide::Workflow).when(idle, |icon| {
            icon.text_color(theme.muted_foreground.opacity(IDLE_ICON_OPACITY))
        }))
        .children(counter(overview.needs_you, theme.warning))
        .children(counter(overview.running, theme.info))
}

fn counter(count: usize, color: Hsla) -> Option<Div> {
    (count > 0).then(|| {
        h_flex()
            .gap_1()
            .items_center()
            .child(div().size_1p5().rounded_full().bg(color))
            .child(div().text_xs().text_color(color).child(count.to_string()))
    })
}

/// Where a click on a line leads.
#[derive(Clone)]
enum Target {
    Run(u64),
    Draft(PathBuf),
}

/// What the card shows of one run or draft, read out of the store before rendering.
struct Line {
    target: Target,
    /// `None` for a draft.
    status: Option<RunStatus>,
    title: String,
    detail: String,
    progress: Option<RunProgress>,
    /// False for a run whose project was removed from Hive.
    reachable: bool,
}

fn run_line(listed: ListedRun, run: &Run, now: u64) -> Line {
    let row = RunRow::of(run, listed.status, now);
    let about = if listed.reachable {
        row.detail.as_str()
    } else {
        "project removed from Hive"
    };
    Line {
        target: Target::Run(listed.id),
        status: Some(listed.status),
        title: run::task_title(&row.task),
        detail: [path_name(run.owner()).as_str(), about].join(DETAIL_SEPARATOR),
        progress: (!row.finished).then(|| run.progress()),
        reachable: listed.reachable,
    }
}

fn draft_line(draft: &LaunchDraft) -> Line {
    Line {
        target: Target::Draft(draft.owner.clone()),
        status: None,
        title: draft_title(&draft.task),
        detail: [path_name(&draft.owner).as_str(), &draft.template.name].join(DETAIL_SEPARATOR),
        progress: None,
        reachable: true,
    }
}

/// A draft is named by its task, and one whose task is still empty by what it is.
fn draft_title(task: &str) -> String {
    let title = run::task_title(task);
    if title.is_empty() {
        UNTITLED_DRAFT.into()
    } else {
        title
    }
}

/// The filled share of a progress bar, in percent; `None` when the run has nothing to count.
fn percent(progress: RunProgress) -> Option<f32> {
    (progress.total > 0).then(|| progress.done as f32 / progress.total as f32 * 100.)
}

/// The overview's groups with each entry read out of the store. An entry that vanished
/// between the overview and this read is skipped.
fn lines(overview: &Overview, cx: &App) -> Vec<(&'static str, Vec<Line>)> {
    let library = &AppState::get(cx).store.read(cx).state.pipelines;
    let now = hive_core::pipeline::now_ms();
    let line = |entry: &Entry| match entry {
        Entry::Run(listed) => {
            let run = library.runs.iter().find(|run| run.id() == listed.id)?;
            Some(run_line(*listed, run, now))
        }
        Entry::Draft(owner) => {
            let draft = library.drafts.iter().find(|draft| &draft.owner == owner)?;
            Some(draft_line(draft))
        }
    };
    overview
        .groups
        .iter()
        .map(|(group, entries)| (group.label(), entries.iter().filter_map(line).collect()))
        .collect()
}

fn card(overview: &Overview, dismiss: &Dismiss, window: &mut Window, cx: &mut App) -> Div {
    let groups = lines(overview, cx);
    let mut list = v_flex().p_1p5().gap_0p5();
    for (label, group) in groups {
        list = list.child(subtle(label, cx).px_2().pt_1p5().pb_0p5());
        for line in group {
            list = list.child(line_row(line, dismiss, window, cx));
        }
    }
    if overview.is_empty() {
        list = list.child(subtle("No pipeline runs yet.", cx).px_2().py_2());
    }
    let title = h_flex()
        .px_3p5()
        .pt_3()
        .pb_1()
        .justify_between()
        .child(heading("Pipelines"))
        .child(subtle("All projects", cx));
    let earlier = (overview.hidden_recent > 0).then(|| {
        let runs = count(overview.hidden_recent, "earlier run", "earlier runs");
        subtle(format!("{runs} in the project libraries"), cx)
            .px_3p5()
            .pb_2p5()
    });
    v_flex()
        .w(CARD_WIDTH)
        .child(title)
        .child(
            div()
                .id("pipelines-overview-list")
                .max_h(LIST_MAX_HEIGHT)
                .overflow_y_scroll()
                .child(list),
        )
        .children(earlier)
}

fn line_row(line: Line, dismiss: &Dismiss, window: &mut Window, cx: &mut App) -> AnyElement {
    let id: ElementId = match &line.target {
        Target::Run(id) => ("pipelines-overview-run", *id).into(),
        Target::Draft(owner) => {
            SharedString::from(format!("pipelines-overview-draft-{}", owner.display())).into()
        }
    };
    let (icon, color, pill) = match line.status {
        Some(status) => (
            status_icon(status),
            status_color(status, cx),
            status.label(),
        ),
        None => (Lucide::Pencil, cx.theme().muted_foreground, "Draft"),
    };
    let bar = line.progress.and_then(|progress| {
        let filled = percent(progress)?;
        Some(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Progress::new((id.clone(), "progress"))
                        .with_size(PROGRESS_HEIGHT)
                        .flex_1()
                        .value(filled)
                        .color(color),
                )
                .child(subtle(format!("{}/{}", progress.done, progress.total), cx)),
        )
    });
    let row = h_flex()
        .id(id.clone())
        .role(Role::Button)
        .aria_label(format!("{}, {pill}", line.title))
        .items_start()
        .gap_2p5()
        .px_2()
        .py_1p5()
        .rounded_md()
        .child(
            h_flex()
                .size_6()
                .flex_none()
                .justify_center()
                .rounded_md()
                .bg(color.opacity(STATUS_TINT_OPACITY))
                .child(Icon::new(icon).size_3p5().text_color(color)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(line.title),
                        )
                        .child(status_pill(pill, color)),
                )
                .child(subtle(line.detail, cx).truncate())
                .children(bar),
        );
    if !line.reachable {
        return row.into_any_element();
    }
    let hover = Hover::new(id, window, cx);
    let (target, dismiss) = (line.target, dismiss.clone());
    row.cursor_pointer()
        .hover_bg(&hover, cx.theme().transparent, cx.theme().list_hover)
        .on_click(move |_, _, cx| {
            // The pointer rests on the card, which would otherwise cover what opens.
            dismiss.dismiss(cx);
            let pipelines = AppState::get(cx).pipelines.clone();
            pipelines.update(cx, |pipelines, cx| match target.clone() {
                Target::Run(id) => pipelines.reveal(id, cx),
                Target::Draft(owner) => pipelines.reveal_draft(owner, cx),
            });
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{UNTITLED_DRAFT, draft_title, percent, summary};
    use hive_core::pipeline::RunProgress;

    #[test]
    fn the_block_is_announced_by_what_waits_and_what_works() {
        assert_eq!(summary(0, 0), "Pipelines: nothing active");
        assert_eq!(summary(1, 0), "Pipelines: 1 needs you");
        assert_eq!(summary(2, 3), "Pipelines: 2 need you, 3 running");
        assert_eq!(summary(0, 1), "Pipelines: 1 running");
    }

    #[test]
    fn a_draft_is_named_by_the_first_line_of_its_task() {
        assert_eq!(draft_title("  Add login\nwith details"), "Add login");
        assert_eq!(draft_title(""), UNTITLED_DRAFT);
        assert_eq!(draft_title(" \n "), UNTITLED_DRAFT);
    }

    #[test]
    fn a_progress_bar_fills_by_the_share_of_finished_stages() {
        assert_eq!(percent(RunProgress { done: 0, total: 7 }), Some(0.));
        assert_eq!(percent(RunProgress { done: 7, total: 7 }), Some(100.));
        assert_eq!(percent(RunProgress { done: 1, total: 4 }), Some(25.));
        // A run saved without stages has no bar to draw.
        assert_eq!(percent(RunProgress::default()), None);
    }
}
