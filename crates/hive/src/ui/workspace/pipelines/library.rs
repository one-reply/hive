//! The library previews executable routes and the selected project's actual runs.
use super::*;
use crate::state::pipelines::RunStatus;
use crate::ui::hover::{SmoothHoverBg as _, WIDE_HOVER_DURATION};
use crate::ui::list_scroll::replace_rows;
use gpui_kit::component::{
    button::ButtonVariants as _,
    menu::{ContextMenuExt as _, PopupMenu},
};

mod runs;
use runs::{RunRow, run_detail, run_row};

/// Strength of the selection accent a hovered, unselected template card's border eases to.
const HOVER_BORDER_OPACITY: f32 = 0.6;
/// Space around the page, the same as around the other pipeline screens.
const PAGE_PADDING: Pixels = px(24.);
/// Space between the template cards and the run history.
const WORKFLOWS_GAP: Pixels = px(32.);
const ROW_GAP: Pixels = px(4.);
/// How far beyond the view rows are drawn, so scrolling and Tab meet rows that are already there.
pub(super) const HISTORY_OVERDRAW: Pixels = px(240.);
/// How often the ages of the listed runs are worded anew.
const AGE_TICK: Duration = Duration::from_secs(60);
const ELSEWHERE: &str = "Needs you in other projects";

/// One entry of the library page. The page is a virtual list, so a history of any length
/// costs only the rows in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Entry {
    /// The title, the actions and the template cards.
    Workflows,
    Section(&'static str),
    Run(u64),
    /// A run of another project that waits for the user.
    Elsewhere(u64),
    NoRuns,
}

/// The page from top to bottom. `runs` are the project's runs, newest first, and every
/// section keeps that order; a section without runs has no heading.
fn entries(runs: &[(u64, RunStatus)], elsewhere: &[u64]) -> Vec<Entry> {
    let mut entries = vec![Entry::Workflows];
    let mut sorted = runs.to_vec();
    // The sort is stable, so each section keeps the newest run first.
    sorted.sort_by_key(|(_, status)| history_section(*status).0);
    let mut previous = None;
    for (id, status) in sorted {
        let (_, section) = history_section(status);
        if previous != Some(section) {
            entries.push(Entry::Section(section));
            previous = Some(section);
        }
        entries.push(Entry::Run(id));
    }
    if !elsewhere.is_empty() {
        entries.push(Entry::Section(ELSEWHERE));
        entries.extend(elsewhere.iter().copied().map(Entry::Elsewhere));
    }
    if runs.is_empty() {
        entries.push(Entry::NoRuns);
    }
    entries
}

