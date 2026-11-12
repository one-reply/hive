//! Past Claude Code and Codex conversations of the selected worktree; choosing one resumes it
//! in a new tab, or shows the tile that already runs it.
use super::*;
use crate::ui::key_hint;
use gpui_kit::component::{
    IndexPath,
    list::{List, ListDelegate, ListState},
    spinner::Spinner,
};
use hive_core::agent::conversation::history::{Conversation, Origin};
use hive_core::agent::launch::ResumeSession;
use std::time::{Duration, Instant};

mod row;
use row::Row;

/// A quick read never shows a spinner; a slow one fades it in after this.
const SPINNER_DELAY: Duration = Duration::from_millis(150);
const SPINNER_FADE: Duration = Duration::from_millis(180);
/// Rows rise and fade in one after another as the list first appears.
const ENTRANCE_DURATION: Duration = Duration::from_millis(180);
const ENTRANCE_STAGGER: Duration = Duration::from_millis(24);
/// Rows past this one appear together with it.
const STAGGERED_ROWS: u32 = 8;

enum Load {
    Reading,
    Ready(Vec<Conversation>),
    Remote,
}

struct History {
    host: Host,
    worktree: PathBuf,
    load: Load,
    query: String,
    /// Indexes into the loaded conversations that match `query`.
    matching: Vec<usize>,
    running: HashMap<ResumeSession, EntityId>,
    selected: Option<IndexPath>,
    opened: Instant,
    /// When the list arrived; `None` under reduced motion, so rows never animate.
    arrived: Option<Instant>,
    /// Dropping the dialog cancels a read still in progress.
    _load: Option<Task<()>>,
}

impl History {
    fn conversations(&self) -> &[Conversation] {
        match &self.load {
            Load::Ready(conversations) => conversations,
            Load::Reading | Load::Remote => &[],
        }
    }

    fn refilter(&mut self) {
        self.matching = self
            .conversations()
            .iter()
            .enumerate()
            .filter(|(_, conversation)| {
                matches(&self.query, &title(conversation), agent_label(conversation))
            })
            .map(|(ix, _)| ix)
            .collect();
    }

    fn loaded(&mut self, conversations: Vec<Conversation>, now: Instant, reduce_motion: bool) {
        self.load = Load::Ready(conversations);
        self.arrived = (!reduce_motion).then_some(now);
        self.refilter();
    }

    fn reading_placeholder(&self, window: &mut Window, cx: &App) -> AnyElement {
        let waited = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.opened);
        let Some(shown) = waited.checked_sub(SPINNER_DELAY) else {
            window.request_animation_frame();
            return div().into_any_element();
        };
        let opacity = if cx.reduce_motion() {
            1.
        } else {
            ease_in_out((shown.as_secs_f32() / SPINNER_FADE.as_secs_f32()).min(1.))
        };
        if opacity < 1. {
            window.request_animation_frame();
        }
        h_flex()
            .p_4()
            .gap_2()
            .opacity(opacity)
            .text_color(cx.theme().muted_foreground)
            .child(Spinner::new().xsmall())
            .child(empty_message(&self.load, &self.query))
            .into_any_element()
    }
}

impl ListDelegate for History {
    type Item = Row;

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.matching.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Row> {
        let conversation = self.conversations().get(*self.matching.get(ix.row)?)?;
        let entrance = self.arrived.and_then(|arrived| {
            let elapsed = cx
                .background_executor()
                .now()
                .saturating_duration_since(arrived);
            entrance(elapsed, ix.row)
        });
        if entrance.is_some() {
            window.request_animation_frame();
        }
        let session = conversation.session();
        let detail = format!("{} · {}", agent_label(conversation), conversation.age());
        Some(
            Row::new(
                format!("history-{}", session.id()).into(),
                session.agent(),
                title(conversation).into(),
                detail.into(),
            )
            .open(self.running.contains_key(session))
            .entrance(entrance),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.into();
        self.refilter();
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(conversation) = self
            .selected
            .and_then(|ix| self.matching.get(ix.row))
            .and_then(|&ix| self.conversations().get(ix))
        else {
            return;
        };
        let session = conversation.session().clone();
        let (host, worktree) = (self.host.clone(), self.worktree.clone());
        window.close_dialog(cx);
        // Run after the dialog returns focus; otherwise it would steal focus from the new tab.
        window.defer(cx, move |_, cx| {
            let agents = AppState::get(cx).agents.clone();
            agents.update(cx, |agents, cx| agents.resume(host, worktree, session, cx));
        });
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.close_dialog(cx);
    }

    fn render_empty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        match self.load {
            Load::Reading => self.reading_placeholder(window, cx),
            Load::Ready(_) | Load::Remote => div()
                .p_4()
                .text_color(cx.theme().muted_foreground)
                .child(empty_message(&self.load, &self.query))
                .into_any_element(),
        }
    }
}

