//! The agents strip of a worktree card (Orca-style): an overall status, the agents' logos and a
//! count of the rest. Clicking it unfolds one row per agent session; a row shows that agent's
//! terminal pane. The strip grows in and out with the agents, so it never pops.
#[cfg(test)]
mod tests;

use std::f32::consts::FRAC_PI_2;

use hive_core::agent::{AgentKind, AgentStatus};

use super::*;
use crate::state::agents::AgentSession;
use crate::ui::agent_logo;

const STRIP_HEIGHT: Pixels = px(24.);
const SESSION_ROW_HEIGHT: Pixels = px(24.);
/// Space above the strip (to the meta row) and below the section (to the card's border).
const SECTION_TOP: Pixels = px(2.);
const SECTION_BOTTOM: Pixels = px(8.);
const LOGO_SIZE: Pixels = px(13.);
/// Logos the strip shows before collapsing the rest into "+N".
const STRIP_LOGOS: usize = 5;
const AGENTS_DURATION: Duration = Duration::from_millis(200);
const STRIP_BACKGROUND_OPACITY: f32 = 0.03;
const STRIP_HOVER_OPACITY: f32 = 0.07;
const STRIP_BORDER_OPACITY: f32 = 0.06;
const PILL_BACKGROUND_OPACITY: f32 = 0.06;

/// The agents part of one card: its eased height (the card adds it to its own) and content.
pub(super) struct AgentsSection {
    pub(super) height: Pixels,
    pub(super) element: Option<AnyElement>,
}

/// Where a card's agents live and where clicking one of them goes.
pub(super) struct CardTarget<'a> {
    pub(super) host: &'a Host,
    pub(super) project: &'a Path,
    pub(super) worktree: Option<&'a Path>,
    /// The card's own path: the worktree, or the project folder for a non-git project.
    pub(super) path: &'a Path,
}

