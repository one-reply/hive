//! Header toolbar, merge-direction strip, and aligned 3-pane column headers.
use super::*;
use crate::ui::conflict_panel::body::conflict_position;
use crate::ui::conflict_panel::rows::adjacent_conflicted_file;
use gpui_kit::component::Icon;
use hive_core::git::conflict_merge::{ConflictResolution, MergeDocument};
use hive_core::git::conflicts::{Operation, OperationContext};

pub(super) const GUTTER_WIDTH: Pixels = px(28.);
const BADGE_PADDING_X: Pixels = px(6.);
const DOT_SIZE: Pixels = px(7.);
const HEADER_TINT_OPACITY: f32 = 0.08;
const PILL_OPACITY: f32 = 0.14;
const NAVIGATOR_OPACITY: f32 = 0.35;

/// Resolved branch/commit display labels for the 3-pane header and merge strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SideNames {
    pub direction: String,
    pub current_label: String,
    pub incoming_label: String,
    pub current_commit: Option<String>,
    pub incoming_commit: Option<String>,
    pub result_label: String,
}

impl SideNames {
    pub(super) fn resolve(
        context: Option<&OperationContext>,
        current_branch: Option<&str>,
        document: Option<&MergeDocument>,
    ) -> Self {
        let first_conflict = document.and_then(|doc| doc.conflicts().next());
        let marker_current = first_conflict
            .map(|c| c.current_label.as_str())
            .filter(|label| !label.is_empty());
        let marker_incoming = first_conflict
            .map(|c| c.incoming_label.as_str())
            .filter(|label| !label.is_empty());

        if let Some(ctx) = context {
            let result_label = if ctx.operation == Operation::Rebase {
                ctx.incoming.label.clone()
            } else {
                ctx.current.label.clone()
            };
            return Self {
                direction: ctx.direction_summary(),
                current_label: ctx.current.label.clone(),
                incoming_label: ctx.incoming.label.clone(),
                current_commit: ctx.current.commit.clone(),
                incoming_commit: ctx.incoming.commit.clone(),
                result_label,
            };
        }

        let current_label = current_branch
            .or(marker_current)
            .unwrap_or("Current")
            .to_owned();
        let incoming_label = marker_incoming.unwrap_or("Incoming").to_owned();
        let direction = if marker_incoming.is_some() {
            format!("Merging {incoming_label} → {current_label}")
        } else {
            format!("Resolving unmerged changes on {current_label}")
        };
        let result_label = current_label.clone();
        Self {
            direction,
            current_label,
            incoming_label,
            current_commit: None,
            incoming_commit: None,
            result_label,
        }
    }
}

impl ConflictPanel {
    pub(super) fn toolbar(
        &self,
        progress: Option<Progress>,
        unmerged: bool,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // A file with no markers left has nothing to choose or step through.
        let resolving = progress
            .filter(|progress| progress.total > 0)
            .map(|progress| self.resolving(progress, names, window, cx));
        let file_actions = self.file_actions(progress, unmerged, window, cx);

        h_flex()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px_2()
            .py_1p5()
            .gap_x_3()
            .gap_y_1p5()
            .items_center()
            .flex_wrap()
            .border_b_1()
            .border_color(cx.theme().border)
            .when_some(resolving, |toolbar, resolving| toolbar.child(resolving))
            .child(file_actions)
    }

