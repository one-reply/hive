//! Center dock and its empty state.
use super::*;

impl Workspace {
    pub(super) fn render_center(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // The center column starts at the window's top edge, so its top band is where the
        // macOS title bar would be: let it drag and double-click-zoom the window like `TitleBar`
        // does. Tabs stop mouse-down propagation, so dragging a tab still reorders it.
        let center = v_flex()
            .id("center")
            .relative()
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.should_move = event.position.y < TAB_BAR_HEIGHT
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = false),
            )
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }))
            .on_double_click(|event, window, _| {
                if event.position().y < TAB_BAR_HEIGHT {
                    window.titlebar_double_click();
                }
            });
        // With tabs open the dock's tab bar carries the right-sidebar toggle (see
        // `TerminalPanel::toolbar_buttons`); the welcome screen has no tab bar, so draw it here.
        match self.shown_center().filter(|center| !center.is_empty()) {
            Some(shown) => center.child(div().size_full().child(shown.dock().clone())),
            None => center
                .child(self.render_welcome(muted, window, cx))
                .when(!self.git_open, |el| el.child(self.render_welcome_toggle())),
        }
    }

    /// The right-sidebar toggle in the welcome screen's top-right corner. Stops mouse-down so
    /// a click does not start a window drag through the title band.
    fn render_welcome_toggle(&self) -> impl IntoElement {
        h_flex()
            .absolute()
            .top_0()
            .right_0()
            .h(TAB_BAR_HEIGHT)
            .px_2()
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(right_sidebar_toggle().small().ghost())
    }

    /// Shown while the project's dock has no tabs (the dock itself draws nothing then) or no
    /// project is selected.
    fn render_welcome(
        &self,
        muted: Hsla,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let has_selection = self.projects.read(cx).selection().is_some();
        let hint = if has_selection {
            "Open a terminal in the selected worktree to begin."
        } else {
            "Select a project from the sidebar to begin."
        };
        let shortcut = |label: &'static str, action: &dyn Action| {
            h_flex()
                .w(px(320.))
                .justify_between()
                .items_center()
                .text_sm()
                .text_color(muted)
                .child(label)
                .children(Kbd::binding_for_action(
                    action,
                    Some(keymap::CONTEXT),
                    window,
                ))
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::default()
                    .data(HIVE_MARK)
                    .size(WELCOME_LOGO_SIZE)
                    .text_color(cx.theme().foreground),
            )
            .child(div().text_3xl().font_weight(FontWeight::BOLD).child("HIVE"))
            .child(div().text_sm().text_color(muted).child(hint))
            .child(
                h_flex()
                    .gap_2()
                    .mt_2()
                    .child(
                        Button::new("welcome-add-project")
                            .cursor_pointer()
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Add project")
                            .on_click(|_, _, cx| {
                                Store::prompt_add_projects(AppState::get(cx).store.clone(), cx)
                            }),
                    )
                    .when(has_selection, |el| {
                        el.child(
                            Button::new("welcome-new-terminal")
                                .cursor_pointer()
                                .outline()
                                .icon(Lucide::SquareTerminal)
                                .label("New terminal")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.new_terminal(&NewTerminal, window, cx)
                                })),
                        )
                    }),
            )
            .when(has_selection, |el| {
                el.child(
                    v_flex()
                        .gap_1()
                        .mt_6()
                        .child(shortcut("New terminal", &NewTerminal)),
                )
            })
    }
}
