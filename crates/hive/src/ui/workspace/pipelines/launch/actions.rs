//! Starting or discarding the run: a bar pinned under the form, so the action and what
//! still blocks it stay in view however long the form grows.
use super::*;
use crate::ui::key_hint;
use gpui_kit::component::input::Enter;

/// How much of the button's text color the shortcut keeps, so the label stays the loudest.
const SHORTCUT_OPACITY: f32 = 0.7;

impl LaunchForm {
    /// The text fields bind secondary Enter themselves and a multi-line one would add a
    /// line, so a run that can start takes the key before they see it.
    pub(super) fn start_on_secondary_enter(
        &mut self,
        enter: &Enter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if enter.secondary && self.blocked_by(cx).is_none() {
            cx.stop_propagation();
            self.submit(window, cx);
        }
    }

    pub(super) fn action_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let blocked = self.blocked_by(cx);
        let direct = self.is_direct();
        let shortcut = Keystroke::parse_with_os("secondary-enter", cx.platform_info().os)
            .inspect_err(|err| tracing::error!(%err, "start shortcut does not parse"))
            .ok()
            .map(|key| div().opacity(SHORTCUT_OPACITY).child(key_hint::label(&key)));
        let hover = Hover::new("start-planning", window, cx).enabled(blocked.is_none());
        let submit = Button::new("start-planning")
            .cursor_pointer()
            .with_size(ButtonSize::Medium)
            .icon(Lucide::Play)
            .label(if direct {
                "Start development"
            } else {
                "Start planning"
            })
            .children(shortcut)
            .disabled(blocked.is_some())
            .on_click(cx.listener(|this, _, w, cx| this.submit(w, cx)))
            .smooth_hover(ButtonLook::Primary, &hover, cx);
        let discard = button_with_size(
            "discard-launch-draft",
            "Discard draft",
            ButtonLook::Ghost,
            ButtonSize::Medium,
            window,
            cx,
            |v, w, cx| {
                v.abandon();
                let _ = v.parent.update(cx, |p, cx| {
                    p.discard_draft(cx);
                    p.home(w, cx);
                });
            },
        );
        let status = blocked.map_or(
            if direct {
                "QA and your final acceptance follow development."
            } else {
                "You approve the plan before development."
            },
            Blocker::message,
        );
        let row = h_flex()
            .w_full()
            .gap_2()
            .child(div().flex_1().min_w_0().child(subtle(status, cx)))
            .child(discard)
            .child(submit);
        h_flex()
            .flex_none()
            .justify_center()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .w_full()
                    .max_w(COMPOSER_WIDTH)
                    .gap_2()
                    .when_some(self.error.clone(), |bar, error| {
                        bar.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
                    .child(row),
            )
    }
}
