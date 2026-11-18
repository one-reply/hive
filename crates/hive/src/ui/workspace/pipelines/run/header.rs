//! The run's title, status, measurements and recovery notices above the tabs.
use super::*;

/// Longest task title shown on one line before it is cut with an ellipsis.
const TITLE_CHARS: usize = 110;
/// Stage clocks show whole seconds.
const CLOCK_TICK: Duration = Duration::from_secs(1);

impl RunView {
    /// Repaints once a second while a stage is being timed, so its clock moves.
    pub(super) fn tick_while(&mut self, recording: bool, cx: &mut Context<Self>) {
        if !recording {
            self.timer = None;
            return;
        }
        if self.timer.is_none() {
            self.timer = Some(cx.spawn(async |view, cx| {
                cx.background_executor().timer(CLOCK_TICK).await;
                let _ = view.update(cx, |this, cx| {
                    this.timer.take();
                    cx.notify();
                });
            }));
        }
    }

    pub(super) fn header(
        &mut self,
        run: &Run,
        sessions: &[LiveSession],
        range: Option<(u64, u64)>,
        stopped: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let pipelines = AppState::get(cx).pipelines.clone();
        let request = sessions
            .iter()
            .find(|s| s.request_action().is_some())
            .and_then(|s| s.status);
        let mut phase = phase_label(run);
        if let Some(status) = request {
            phase = format!("{phase} · {}", status.label());
        }
        let status = RunStatus::of(
            run.phase(),
            pipelines.read(cx).error(run.id()).is_some(),
            request.is_some(),
        );
        let mut top = v_flex().px_5().pt_4().pb_3().gap_2();
        let mut title = h_flex().gap_2().items_start().child(
            div()
                .flex_1()
                .min_w_0()
                .text_3xl()
                .font_weight(FontWeight::MEDIUM)
                .child(task_title(run.task())),
        );
        if run.task().lines().count() > 1 || run.task().chars().count() > TITLE_CHARS {
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
                    .child(SelectableText::new("full-task", run.task().to_owned())),
            );
        }
        let mut meta = h_flex()
            .gap_3()
            .flex_wrap()
            .child(status_pill(phase, status_color(status, cx)))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(Lucide::GitBranch)
                            .size_3p5()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(subtle(run.template().name.clone(), cx)),
            );
        let repos = run
            .lanes()
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
        if let Some((start, end)) = range {
            meta = meta.child(subtle(
                format!("{} elapsed", elapsed::duration(end.saturating_sub(start))),
                cx,
            ));
            let invocations = run
                .timings()
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
            top = top.children(self.checkpoint_actions(run, sessions, w, cx));
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
        top
    }

    pub(super) fn navigation(
        &self,
        sessions: &[LiveSession],
        stopped: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let tabs = TabBar::new("pipeline-run-tabs")
            .segmented()
            .selected_index(
                RunTab::ALL
                    .iter()
                    .position(|t| *t == self.tab)
                    .unwrap_or_default(),
            )
            .children(RunTab::ALL.iter().map(|tab| {
                // `Tab::icon` would drop the label, so the icon goes in as content.
                // Without its own color it follows the tab's selected and idle text.
                Tab::new().aria_label(tab.label()).cursor_pointer().child(
                    h_flex()
                        .gap_1p5()
                        .child(Icon::new(tab.icon()).size_3p5())
                        .child(tab.label()),
                )
            }))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if let Some(tab) = RunTab::ALL.get(*ix) {
                    this.tab = *tab;
                }
                if this.tab == RunTab::Changes
                    && AppState::get(cx)
                        .pipelines
                        .read(cx)
                        .review(this.id)
                        .is_none()
                {
                    this.refresh_changes(cx);
                }
                cx.notify();
            }));
        let mut navigation = h_flex()
            .px_5()
            .pb_2()
            .gap_2()
            .flex_wrap()
            .flex_none()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(tabs);
        if !stopped {
            let mut actions = h_flex().ml_auto().gap_2();
            if let [session] = sessions
                && session.request_action().is_none()
            {
                let ticket = session.ticket;
                actions = actions.child(button_with_size(
                    "open-active-session",
                    "Open agent session",
                    ButtonLook::Outline,
                    ButtonSize::Medium,
                    w,
                    cx,
                    move |v, w, cx| v.terminal(ticket, w, cx),
                ));
            }
            navigation = navigation.child(actions.child(button_with_size(
                "cancel-pipeline",
                "Stop run",
                ButtonLook::Danger,
                ButtonSize::Medium,
                w,
                cx,
                |v, w, cx| v.stop(w, cx),
            )));
        }
        navigation
    }
}

fn task_title(task: &str) -> String {
    let full = task.trim().lines().next().unwrap_or_default();
    if full.chars().count() > TITLE_CHARS {
        let cut: String = full.chars().take(TITLE_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        full.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::task_title;

    #[test]
    fn task_title_shows_the_first_line_and_cuts_long_ones_on_characters() {
        assert_eq!(task_title("  Fix login\nwith details"), "Fix login");
        let long = "я".repeat(200);
        let title = task_title(&long);
        assert_eq!(title.chars().count(), 110);
        assert!(title.ends_with('…'));
    }
}