impl PipelinePanel {
    /// The library page as a virtual list: the workflows block, then the project's runs.
    pub(super) fn library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let state = AppState::get(cx);
        let pipelines = state.pipelines.read(cx);
        let runs: Vec<_> = state
            .store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .rev()
            .filter(|run| run.owner() == self.project)
            .map(|run| (run.id(), pipelines.status(run, cx)))
            .collect();
        let elsewhere: Vec<_> = pipelines
            .attention(cx)
            .into_iter()
            .filter(|(_, owner, _)| owner != &self.project)
            .map(|(id, ..)| id)
            .collect();
        let entries: Rc<[Entry]> = entries(&runs, &elsewhere).into();
        if *self.entries != *entries {
            replace_rows(&self.history, &self.entries, &entries);
            self.entries = entries.clone();
        }
        if !runs.is_empty() {
            self.keep_ages_current(cx);
        }
        let view = cx.entity().downgrade();
        let page = list(self.history.clone(), move |ix, window, cx| {
            view.update(cx, |this, cx| this.entry(&entries, ix, window, cx))
                .unwrap_or_else(|_| div().into_any_element())
        });
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(page.size_full())
            .into_any_element()
    }

    /// Run ages are worded to the minute, and the cached panel repaints only when told to.
    /// The tick is armed by a render, so it stops once the library is no longer drawn.
    fn keep_ages_current(&mut self, cx: &mut Context<Self>) {
        if self.age_tick.is_some() {
            return;
        }
        self.age_tick = Some(cx.spawn(async move |panel, cx| {
            cx.background_executor().timer(AGE_TICK).await;
            let _ = panel.update(cx, |panel, cx| {
                panel.age_tick = None;
                cx.notify();
            });
        }));
    }

    fn entry(
        &self,
        entries: &[Entry],
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = entries.get(ix) else {
            return div().into_any_element();
        };
        let body = match *entry {
            Entry::Workflows => self.workflows(window, cx).into_any_element(),
            Entry::Section(title) => heading(title).into_any_element(),
            Entry::Run(id) => self.run_entry(id, window, cx),
            Entry::Elsewhere(id) => self.elsewhere_entry(id, window, cx),
            Entry::NoRuns => h_flex()
                .gap_2()
                .py_3()
                .child(
                    Icon::new(Lucide::Clock)
                        .size_4()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(subtle("No runs in this project yet.", cx))
                .into_any_element(),
        };
        let below = if ix + 1 == entries.len() {
            PAGE_PADDING
        } else if *entry == Entry::Workflows {
            WORKFLOWS_GAP
        } else {
            ROW_GAP
        };
        div()
            .px(PAGE_PADDING)
            .when(ix == 0, |el| el.pt(PAGE_PADDING))
            .pb(below)
            .child(body)
            .into_any_element()
    }

    fn run_entry(&self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = AppState::get(cx);
        let runs = &state.store.read(cx).state.pipelines.runs;
        let Some(run) = runs.iter().find(|run| run.id() == id) else {
            return div().into_any_element();
        };
        let status = state.pipelines.read(cx).status(run, cx);
        let finished = matches!(status, RunStatus::Completed | RunStatus::Cancelled);
        let now = hive_core::pipeline::now_ms();
        // A finished run's phase repeats its status, and a live one's length is still
        // growing, so each row shows only the one that says something.
        let detail = run_detail(
            &run.template().name,
            (!finished).then(|| phase_label(run)).as_deref(),
            run.age(now).as_deref(),
            finished.then(|| run::length(run, now)).flatten().as_deref(),
        );
        let row = RunRow {
            id,
            task: run.task().to_owned(),
            detail,
            status,
            finished,
        };
        run_row(row, window, cx)
    }

    /// A run of another project that waits for the user; a click goes to it.
    fn elsewhere_entry(&self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let waiting = AppState::get(cx).pipelines.read(cx).attention(cx);
        let Some((_, owner, task)) = waiting.into_iter().find(|(run, ..)| *run == id) else {
            return div().into_any_element();
        };
        button(
            ("other-project-run", id),
            format!("{} · {}", path_name(&owner), task),
            ButtonLook::Ghost,
            window,
            cx,
            move |_, _, cx| {
                AppState::get(cx)
                    .pipelines
                    .clone()
                    .update(cx, |p, cx| p.reveal(id, cx))
            },
        )
        .into_any_element()
    }

    /// The title, the actions and the template cards.
    fn workflows(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let templates = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .templates
            .clone();
        let draft_template = self.draft_template(cx);
        let selected = templates
            .iter()
            .position(|t| Some(&t.name) == self.selected_template.as_ref())
            .unwrap_or(0);
        let mut actions = h_flex().gap_2().flex_wrap().child(button_with_size(
            "new-template",
            "New template",
            ButtonLook::Outline,
            ButtonSize::Medium,
            window,
            cx,
            |p, w, cx| p.edit(None, w, cx),
        ));
        let has_draft = draft_template.is_some();
        if let Some(launch) = draft_template.or_else(|| templates.get(selected).cloned()) {
            actions = actions.child(button_with_size(
                "launch-template",
                if has_draft {
                    "Resume draft"
                } else {
                    "Start run"
                },
                ButtonLook::Primary,
                ButtonSize::Medium,
                window,
                cx,
                move |p, w, cx| p.launch(launch.clone(), w, cx),
            ));
        }
        let mut cards = h_flex().items_stretch().gap_3().flex_wrap();
        for (i, t) in templates.iter().enumerate() {
            let id = SharedString::from(format!("workflow-template-{}", t.name));
            let hover = Hover::new(id.clone(), window, cx);
            let selected = selected == i;
            let name = t.name.clone();
            let pick = move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                this.selected_template = Some(name.clone());
                cx.notify()
            };
            let summary = format!(
                "{}   ·   {}   ·   2 decisions",
                count(t.lanes.len(), "lane", "lanes"),
                count(t.reviews.len(), "review", "reviews")
            );
            let accent = cx.theme().blue;
            let panel = cx.entity();
            let template = t.clone();
            let menu = move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
                template_menu(menu, &panel, &template)
            };
            let more = Button::new(("workflow-menu", i))
                .cursor_pointer()
                .ghost()
                .xsmall()
                .icon(Lucide::Ellipsis)
                .accessibility_label(format!("Actions for {}", t.name))
                .tooltip("Workflow actions")
                .dropdown_menu(menu.clone());
            let bottom = v_flex()
                .px_3()
                .py_2p5()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .flex_1()
                                .truncate()
                                .child(t.name.clone()),
                        )
                        .child(
                            Icon::new(if selected {
                                Lucide::CircleCheck
                            } else {
                                Lucide::Circle
                            })
                            .size_4()
                            .text_color(if selected {
                                accent
                            } else {
                                cx.theme().border
                            }),
                        ),
                )
                .child(
                    h_flex()
                        .justify_between()
                        .child(subtle(summary, cx))
                        .child(more),
                );
            let card = v_flex()
                .id(id)
                .role(Role::Button)
                .aria_label(t.name.clone())
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tab_index(0)
                .min_w(rems(11.))
                .max_w(rems(20.))
                .flex_1()
                .border_1()
                .bg(cx.theme().sidebar)
                .border_color(if selected {
                    accent
                } else {
                    hover.mix(cx.theme().border, accent.opacity(HOVER_BORDER_OPACITY))
                })
                .on_hover(hover.listener())
                .rounded_lg()
                .overflow_hidden()
                .cursor_pointer()
                .focus_visible(move |s| s.border_color(accent))
                .child(map::miniature(t, cx))
                .child(bottom);
            cards = cards.child(on_activate(card, cx, pick).context_menu(menu));
        }
        if templates.is_empty() {
            cards = cards.child(subtle("Create a template to start a run.", cx));
        }
        v_flex()
            .gap_5()
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .justify_between()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Workflows"),
                    )
                    .child(actions),
            )
            .child(cards)
    }
}

