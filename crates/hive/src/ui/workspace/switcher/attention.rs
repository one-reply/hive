//! The task, agent and workspace behind an unread status report.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::*;
use hive_core::agent::{AgentKind, AgentStatus};
use hive_core::host::Host;
use hive_core::projects::{Project, Worktree};

use crate::state::agents::AgentSession;
use crate::ui::agent_logo;

// List measures one item for every row; reserve two title lines even for a short task.
const ROW_HEIGHT: Rems = rems(6.5);
const TITLE_HEIGHT: Rems = rems(2.5);
const TITLE_LINES: usize = 2;
const TITLE_LINE_HEIGHT: f32 = 1.25;

pub(in crate::ui::workspace) struct Attention {
    kind: AgentKind,
    status: AgentStatus,
    title: SharedString,
    agent: SharedString,
    location: SharedString,
}

impl Attention {
    pub(in crate::ui::workspace) fn new(
        project: &Project,
        worktree: Option<&Worktree>,
        session: &AgentSession,
    ) -> Self {
        let title = session
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or("Untitled task");
        let agent = match session.name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => format!("{} / {name}", session.kind.name()),
            None => session.kind.name().to_owned(),
        };
        let mut location = project.name.clone();
        if let Some(worktree) = worktree {
            if worktree.path != project.path {
                location.push_str(&format!(" / {}", worktree.name()));
            }
            location.push_str(&format!(
                " · {}",
                worktree.branch().unwrap_or("Detached HEAD")
            ));
        }
        if let Host::Ssh { target } = &project.host {
            location.push_str(&format!(" · SSH: {target}"));
        }
        Self {
            kind: session.kind,
            status: session.status,
            title: title.to_owned().into(),
            agent: agent.into(),
            location: location.into(),
        }
    }

    /// Title and body of the system notification for this report. `status` comes from the
    /// event itself: the listed session can still carry the previous poll's status.
    pub(in crate::ui::workspace) fn notification(&self, status: AgentStatus) -> (String, String) {
        (
            format!("{} · {}", self.agent, self.location),
            format!("{} · {}", status.label(), self.title),
        )
    }

    pub(super) fn label(&self) -> String {
        format!(
            "{} · {} · {} · {}",
            self.status.label(),
            self.title,
            self.agent,
            self.location
        )
    }

    pub(super) fn render(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let (icon, color) = match self.status {
            AgentStatus::WaitingInput | AgentStatus::WaitingApproval => {
                (Lucide::CircleAlert, theme.warning)
            }
            AgentStatus::Done => (Lucide::CircleCheck, theme.success),
            AgentStatus::Failed => (Lucide::CircleX, theme.danger),
            AgentStatus::Working | AgentStatus::Idle => (Lucide::Circle, theme.muted_foreground),
        };
        v_flex()
            .w_full()
            .h(ROW_HEIGHT)
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_3()
                    .justify_between()
                    .text_xs()
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1p5()
                            .text_color(color)
                            .child(Icon::new(icon).size_3p5())
                            .child(self.status.label()),
                    )
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_1p5()
                            .text_color(theme.muted_foreground)
                            .child(agent_logo::icon(self.kind, cx).size_3p5().flex_none())
                            .child(div().truncate().child(self.agent.clone())),
                    ),
            )
            .child(
                div()
                    .h(TITLE_HEIGHT)
                    .flex_none()
                    .text_base()
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_normal()
                    .line_height(relative(TITLE_LINE_HEIGHT))
                    .line_clamp(TITLE_LINES)
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .truncate()
                    .child(self.location.clone()),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::Attention;
    use crate::state::agents::AgentSession;
    use gpui_kit::EntityId;
    use hive_core::agent::{AgentKind, AgentStatus};
    use hive_core::host::Host;
    use hive_core::identity::ProjectIdentity;
    use hive_core::projects::{Head, Project, Worktree};
    use hive_core::state::WorktreeFilter;

    fn project() -> Project {
        Project {
            name: "omna-app".into(),
            path: "/projects/omna-app".into(),
            host: Host::Local,
            filter: WorktreeFilter::Hive,
            identity: ProjectIdentity::default(),
            worktrees: Vec::new(),
        }
    }

    fn session() -> AgentSession {
        AgentSession {
            session: EntityId::from(1),
            kind: AgentKind::ClaudeCode,
            status: AgentStatus::WaitingApproval,
            reported: true,
            unread: true,
            title: Some("Помилка збірки: node не знайдено для Pods".into()),
            name: None,
        }
    }

    #[test]
    fn primary_checkout_shows_branch_without_repeating_project() {
        let project = project();
        let worktree = Worktree {
            path: project.path.clone(),
            head: Head::Branch("develop".into()),
        };
        let details = Attention::new(&project, Some(&worktree), &session());
        assert_eq!(details.location, "omna-app · develop");
        assert_eq!(details.agent, "Claude Code");
        assert_eq!(details.status, AgentStatus::WaitingApproval);
        assert_eq!(details.title, "Помилка збірки: node не знайдено для Pods");
    }

    #[test]
    fn search_and_accessibility_keep_full_task_agent_and_remote_context() {
        let mut project = project();
        project.host = Host::Ssh {
            target: "build-server".into(),
        };
        let worktree = Worktree {
            path: "/projects/omna-app-worktrees/fix-pods".into(),
            head: Head::Branch("fix/node-version".into()),
        };
        let mut session = session();
        let title = "Довга задача з Unicode та деталями збірки ".repeat(20);
        session.title = Some(title.trim().to_owned().into());
        session.name = Some("builder".into());
        let details = Attention::new(&project, Some(&worktree), &session);
        let label = details.label();
        for expected in [
            "Needs approval",
            title.trim(),
            "Claude Code / builder",
            "omna-app / fix-pods",
            "fix/node-version",
            "SSH: build-server",
        ] {
            assert!(label.contains(expected), "missing {expected}");
        }
        assert!(!label.contains("status unavailable"));
    }

    #[test]
    fn missing_or_blank_titles_have_a_readable_fallback() {
        for title in [None, Some(""), Some(" \n\t ")] {
            let mut session = session();
            session.title = title.map(Into::into);
            let details = Attention::new(&project(), None, &session);
            assert_eq!(details.title, "Untitled task");
            assert_eq!(details.location, "omna-app");
        }
    }

    #[test]
    fn detached_checkout_and_each_attention_reason_remain_explicit() {
        let project = project();
        let worktree = Worktree {
            path: project.path.clone(),
            head: Head::Detached,
        };
        for status in [
            AgentStatus::WaitingApproval,
            AgentStatus::WaitingInput,
            AgentStatus::Done,
            AgentStatus::Failed,
        ] {
            let mut session = session();
            session.status = status;
            let details = Attention::new(&project, Some(&worktree), &session);
            assert_eq!(details.location, "omna-app · Detached HEAD");
            assert!(details.label().starts_with(status.label()));
        }
    }

    #[test]
    fn notification_names_agent_and_place_and_uses_the_event_status() {
        let project = project();
        let worktree = Worktree {
            path: project.path.clone(),
            head: Head::Branch("develop".into()),
        };
        let details = Attention::new(&project, Some(&worktree), &session());
        let (title, body) = details.notification(AgentStatus::Done);
        assert_eq!(title, "Claude Code · omna-app · develop");
        assert_eq!(
            body,
            "Response finished · Помилка збірки: node не знайдено для Pods"
        );
    }
}