    /// Whole-file actions at the end of the toolbar, the primary Mark Resolved last.
    fn file_actions(
        &self,
        progress: Option<Progress>,
        unmerged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let [agent, open, refresh] = ["conflict-agent", "conflict-open-file", "conflict-refresh"]
            .map(|id| Hover::new(id, window, cx));
        h_flex()
            .ml_auto()
            .gap_1()
            .items_center()
            .child(
                button("conflict-agent")
                    .icon(Lucide::Bot)
                    .label("Resolve with Agent")
                    .tooltip("Draft a request to resolve this file for the worktree's agent")
                    .disabled(!unmerged)
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_with_agent(cx)))
                    .smooth_hover(ButtonLook::Ghost, &agent.enabled(unmerged), cx),
            )
            .child(
                button("conflict-open-file")
                    .icon(Lucide::FileCode)
                    .label("Open file")
                    .tooltip("Edit the working file in the code editor")
                    .on_click(cx.listener(|this, _, _, cx| this.open_file(None, cx)))
                    .smooth_hover(ButtonLook::Ghost, &open, cx),
            )
            .child(
                button("conflict-refresh")
                    .icon(Lucide::RefreshCw)
                    .tooltip("Reload the working file")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.conflict
                            .update(cx, |conflict, cx| conflict.refresh(cx))
                    }))
                    .smooth_hover(ButtonLook::Ghost, &refresh, cx),
            )
            .when_some(progress, |actions, progress| {
                actions
                    .child(separator(cx))
                    .child(self.mark_resolved_button(progress, window, cx))
            })
    }

    /// Shows which branches/commits are being merged and in which direction, plus aligned
    /// column headers for Current | Result | Incoming.
    pub(super) fn merge_header(
        &self,
        names: &SideNames,
        progress: Option<Progress>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let files_nav = self.conflicted_files_nav(window, cx);
        let theme = cx.theme();
        let direction_bar = h_flex()
            .w_full()
            .px_3()
            .py_1()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.25))
            .when_some(files_nav, |bar, nav| bar.child(nav))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_sm()
                    .child(names.direction.clone()),
            )
            .when_some(progress, |bar, progress| {
                bar.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(if progress.is_complete() {
                            theme.success
                        } else {
                            theme.muted_foreground
                        })
                        .child(progress.label()),
                )
            });

        if progress.is_none() {
            return direction_bar.into_any_element();
        }

        let columns = h_flex()
            .w_full()
            .min_w_0()
            .items_stretch()
            .border_b_1()
            .border_color(theme.border)
            .child(column_header(
                "Current",
                &names.current_label,
                names.current_commit.as_deref(),
                theme.success,
                cx,
            ))
            .child(div().w(GUTTER_WIDTH).flex_none())
            .child(column_header(
                "Result",
                &names.result_label,
                None,
                theme.primary,
                cx,
            ))
            .child(div().w(GUTTER_WIDTH).flex_none())
            .child(column_header(
                "Incoming",
                &names.incoming_label,
                names.incoming_commit.as_deref(),
                theme.info,
                cx,
            ));

        v_flex()
            .w_full()
            .min_w_0()
            .child(direction_bar)
            .child(columns)
            .into_any_element()
    }

    /// Compact prev/next switcher when the worktree has multiple conflicted files.
    fn conflicted_files_nav(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let request = &self.conflict.read(cx).request;
        let git = self.git.read(cx);
        if git.target.as_ref() != Some(&request.target) {
            return None;
        }
        let paths = git.conflicted_paths();
        if paths.len() <= 1 {
            return None;
        }
        let pos = paths.iter().position(|path| path == &request.path);
        let label = match pos {
            Some(ix) => format!("File {}/{}", ix + 1, paths.len()),
            None => format!("{} conflicted files", paths.len()),
        };
        let prev_path = adjacent_conflicted_file(&request.path, &paths, false);
        let next_path = adjacent_conflicted_file(&request.path, &paths, true);
        let [prev_hover, next_hover] =
            ["conflict-prev-file", "conflict-next-file"].map(|id| Hover::new(id, window, cx));

        Some(
            navigator_pill(cx)
                .when_some(prev_path, |bar, path| {
                    let tooltip: SharedString = format!("Previous conflicted file: {path}").into();
                    bar.child(
                        button("conflict-prev-file")
                            .icon(Lucide::ChevronLeft)
                            .tooltip(tooltip)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git.update(cx, |git, cx| git.open_conflict(&path, cx));
                            }))
                            .smooth_hover(ButtonLook::Ghost, &prev_hover, cx),
                    )
                })
                .child(navigator_label(label, cx))
                .when_some(next_path, |bar, path| {
                    let tooltip: SharedString = format!("Next conflicted file: {path}").into();
                    bar.child(
                        button("conflict-next-file")
                            .icon(Lucide::ChevronRight)
                            .tooltip(tooltip)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git.update(cx, |git, cx| git.open_conflict(&path, cx));
                            }))
                            .smooth_hover(ButtonLook::Ghost, &next_hover, cx),
                    )
                })
                .into_any_element(),
        )
    }

    /// Bulk choices, the conflict navigator, and the unchanged-context toggle.
    fn resolving(
        &self,
        progress: Progress,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let undecided = !progress.is_complete();
        let [current, incoming] = ["conflict-all-current", "conflict-all-incoming"]
            .map(|id| Hover::new(id, window, cx).enabled(undecided));
        let fold_hover = Hover::new("conflict-toggle-fold", window, cx);
        let navigator = self.conflict_navigator(progress.total, window, cx);

        let cur_tip: SharedString =
            format!("Accept {} in every undecided conflict", names.current_label).into();
        let inc_tip: SharedString = format!(
            "Accept {} in every undecided conflict",
            names.incoming_label
        )
        .into();
        let (fold_icon, fold_label, fold_tip) = if self.collapse_unchanged {
            (
                Lucide::UnfoldVertical,
                "Show Unchanged",
                "Show all unchanged lines in the source panes",
            )
        } else {
            (
                Lucide::FoldVertical,
                "Collapse Unchanged",
                "Collapse long unchanged sections in the source panes",
            )
        };
        let theme = cx.theme();
        // The arrows point the way the per-conflict gutter buttons do: from a side into Result.
        let current_icon = side_icon(Lucide::ChevronsRight, theme.success, undecided);
        let incoming_icon = side_icon(Lucide::ChevronsLeft, theme.info, undecided);

        h_flex()
            .gap_1()
            .items_center()
            .child(
                button("conflict-all-current")
                    .icon(current_icon)
                    .label("All Current")
                    .tooltip(cur_tip)
                    .disabled(!undecided)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.accept_all(ConflictResolution::Current, window, cx)
                    }))
                    .smooth_hover(ButtonLook::Ghost, &current, cx),
            )
            .child(
                button("conflict-all-incoming")
                    .icon(incoming_icon)
                    .label("All Incoming")
                    .tooltip(inc_tip)
                    .disabled(!undecided)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.accept_all(ConflictResolution::Incoming, window, cx)
                    }))
                    .smooth_hover(ButtonLook::Ghost, &incoming, cx),
            )
            .child(separator(cx))
            .child(navigator)
            .child(
                button("conflict-toggle-fold")
                    .icon(fold_icon)
                    .label(fold_label)
                    .tooltip(fold_tip)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_collapse_unchanged(cx)))
                    .smooth_hover(ButtonLook::Ghost, &fold_hover, cx),
            )
    }

    /// ↑/↓ around the position of the conflict they last moved to.
    fn conflict_navigator(&self, total: usize, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let [previous, next] =
            ["conflict-previous", "conflict-next"].map(|id| Hover::new(id, window, cx));
        navigator_pill(cx)
            .child(
                button("conflict-previous")
                    .icon(Lucide::ArrowUp)
                    .tooltip("Previous conflict")
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(false, window, cx)))
                    .smooth_hover(ButtonLook::Ghost, &previous, cx),
            )
            .child(navigator_label(conflict_position(self.current, total), cx))
            .child(
                button("conflict-next")
                    .icon(Lucide::ArrowDown)
                    .tooltip("Next conflict")
                    .on_click(cx.listener(|this, _, window, cx| this.navigate(true, window, cx)))
                    .smooth_hover(ButtonLook::Ghost, &next, cx),
            )
    }

    fn mark_resolved_button(
        &self,
        progress: Progress,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let conflict = self.conflict.read(cx);
        let busy = self.git.read(cx).is_busy_at(&conflict.request.target);
        let saving = conflict.is_saving();
        let can_resolve = progress.is_complete() && !busy && !saving;
        let hover = Hover::new("conflict-resolve", window, cx).enabled(can_resolve);
        button("conflict-resolve")
            .icon(Lucide::Check)
            .label("Mark Resolved")
            .tooltip(if !progress.is_complete() {
                "Choose a side or edit each conflict first"
            } else if progress.total == 0 {
                "Stage this file as the resolution"
            } else {
                "Write the merged result and stage it"
            })
            .loading(saving)
            .disabled(!can_resolve)
            .on_click(cx.listener(|this, _, _, cx| this.mark_resolved(cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx)
    }
}

fn column_header(
    role: &'static str,
    branch: &str,
    commit: Option<&str>,
    accent: Hsla,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_1()
        .min_w_0()
        .px_2p5()
        .py_1()
        .gap_1p5()
        .items_center()
        .bg(accent.opacity(HEADER_TINT_OPACITY))
        .child(div().size(DOT_SIZE).flex_none().rounded_full().bg(accent))
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(role),
        )
        .child(branch_pill(branch, accent, cx))
        .when_some(commit, |row, commit| {
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(commit.to_owned()),
            )
        })
        .when(commit.is_none(), |row| row.child(div().flex_1()))
}