fn template_menu(menu: PopupMenu, panel: &Entity<PipelinePanel>, template: &Template) -> PopupMenu {
    let duplicate_panel = panel.clone();
    let source = template.clone();
    let panel = panel.clone();
    let edit = template.clone();
    let delete = template.clone();
    menu.item(
        PopupMenuItem::new("Edit workflow")
            .icon(Lucide::Pencil)
            .on_click(move |_, window, cx| {
                panel.update(cx, |p, cx| p.edit(Some(edit.clone()), window, cx));
            }),
    )
    .item(
        PopupMenuItem::new("Duplicate template")
            .icon(Lucide::Copy)
            .on_click(move |_, window, cx| {
                let pipelines = AppState::get(cx).pipelines.clone();
                match pipelines.update(cx, |p, cx| p.duplicate_template(&source, cx)) {
                    Ok(copy) => duplicate_panel.update(cx, |p, cx| {
                        p.selected_template = Some(copy.name.clone());
                        p.edit(Some(copy), window, cx);
                    }),
                    Err(error) => window.push_notification(Notification::error(error), cx),
                }
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("Delete template…")
            .icon(Lucide::Trash)
            .on_click(move |_, window, cx| confirm_delete(&delete, window, cx)),
    )
}

fn confirm_delete(template: &Template, window: &mut Window, cx: &mut App) {
    let template = template.clone();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let template = template.clone();
        crate::ui::destructive::confirmation(alert
            .confirm()
            .title(format!("Delete “{}”?", template.name))
            .description(
                "This removes the template from all projects. Existing runs keep their workflow.",
            ), "Delete", move |window, cx| {
                let pipelines = AppState::get(cx).pipelines.clone();
                if let Err(error) = pipelines.update(cx, |p, cx| p.delete_template(&template, cx)) {
                    window.push_notification(Notification::error(error), cx);
                }
                true
            })
    });
}

/// Where a run is listed in the history, in display order: what waits for the user comes
/// before what is working, and finished runs come last.
fn history_section(status: RunStatus) -> (u8, &'static str) {
    match status {
        RunStatus::NeedsYou | RunStatus::Failed => (0, "Needs you"),
        RunStatus::Running => (1, "Running"),
        RunStatus::Completed | RunStatus::Cancelled => (2, "Recent"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ELSEWHERE, Entry, RunStatus, entries, history_section};

    #[test]
    fn the_page_lists_each_section_under_its_heading_newest_run_first() {
        use RunStatus::*;
        // Newest first, as the store is read.
        let runs = [(5, Completed), (4, Running), (3, NeedsYou), (2, Cancelled), (1, Failed)];
        assert_eq!(
            entries(&runs, &[9]),
            [
                Entry::Workflows,
                Entry::Section("Needs you"),
                Entry::Run(3),
                Entry::Run(1),
                Entry::Section("Running"),
                Entry::Run(4),
                Entry::Section("Recent"),
                Entry::Run(5),
                Entry::Run(2),
                Entry::Section(ELSEWHERE),
                Entry::Elsewhere(9),
            ]
        );
    }

    #[test]
    fn a_project_without_runs_says_so_under_the_workflows() {
        assert_eq!(entries(&[], &[]), [Entry::Workflows, Entry::NoRuns]);
        assert_eq!(
            entries(&[], &[9]),
            [
                Entry::Workflows,
                Entry::Section(ELSEWHERE),
                Entry::Elsewhere(9),
                Entry::NoRuns,
            ]
        );
    }

    #[test]
    fn runs_that_wait_for_the_user_are_listed_before_working_and_finished_ones() {
        use RunStatus::*;
        let mut runs = [Completed, Running, Failed, Cancelled, NeedsYou];
        runs.sort_by_key(|status| history_section(*status).0);
        assert_eq!(runs, [Failed, NeedsYou, Running, Completed, Cancelled]);
        assert_eq!(history_section(Failed).1, history_section(NeedsYou).1);
        assert_eq!(history_section(Completed).1, history_section(Cancelled).1);
    }
}
