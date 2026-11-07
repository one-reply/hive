//! Center dock and its empty state.
use super::*;
use crate::ui::app_menu::AddProject;
use crate::ui::editor_panel::OpenFileSearch;
use crate::ui::key_hint;
use crate::ui::settings::OpenSettings;

impl Workspace {
    pub(super) fn render_center(
        &self,
        window: &mut Window,
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
            .min_h_0();
        let center = self.window_drag_band(center, |position| position.y < TAB_BAR_HEIGHT, cx);
        // With tabs open the dock's tab bar carries the right-sidebar toggle (see
        // `TerminalPanel::toolbar_buttons`); the welcome screen has no tab bar, so draw it here.
        match self.shown_center().filter(|center| !center.is_empty()) {
            Some(shown) => center.child(div().size_full().child(shown.dock().clone())),
            None => center
                .child(self.render_welcome(muted, window, cx))
                .child(self.render_welcome_band(cx))
                .when(!self.git_open, |el| {
                    el.child(self.render_welcome_toggle(window, cx))
                }),
        }
    }

    /// The welcome screen's top band, ruled off like the title bar so its bottom line runs
    /// across the window instead of stopping at the title bar's edge.
    fn render_welcome_band(&self, cx: &App) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(TAB_BAR_HEIGHT)
            .border_b_1()
            .border_color(cx.theme().title_bar_border)
    }

    /// The right-sidebar toggle in the welcome screen's top-right corner. Stops mouse-down so
    /// a click does not start a window drag through the title band.
    fn render_welcome_toggle(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hover = Hover::new("toggle-right-sidebar", window, cx);
        h_flex()
            .absolute()
            .top_0()
            .right_0()
            .h(TAB_BAR_HEIGHT)
            .px_2()
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                right_sidebar_toggle()
                    .small()
                    .smooth_hover(ButtonLook::Ghost, &hover, cx),
            )
    }

    /// Shown while the project's dock has no tabs (the dock itself draws nothing then) or no
    /// project is selected.
    fn render_welcome(
        &self,
        muted: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let add_hover = Hover::new("welcome-add-project", window, cx);
        let terminal_hover = Hover::new("welcome-new-terminal", window, cx);
        let projects = self.projects.read(cx);
        let has_projects = !projects.items.is_empty();
        let has_selection = projects.selection().is_some();
        let hint = match (has_selection, has_projects) {
            (true, _) => "Open a terminal in the selected worktree to begin.",
            (false, true) => "Select a project from the sidebar to begin.",
            (false, false) => "Add a project folder to get started.",
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
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Control center for your AI coding agents"),
            )
            .child(div().mt_2().text_sm().text_color(muted).child(hint))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("welcome-add-project")
                            .cursor_pointer()
                            .icon(IconName::FolderOpen)
                            .label("Add project")
                            .on_click(|_, window, cx| crate::ui::add_project::open(window, cx))
                            .smooth_hover(ButtonLook::Outline, &add_hover, cx),
                    )
                    .when(has_selection, |el| {
                        el.child(
                            Button::new("welcome-new-terminal")
                                .cursor_pointer()
                                .icon(Lucide::SquareTerminal)
                                .label("New terminal")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.new_terminal(&NewTerminal, window, cx)
                                }))
                                .smooth_hover(ButtonLook::Outline, &terminal_hover, cx),
                        )
                    }),
            )
            .child(self.render_shortcuts(has_selection, muted, window, cx))
    }

    /// The window-wide shortcuts that make sense before anything is open.
    fn render_shortcuts(&self, has_selection: bool, muted: Hsla, window: &Window, cx: &App) -> Div {
        let actions: &[&dyn Action] = if has_selection {
            &[
                &NewTerminal,
                &OpenFileSearch,
                &QuickSwitch,
                &ToggleSidebar,
                &ToggleGitPanel,
                &OpenSettings,
            ]
        } else {
            &[&AddProject, &QuickSwitch, &ToggleSidebar, &OpenSettings]
        };
        v_flex()
            .w(WELCOME_CONTENT_WIDTH)
            .gap_1p5()
            .mt_6()
            .children(actions.iter().filter_map(|action| {
                let label = keymap::label(*action)?;
                Some(
                    h_flex()
                        .justify_between()
                        .items_center()
                        .text_sm()
                        .text_color(muted)
                        .child(label)
                        .children(key_hint::for_action(*action, keymap::CONTEXT, window, cx)),
                )
            }))
    }
}