fn branch_pill(label: &str, accent: Hsla, cx: &App) -> Div {
    div()
        .max_w(px(180.))
        .truncate()
        .px(BADGE_PADDING_X)
        .py(px(1.))
        .rounded_sm()
        .bg(accent.opacity(PILL_OPACITY))
        .text_color(cx.theme().foreground)
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .child(label.to_owned())
}

fn button(id: &'static str) -> Button {
    Button::new(id).cursor_pointer().small()
}

/// The shared look of the file and conflict prev/next switchers.
fn navigator_pill(cx: &App) -> Div {
    h_flex()
        .flex_none()
        .gap_0p5()
        .items_center()
        .px_1()
        .rounded_sm()
        .bg(cx.theme().muted.opacity(NAVIGATOR_OPACITY))
}

fn navigator_label(label: String, cx: &App) -> Div {
    div()
        .px_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}

/// A thin rule between toolbar groups.
fn separator(cx: &App) -> Div {
    div().flex_none().mx_1().w_px().h_4().bg(cx.theme().border)
}

/// A bulk choice's icon in its column's accent, so the button reads as that column. A disabled
/// button keeps the library's muted icon.
fn side_icon(icon: Lucide, accent: Hsla, enabled: bool) -> Icon {
    let icon = Icon::new(icon);
    if enabled {
        icon.text_color(accent)
    } else {
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::SideNames;
    use hive_core::git::conflict_merge::MergeDocument;
    use hive_core::git::conflicts::{Operation, OperationContext, SideInfo};

    #[test]
    fn side_names_prefer_operation_context_and_format_direction() {
        let doc = MergeDocument::parse("<<<<<<< HEAD\na\n=======\nb\n>>>>>>> feat\n").unwrap();
        let rebase = OperationContext {
            operation: Operation::Rebase,
            current: SideInfo {
                label: "main".into(),
                commit: Some("1111111 base".into()),
            },
            incoming: SideInfo {
                label: "feature/snap".into(),
                commit: Some("2222222 snap".into()),
            },
            step: Some((2, 3)),
        };
        let names = SideNames::resolve(Some(&rebase), None, Some(&doc));
        assert_eq!(names.current_label, "main");
        assert_eq!(names.incoming_label, "feature/snap");
        assert_eq!(names.result_label, "feature/snap");
        assert_eq!(
            names.direction,
            "Rebasing feature/snap onto main (step 2/3)"
        );

        let fallback = SideNames::resolve(None, Some("checkout"), Some(&doc));
        assert_eq!(fallback.current_label, "checkout");
        assert_eq!(fallback.incoming_label, "feat");
        assert_eq!(fallback.direction, "Merging feat → checkout");
    }
}