impl Sidebar {
    pub(super) fn agents_section(
        &self,
        target: CardTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AgentsSection {
        let key = ElementId::from(SharedString::from(format!(
            "agents:{:?}:{}",
            target.host,
            target.path.display()
        )));
        let live = AppState::get(cx)
            .agents
            .read(cx)
            .sessions(target.host, target.path)
            .to_vec();
        // While the section shrinks away after the last agent quits, keep showing it.
        let shown = window.use_keyed_state((key.clone(), "shown"), cx, |_, _| Vec::new());
        if !live.is_empty() && *shown.read(cx) != live {
            shown.update(cx, |shown, _| shown.clone_from(&live));
        }
        let sessions = shown.read(cx).clone();
        let open = !live.is_empty()
            && self
                .agents_open
                .contains(&(target.host.clone(), target.path.to_path_buf()));
        let full = SECTION_TOP + STRIP_HEIGHT + SECTION_BOTTOM;
        let height = if live.is_empty() {
            px(0.)
        } else if open {
            full + SESSION_ROW_HEIGHT * live.len() as f32
        } else {
            full
        };
        let height = px(eased((key.clone(), "height"), height.as_f32(), window, cx));
        let unfolded = eased(
            (key.clone(), "open"),
            if open { 1. } else { 0. },
            window,
            cx,
        );
        if sessions.is_empty() || height < px(0.5) {
            return AgentsSection {
                height,
                element: None,
            };
        }
        let strip_hover = Hover::new((key.clone(), "strip"), window, cx);
        // Folded rows are clipped to nothing; building them would only keep their spinners
        // asking for frames.
        let rows_shown = height > full;
        // Keyed by session, so a row keeps its hover when another agent quits above it.
        let row_hovers: Vec<Hover> = sessions
            .iter()
            .filter(|_| rows_shown)
            .map(|session| {
                let id = format!("agent-row:{}:{}", target.path.display(), session.session);
                Hover::new(SharedString::from(id), window, cx)
            })
            .collect();
        let selected =
            self.projects
                .read(cx)
                .is_selected(target.host, target.project, target.worktree);
        let element = v_flex()
            .h(height)
            .flex_none()
            .overflow_hidden()
            .opacity((height / full).min(1.))
            .pt(SECTION_TOP)
            .child(self.render_strip(
                &sessions,
                &(target.host.clone(), target.path.to_path_buf()),
                &strip_hover,
                unfolded,
                selected,
                cx,
            ))
            .child(
                v_flex()
                    // Clip rows before the bottom inset, even while the section animates.
                    .h((height - full).max(px(0.)))
                    .flex_none()
                    .overflow_hidden()
                    .children(
                        sessions
                            .iter()
                            .zip(row_hovers)
                            .map(|(session, hover)| render_session(session, &target, &hover, cx)),
                    ),
            )
            .into_any_element();
        AgentsSection {
            height,
            element: Some(element),
        }
    }

    fn toggle_agents(&mut self, path: &(Host, PathBuf), cx: &mut Context<Self>) {
        if !self.agents_open.remove(path) {
            self.agents_open.insert(path.clone());
        }
        cx.notify();
    }

    fn render_strip(
        &self,
        sessions: &[AgentSession],
        path: &(Host, PathBuf),
        hover: &Hover,
        unfolded: f32,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let fg = cx.theme().sidebar_foreground;
        let muted = cx.theme().muted_foreground;
        let status = [
            AgentStatus::Failed,
            AgentStatus::WaitingApproval,
            AgentStatus::WaitingInput,
            AgentStatus::Working,
            AgentStatus::Done,
        ]
        .into_iter()
        .find(|status| sessions.iter().any(|s| s.status == *status))
        .unwrap_or(AgentStatus::Idle);
        let hidden = sessions.len().saturating_sub(STRIP_LOGOS);
        let pill = h_flex()
            .h(STRIP_HEIGHT - px(6.))
            .px_1()
            .gap_1()
            .items_center()
            .rounded_md()
            .bg(fg.opacity(PILL_BACKGROUND_OPACITY))
            .child(status_icon(status, cx))
            .children(
                sessions
                    .iter()
                    .take(STRIP_LOGOS)
                    .map(|session| logo(session.kind, cx)),
            )
            .when(hidden > 0, |pill| {
                pill.child(
                    div()
                        .text_size(BADGE_TEXT)
                        .text_color(muted)
                        .child(format!("+{hidden}")),
                )
            });
        let path_buf = path.clone();
        h_flex()
            .id(SharedString::from(format!(
                "agents-strip:{:?}:{}",
                path.0,
                path.1.display()
            )))
            .h(STRIP_HEIGHT)
            .flex_none()
            .mr_1p5()
            .pl_0p5()
            .pr_1()
            .justify_between()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(fg.opacity(STRIP_BORDER_OPACITY))
            .hover_bg(
                hover,
                fg.opacity(STRIP_BACKGROUND_OPACITY),
                fg.opacity(STRIP_HOVER_OPACITY),
            )
            .cursor_pointer()
            .child(pill)
            .child(
                Icon::new(Lucide::ChevronRight)
                    .size_3p5()
                    .text_color(muted)
                    .rotate(radians(FRAC_PI_2 * unfolded)),
            )
            .when(selected, |strip| {
                strip.on_click(cx.listener(move |this, _, _, cx| {
                    // Inactive strips let the card select its worktree instead.
                    cx.stop_propagation();
                    this.toggle_agents(&path_buf, cx);
                }))
            })
    }
}

/// One agent session: clicking it selects the card's worktree and shows the agent's pane.
fn render_session(
    session: &AgentSession,
    target: &CardTarget,
    hover: &Hover,
    cx: &mut Context<Sidebar>,
) -> Stateful<Div> {
    let fg = cx.theme().sidebar_foreground;
    let muted = cx.theme().muted_foreground;
    let (projects, agents) = {
        let app = AppState::get(cx);
        (app.projects.clone(), app.agents.clone())
    };
    let (project, worktree) = (
        target.project.to_path_buf(),
        target.worktree.map(Path::to_path_buf),
    );
    let host = target.host.clone();
    let id = session.session;
    h_flex()
        .id(SharedString::from(format!("agent-session:{}", id.as_u64())))
        .h(SESSION_ROW_HEIGHT)
        .flex_none()
        .mr_1p5()
        .px_1()
        .gap_1p5()
        .items_center()
        .rounded_md()
        .hover_bg(hover, fg.opacity(0.), fg.opacity(STRIP_HOVER_OPACITY))
        .cursor_pointer()
        .text_size(META_TEXT)
        .child(status_icon(session.status, cx))
        .child(logo(session.kind, cx))
        .when_some(session.name.clone(), |row, name| {
            row.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
        })
        .child(div().flex_none().child(session.kind.name()))
        // A remote agent reaches the sidebar only through its own OSC 7501 reports.
        .child(div().text_color(muted).child(if session.reported {
            session.status.label().to_owned()
        } else if !target.host.is_local() {
            "Remote · status unavailable".to_owned()
        } else {
            format!("{} · estimated", session.status.label())
        }))
        .when_some(session.title.clone(), |row, title| {
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(muted)
                    .child(title),
            )
        })
        .on_click(move |_, _, cx| {
            // The card behind would otherwise run its own selection on top.
            cx.stop_propagation();
            projects.update(cx, |p, cx| {
                p.select(&host, &project, worktree.as_deref(), cx)
            });
            agents.update(cx, |agents, cx| agents.reveal(id, cx));
        })
}

fn status_icon(status: AgentStatus, cx: &App) -> AnyElement {
    match status {
        AgentStatus::Working => Spinner::new()
            .icon(IconName::LoaderCircle)
            .color(rgb(ACTIVE_COLOR).into())
            .xsmall()
            .into_any_element(),
        AgentStatus::WaitingInput | AgentStatus::WaitingApproval => Icon::new(Lucide::CircleAlert)
            .size_3()
            .text_color(cx.theme().warning)
            .into_any_element(),
        AgentStatus::Done => Icon::new(Lucide::CircleCheck)
            .size_3()
            .text_color(cx.theme().success)
            .into_any_element(),
        AgentStatus::Failed => Icon::new(Lucide::CircleX)
            .size_3()
            .text_color(cx.theme().danger)
            .into_any_element(),
        AgentStatus::Idle => Icon::new(Lucide::Circle)
            .size_3()
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    }
}

fn logo(kind: AgentKind, cx: &App) -> impl IntoElement {
    Icon::default()
        .data(agent_logo::svg(kind))
        .size(LOGO_SIZE)
        .flex_none()
        .text_color(agent_logo::color(kind, cx))
}

fn eased(id: (ElementId, &'static str), target: f32, window: &mut Window, cx: &mut App) -> f32 {
    transition(
        id,
        target,
        Transition::new(AGENTS_DURATION).ease(ease_in_out),
        window,
        cx,
    )
}
