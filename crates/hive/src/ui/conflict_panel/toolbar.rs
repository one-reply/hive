//! The resolver's header: progress, bulk choices, navigation and the resolving action.
use super::*;

impl ConflictPanel {
    /// `progress` is `None` when the tab shows no file to resolve in place, which leaves
    /// only the actions on the file itself.
    pub(super) fn toolbar(
        &self,
        progress: Option<Progress>,
        unmerged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let [agent, open, refresh] = ["conflict-agent", "conflict-open-file", "conflict-refresh"]
            .map(|id| Hover::new(id, window, cx));
        let title = progress.map_or("Conflicts".into(), Progress::label);
        let resolving = progress.map(|progress| self.resolving(progress, window, cx));
        h_flex()
            .p_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().min_w_0().truncate().text_sm().child(title))
            .when_some(resolving, |toolbar, [current, incoming, previous, next]| {
                toolbar
                    .child(current)
                    .child(incoming)
                    .child(previous)
                    .child(next)
            })
            .child(
                button("conflict-agent")
                    .label("Resolve with Agent")
                    .tooltip("Draft a request to resolve this file for the worktree's agent")
                    .disabled(!unmerged)
                    .on_click(cx.listener(|this, _, _, cx| this.resolve_with_agent(cx)))
                    .smooth_hover(ButtonLook::Ghost, &agent.enabled(unmerged), cx),
            )
            .child(
                button("conflict-open-file")
                    .label("Open file")
                    .tooltip("Edit the working file to resolve it by hand")
                    .on_click(cx.listener(|this, _, _, cx| this.open_file(None, cx)))
                    .smooth_hover(ButtonLook::Ghost, &open, cx),
            )
            .child(
                button("conflict-refresh")
                    .label("↻")
                    .tooltip("Reload the working file")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.conflict
                            .update(cx, |conflict, cx| conflict.refresh(cx))
                    }))
                    .smooth_hover(ButtonLook::Ghost, &refresh, cx),
            )
            .when_some(progress, |toolbar, progress| {
                toolbar.child(self.mark_resolved_button(progress, window, cx))
            })
    }

    /// Bulk choices and the arrows between conflicts, in toolbar order.
    fn resolving(
        &self,
        progress: Progress,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [Stateful<Div>; 4] {
        let undecided = !progress.is_complete();
        let has_conflicts = progress.total > 0;
        let [current, incoming] = ["conflict-all-current", "conflict-all-incoming"]
            .map(|id| Hover::new(id, window, cx).enabled(undecided));
        let [previous, next] = ["conflict-previous", "conflict-next"]
            .map(|id| Hover::new(id, window, cx).enabled(has_conflicts));
        [
            button("conflict-all-current")
                .label("All Current")
                .tooltip("Accept the current change in every undecided conflict")
                .disabled(!undecided)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.accept_all(DiffConflictResolution::Current, cx)
                }))
                .smooth_hover(ButtonLook::Ghost, &current, cx),
            button("conflict-all-incoming")
                .label("All Incoming")
                .tooltip("Accept the incoming change in every undecided conflict")
                .disabled(!undecided)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.accept_all(DiffConflictResolution::Incoming, cx)
                }))
                .smooth_hover(ButtonLook::Ghost, &incoming, cx),
            button("conflict-previous")
                .label("↑")
                .tooltip("Previous conflict")
                .disabled(!has_conflicts)
                .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx)))
                .smooth_hover(ButtonLook::Ghost, &previous, cx),
            button("conflict-next")
                .label("↓")
                .tooltip("Next conflict")
                .disabled(!has_conflicts)
                .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx)))
                .smooth_hover(ButtonLook::Ghost, &next, cx),
        ]
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
            .label("Mark Resolved")
            .tooltip(if !progress.is_complete() {
                "Choose a side for every conflict first"
            } else if progress.total == 0 {
                "Stage this file as the resolution"
            } else {
                "Write the chosen sides to the file and stage it"
            })
            .loading(saving)
            .disabled(!can_resolve)
            .on_click(cx.listener(|this, _, _, cx| this.mark_resolved(cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx)
    }
}

fn button(id: &'static str) -> Button {
    Button::new(id).cursor_pointer().small()
}
