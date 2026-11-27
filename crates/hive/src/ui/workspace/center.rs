//! Center dock and its empty state.
use super::*;
use crate::ui::app_menu::AddProject;
use crate::ui::editor_panel::OpenFileSearch;
use crate::ui::key_hint;
use crate::ui::settings::OpenSettings;

const HIVE_MARK: &[u8] = include_bytes!("../../../assets/icons/Hive.icon/Assets/mark.svg");
const WELCOME_LOGO_SIZE: Pixels = px(68.);
const WELCOME_CONTENT_WIDTH: Pixels = px(336.);
const MARK_UNLIT_OPACITY: f32 = 0.24;
const PRIMARY_HINT_OPACITY: f32 = 0.12;
const MARK_REVEAL: Duration = Duration::from_millis(420);

impl Workspace {
    pub(super) fn render_center(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                .child(self.render_welcome(window, cx))
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

    /// The first action matches what can be opened in this workspace right now.
    fn render_welcome(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = self.projects.read(cx);
        let selection = projects.selection();
        let actions = welcome_actions(selection.is_some(), !projects.items.is_empty());
        let (title, detail): (SharedString, SharedString) = match selection {
            Some((project, worktree)) => (
                project.name.clone().into(),
                worktree
                    .map_or_else(
                        || project.path.to_string_lossy().into_owned(),
                        |worktree| worktree_detail(&project.name, worktree),
                    )
                    .into(),
            ),
            None if projects.items.is_empty() => (
                "Hive".into(),
                "Open a project folder to work with your coding agents.".into(),
            ),
            None => (
                "Your workspace".into(),
                "Choose a project or jump to a running agent.".into(),
            ),
        };
        let theme = cx.theme();
        let heading = h_flex()
            .id("welcome-heading")
            .role(Role::Heading)
            .aria_label(title.clone())
            .aria_description(detail.clone())
            .gap_3()
            .items_center()
            .child(welcome_mark(theme.foreground, theme.muted_foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .truncate()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .when(selection.is_some(), |detail| detail.truncate())
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    ),
            );
        let commands = v_flex()
            .gap_1()
            .children(
                actions
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, (icon, action))| {
                        welcome_command(*icon, *action, ix == 0, window, cx)
                    }),
            );
        v_flex().size_full().pt(TAB_BAR_HEIGHT).child(
            div()
                .id("welcome-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    v_flex()
                        .min_h_full()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(WELCOME_CONTENT_WIDTH)
                                .gap_5()
                                .child(heading)
                                .child(commands),
                        ),
                ),
        )
    }
}

fn worktree_detail(title: &str, worktree: &Worktree) -> String {
    let name = worktree.name();
    let branch = worktree.branch().unwrap_or("Detached HEAD");
    if name == title || name == branch {
        branch.to_owned()
    } else {
        format!("{name} · {branch}")
    }
}

fn welcome_command(
    icon: Lucide,
    action: &'static dyn Action,
    primary: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<Stateful<Div>> {
    let label = keymap::label(action)?;
    let hover = Hover::new(action.name(), window, cx);
    let hint = key_hint::for_action(action, keymap::CONTEXT, window, cx).map(|hint| {
        hint.when(primary, |hint| {
            let ink = cx.theme().button_primary_foreground;
            hint.bg(ink.opacity(PRIMARY_HINT_OPACITY)).text_color(ink)
        })
    });
    let row = h_flex()
        .w_full()
        .min_w_0()
        .gap_2()
        .child(Icon::new(icon).small())
        .child(div().flex_1().truncate().text_sm().child(label))
        .children(hint);
    let look = if primary {
        ButtonLook::Primary
    } else {
        ButtonLook::Ghost
    };
    Some(
        Button::new(action.name())
            .cursor_pointer()
            .accessibility_label(label)
            .w_full()
            .h_9()
            .child(row)
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            .smooth_hover(look, &hover, cx),
    )
}

/// Paint the light through the existing cells of the mark once, without moving the commands.
fn welcome_mark(ink: Hsla, muted: Hsla) -> impl IntoElement {
    let mark = |color| {
        Icon::default()
            .data(HIVE_MARK)
            .size(WELCOME_LOGO_SIZE)
            .text_color(color)
    };
    div()
        .relative()
        .size(WELCOME_LOGO_SIZE)
        .flex_none()
        .child(mark(muted.opacity(MARK_UNLIT_OPACITY)))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .overflow_hidden()
                .child(mark(ink))
                .with_animation(
                    "hive-mark-reveal",
                    Animation::new(MARK_REVEAL).with_easing(ease_in_out),
                    |mask, t| mask.w(WELCOME_LOGO_SIZE * t),
                ),
        )
}

fn welcome_actions(selected: bool, has_projects: bool) -> &'static [(Lucide, &'static dyn Action)] {
    if selected {
        &[
            (Lucide::SquareTerminal, &NewTerminal),
            (Lucide::FileSearch, &OpenFileSearch),
            (Lucide::Search, &QuickSwitch),
            (Lucide::FolderPlus, &AddProject),
            (Lucide::Command, &ShowCommands),
        ]
    } else if has_projects {
        &[
            (Lucide::Search, &QuickSwitch),
            (Lucide::FolderPlus, &AddProject),
            (Lucide::Command, &ShowCommands),
            (Lucide::Settings, &OpenSettings),
        ]
    } else {
        &[
            (Lucide::FolderPlus, &AddProject),
            (Lucide::Command, &ShowCommands),
            (Lucide::Settings, &OpenSettings),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AddProject, NewTerminal, OpenFileSearch, QuickSwitch, welcome_actions, worktree_detail,
    };
    use gpui_kit::Action as _;
    use hive_core::projects::{Head, Worktree};

    #[test]
    fn the_primary_command_follows_the_workspace_and_file_search_requires_a_selection() {
        for (selected, projects, first) in [
            (false, false, AddProject.name()),
            (false, true, QuickSwitch.name()),
            (true, true, NewTerminal.name()),
        ] {
            let actions = welcome_actions(selected, projects);
            assert_eq!(actions[0].1.name(), first);
            assert_eq!(
                actions
                    .iter()
                    .any(|(_, action)| action.name() == OpenFileSearch.name()),
                selected
            );
            assert!(
                actions
                    .iter()
                    .all(|(_, action)| crate::ui::keymap::label(*action).is_some())
            );
        }
    }

    #[test]
    fn worktree_context_names_distinct_folders_and_detached_heads() {
        let mut tree = Worktree {
            path: "/src/hive".into(),
            head: Head::Branch("develop".into()),
        };
        assert_eq!(worktree_detail("hive", &tree), "develop");
        tree.path = "/trees/review".into();
        assert_eq!(worktree_detail("hive", &tree), "review · develop");
        tree.head = Head::Detached;
        assert_eq!(worktree_detail("hive", &tree), "review · Detached HEAD");
    }
}