/// Opens the history of the selected worktree, or of the project when it has none.
pub(crate) fn open_selected(window: &mut Window, cx: &mut App) {
    let projects = AppState::get(cx).projects.read(cx);
    let Some((project, worktree)) = projects.selection() else {
        return;
    };
    let host = project.host.clone();
    let path = worktree.map_or_else(|| project.path.clone(), |w| w.path.clone());
    let place = match worktree.and_then(|w| w.branch()) {
        Some(branch) => format!("{} / {branch}", project.name),
        None => project.name.clone(),
    };
    open(host, path, place, window, cx);
}

fn open(host: Host, worktree: PathBuf, place: String, window: &mut Window, cx: &mut App) {
    let app = AppState::get(cx);
    let agents = app.agents.clone();
    let now = cx.background_executor().now();
    let local = host.is_local();
    let delegate = History {
        load: if local { Load::Reading } else { Load::Remote },
        running: agents.read(cx).running(cx),
        host: host.clone(),
        worktree: worktree.clone(),
        query: String::new(),
        matching: Vec::new(),
        selected: None,
        opened: now,
        arrived: None,
        _load: None,
    };
    let list = cx.new(|cx| ListState::new(delegate, window, cx).searchable(true));
    list.update(cx, |list, cx| {
        list.set_selected_index(Some(IndexPath::default()), window, cx);
    });
    if local {
        let read = agents.read(cx).history(&host, &worktree, cx);
        let weak = list.downgrade();
        let load = cx.spawn(async move |cx| {
            let conversations = read.await;
            let _ = weak.update(cx, |list, cx| {
                let now = cx.background_executor().now();
                let reduce_motion = cx.reduce_motion();
                list.delegate_mut()
                    .loaded(conversations, now, reduce_motion);
                cx.notify();
            });
        });
        list.update(cx, |list, _| list.delegate_mut()._load = Some(load));
    }
    let weak = list.downgrade();
    // The dialog closure retains the subscription exactly as long as the list is visible.
    let subscription = cx.observe(&agents, move |agents, cx| {
        let running = agents.read(cx).running(cx);
        let _ = weak.update(cx, |list, cx| {
            list.delegate_mut().running = running;
            cx.notify();
        });
    });
    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            let list = list.clone();
            window.defer(cx, move |window, cx| {
                list.update(cx, |list, cx| list.focus(window, cx))
            });
        }
        let _keep_alive = &subscription;
        dialog
            .title(format!("Conversation history · {place}"))
            .w(switcher::WIDTH)
            .child(
                List::new(&list)
                    .h(switcher::HEIGHT)
                    .search_placeholder("Search conversations…"),
            )
            .child(footer(cx))
    });
}

fn footer(cx: &App) -> impl IntoElement {
    let hint = |key: &'static str, label: &'static str| {
        h_flex()
            .gap_1()
            .children(
                Keystroke::parse(key)
                    .ok()
                    .map(|key| key_hint::render(&key, cx)),
            )
            .child(label)
    };
    h_flex()
        .pt_2()
        .justify_between()
        .gap_3()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(
            div()
                .truncate()
                .child("Claude Code and Codex conversations started in this folder"),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_3()
                .child(hint("enter", "Resume"))
                .child(hint("escape", "Close")),
        )
}

fn title(conversation: &Conversation) -> String {
    conversation
        .title()
        .unwrap_or("Untitled conversation")
        .to_owned()
}

fn agent_label(conversation: &Conversation) -> &'static str {
    match conversation.origin() {
        Origin::ClaudeApp => "Claude app",
        Origin::Cli => conversation.session().agent().name(),
    }
}

/// Every word of the query must occur in the title or the agent's name, ignoring case.
fn matches(query: &str, title: &str, agent: &str) -> bool {
    let haystack = format!("{title} {agent}").to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|term| haystack.contains(term))
}

fn empty_message(load: &Load, query: &str) -> &'static str {
    match (load, query.trim().is_empty()) {
        (Load::Reading, _) => "Reading conversations…",
        (Load::Remote, _) => "Conversation history is available for local projects only.",
        (Load::Ready(_), true) => "No Claude Code or Codex conversations in this folder yet.",
        (Load::Ready(_), false) => "No matching conversations. Try another search.",
    }
}

/// Eased progress of row `row` appearing `elapsed` after the list arrived, or `None` once it
/// rests; rows wait their turn at zero.
fn entrance(elapsed: Duration, row: usize) -> Option<f32> {
    let turn = row.min(STAGGERED_ROWS as usize - 1) as u32;
    let start = ENTRANCE_STAGGER * turn;
    if elapsed >= start + ENTRANCE_DURATION {
        return None;
    }
    let progress = elapsed.saturating_sub(start).as_secs_f32() / ENTRANCE_DURATION.as_secs_f32();
    Some(ease_out_quint()(progress))
}

#[cfg(test)]
mod tests {
    use super::{ENTRANCE_DURATION, ENTRANCE_STAGGER, Load, empty_message, entrance, matches};
    use std::time::Duration;

    #[test]
    fn every_search_term_must_match_title_or_agent() {
        assert!(matches("", "Fix the build", "Codex"));
        assert!(matches("build codex", "Fix the build", "Codex"));
        assert!(matches("claude app", "Release notes", "Claude app"));
        assert!(!matches("build claude", "Fix the build", "Codex"));
    }

    #[test]
    fn search_ignores_case_including_cyrillic() {
        assert!(matches("ЗБІРКУ", "Виправити збірку", "Claude Code"));
        assert!(matches(
            "  виправити   CLAUDE ",
            "Виправити збірку",
            "Claude Code"
        ));
    }

    #[test]
    fn empty_state_names_reading_remote_no_history_and_no_match() {
        assert_eq!(empty_message(&Load::Reading, ""), "Reading conversations…");
        assert_eq!(
            empty_message(&Load::Remote, "x"),
            "Conversation history is available for local projects only."
        );
        assert_eq!(
            empty_message(&Load::Ready(Vec::new()), " "),
            "No Claude Code or Codex conversations in this folder yet."
        );
        assert_eq!(
            empty_message(&Load::Ready(Vec::new()), "missing"),
            "No matching conversations. Try another search."
        );
    }

    #[test]
    fn rows_enter_staggered_and_then_rest() {
        assert_eq!(entrance(Duration::ZERO, 0), Some(0.));
        assert_eq!(entrance(Duration::ZERO, 3), Some(0.));
        let first = entrance(ENTRANCE_STAGGER, 0).unwrap();
        assert!(first > 0. && first < 1.);
        assert_eq!(entrance(ENTRANCE_STAGGER, 1), Some(0.));
        assert_eq!(entrance(ENTRANCE_DURATION, 0), None);
        assert!(entrance(ENTRANCE_DURATION, 1).is_some());
        assert_eq!(entrance(Duration::from_secs(1), 50), None);
    }

    #[test]
    fn late_rows_share_the_last_delay() {
        let at = ENTRANCE_STAGGER * 7 + ENTRANCE_DURATION / 2;
        assert_eq!(entrance(at, 7), entrance(at, 40));
        assert!(entrance(at, 7).is_some());
    }
}
