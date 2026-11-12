//! The thread a running Codex TUI shows.
//!
//! A Codex that runs its threads in process holds `thread-writer-locks/<thread id>.lock` open.
//! Since 0.16x the TUI is only a client of a shared `codex app-server --managed-daemon`: the
//! daemon holds the locks of every client's threads, so the TUI process holds none. Its thread
//! is then the rollout the daemon wrote for a TUI in the same directory since it started.
pub(super) mod history;

use super::{process, read_bounded_line};
use crate::agent::{AgentKind, launch::ResumeSession};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

const LOCK_DIRECTORY: &str = "thread-writer-locks";
/// Rollouts live in `sessions/<year>/<month>/<day>/`.
const DATE_LEVELS: usize = 3;
/// The first rollout line embeds the base instructions (~20 KB in 0.167); anything larger is
/// not a rollout.
const SESSION_META_LIMIT: u64 = 1024 * 1024;
/// What an interactive CLI writes as `originator`: `codex-tui` since the TUI became an
/// app-server client, `codex_cli_rs` before.
const TUI_ORIGINATORS: [&str; 2] = ["codex-tui", "codex_cli_rs"];

/// Blocking: the thread the Codex job `pids` shows, if it has written one yet.
pub(super) fn discover(pids: &[u32]) -> Option<ResumeSession> {
    let open_files: Vec<PathBuf> = pids
        .iter()
        .flat_map(|&pid| process::open_files(pid))
        .collect();
    if let Some(thread) = locked_thread(&open_files) {
        return Some(thread);
    }
    let started = pids
        .iter()
        .filter_map(|&pid| process::start_time(pid))
        .min()?;
    let directory = pids
        .iter()
        .find_map(|&pid| crate::terminal::working_directory(pid).ok())?;
    latest_rollout(&home()?.join("sessions"), &directory, started)
}

/// Codex honours `CODEX_HOME`; shells started by Hive take it from the login shell.
pub(super) fn home() -> Option<PathBuf> {
    crate::host::environment::login()
        .and_then(|environment| environment.get("CODEX_HOME"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| crate::state::home_dir().map(|home| home.join(".codex")))
}

/// A Codex process also locks the threads of subagents it spawns. Thread ids are UUIDv7,
/// so the smallest is the earliest thread: the conversation the user started or resumed.
fn locked_thread(open_files: &[PathBuf]) -> Option<ResumeSession> {
    open_files
        .iter()
        .filter(|path| {
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == LOCK_DIRECTORY)
        })
        .filter_map(|path| path.file_name()?.to_str()?.strip_suffix(".lock"))
        .filter_map(|id| ResumeSession::new(AgentKind::Codex, id))
        .min_by(|left, right| left.id().cmp(right.id()))
}

/// The daemon appends to a thread's rollout on every event, a resumed thread included, so the
/// most recently written TUI rollout of `directory` since `started` is the thread on screen.
/// Two TUIs in one directory can both claim it; `forget_shared` drops such a claim.
fn latest_rollout(sessions: &Path, directory: &Path, started: SystemTime) -> Option<ResumeSession> {
    let mut written: Vec<(SystemTime, PathBuf)> = rollout_files(sessions)
        .into_iter()
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            (modified >= started).then_some((modified, path))
        })
        .collect();
    written.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    written.iter().find_map(|(_, path)| {
        let line = read_bounded_line(path, SESSION_META_LIMIT)?;
        parse_session_meta(&line, directory)
    })
}

fn rollout_files(sessions: &Path) -> Vec<PathBuf> {
    let mut directories = vec![sessions.to_path_buf()];
    for _ in 0..DATE_LEVELS {
        directories = directories
            .iter()
            .flat_map(|directory| entries(directory))
            .filter(|path| path.is_dir())
            .collect();
    }
    directories
        .iter()
        .flat_map(|directory| entries(directory))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
        })
        .collect()
}

fn entries(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect()
}

#[derive(Deserialize)]
struct RolloutLine {
    #[serde(rename = "type")]
    kind: String,
    payload: SessionMeta,
}

#[derive(Deserialize)]
struct SessionMeta {
    id: String,
    cwd: PathBuf,
    #[serde(default)]
    originator: String,
    /// A user's thread names its client (`cli`, `vscode`); a subagent's is an object.
    #[serde(default)]
    source: serde_json::Value,
    /// Missing before 0.16x; `user` for a conversation the user started.
    thread_source: Option<String>,
}

/// A rollout counts only as a user's TUI thread in `directory`; desktop, editor and subagent
/// threads of the same directory share the daemon and are someone else's.
fn parse_session_meta(line: &[u8], directory: &Path) -> Option<ResumeSession> {
    let RolloutLine {
        kind,
        payload: meta,
    } = serde_json::from_slice(line).ok()?;
    let from_user = meta.source.is_string()
        && meta
            .thread_source
            .as_deref()
            .is_none_or(|source| source == "user");
    let is_tui = TUI_ORIGINATORS.contains(&meta.originator.as_str());
    (kind == "session_meta" && from_user && is_tui && same_directory(&meta.cwd, directory))
        .then(|| ResumeSession::new(AgentKind::Codex, &meta.id))
        .flatten()
}

/// `/tmp` and `/private/tmp` name one directory; the process table reports the resolved one.
fn same_directory(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (left.canonicalize(), right.canonicalize()),
            (Ok(left), Ok(right)) if left == right
        )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn locked_thread_is_the_earliest_locked_thread() {
        let locks = Path::new("/Users/me/.codex/thread-writer-locks");
        let paths = [
            PathBuf::from("/Users/me/.codex/logs_2.sqlite"),
            locks.join(".coordination.lock"),
            locks.join("01a1b1a0-a3d4-7833-ab5a-1e45ec49f814.lock"),
            locks.join("01a1abef-96e9-7011-8064-bea0638f0ccb.lock"),
            PathBuf::from("/tmp/elsewhere/01a1a000-0000-7000-8000-000000000000.lock"),
        ];
        let session = locked_thread(&paths).unwrap();
        assert_eq!(session.agent(), AgentKind::Codex);
        assert_eq!(session.id(), "01a1abef-96e9-7011-8064-bea0638f0ccb");
    }

    #[test]
    fn without_a_thread_lock_there_is_no_locked_thread() {
        assert!(locked_thread(&[]).is_none());
        assert!(
            locked_thread(&[PathBuf::from(
                "/Users/me/.codex/thread-writer-locks/$(touch x).lock"
            )])
            .is_none()
        );
    }

    #[test]
    fn thread_lock_held_by_the_process_is_found_through_open_files() {
        let directory = scratch("lock");
        let path = directory.join(LOCK_DIRECTORY);
        std::fs::create_dir(&path).unwrap();
        let held =
            std::fs::File::create(path.join("01a1abef-96e9-7011-8064-bea0638f0ccb.lock")).unwrap();
        assert_eq!(
            locked_thread(&process::open_files(std::process::id()))
                .as_ref()
                .map(ResumeSession::id),
            Some("01a1abef-96e9-7011-8064-bea0638f0ccb")
        );
        drop(held);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Captured from Codex 0.167.0 (TUI on the managed daemon), trimmed: base instructions
    /// and account ids removed.
    pub(in crate::agent::conversation) const TUI_META: &str = r#"{"timestamp":"2026-11-07T07:25:21.871Z","ordinal":0,"type":"session_meta","payload":{"session_id":"01a1b4e5-cce0-7f72-ac75-8c15a2f6678e","id":"01a1b4e5-cce0-7f72-ac75-8c15a2f6678e","timestamp":"2026-11-07T07:25:21.578Z","cwd":"/Users/me/project","runtime_workspace_roots":["/Users/me/project"],"originator":"codex-tui","cli_version":"0.167.0","source":"vscode","thread_source":"user","model_provider":"openai","history_mode":"paginated","base_instructions":{"text":"You are Codex"},"git":{"branch":"main"}}}"#;
    /// Captured from Codex 0.167.0-alpha.3.2, trimmed: a guardian subagent of a desktop thread.
    pub(in crate::agent::conversation) const SUBAGENT_META: &str = r#"{"timestamp":"2026-11-06T20:44:39.400Z","ordinal":0,"type":"session_meta","payload":{"id":"01a1b29b-3834-7662-abbe-c12712449edb","parent_thread_id":"01a1b26f-f2ad-7b50-aba7-88f01dd48cd0","cwd":"/Users/me/project","originator":"Codex Desktop","source":{"subagent":{"other":"guardian"}},"thread_source":"guardian_review"}}"#;

    #[test]
    fn tui_session_meta_names_the_thread_of_its_directory() {
        let session = parse_session_meta(TUI_META.as_bytes(), Path::new("/Users/me/project"));
        assert_eq!(
            session.as_ref().map(ResumeSession::id),
            Some("01a1b4e5-cce0-7f72-ac75-8c15a2f6678e")
        );
        assert!(parse_session_meta(TUI_META.as_bytes(), Path::new("/Users/me/other")).is_none());
    }

    #[test]
    fn subagent_desktop_and_malformed_rollouts_are_not_a_tui_thread() {
        let project = Path::new("/Users/me/project");
        assert!(parse_session_meta(SUBAGENT_META.as_bytes(), project).is_none());
        let desktop = TUI_META.replace("codex-tui", "Codex Desktop");
        assert!(parse_session_meta(desktop.as_bytes(), project).is_none());
        let tui_subagent = SUBAGENT_META.replace("Codex Desktop", "codex-tui");
        assert!(parse_session_meta(tui_subagent.as_bytes(), project).is_none());
        let other_line = TUI_META.replace("session_meta", "response_item");
        assert!(parse_session_meta(other_line.as_bytes(), project).is_none());
        let unsafe_id = TUI_META.replace("\"id\":\"01a1b4e5", "\"id\":\"--01a1b4e5");
        assert!(parse_session_meta(unsafe_id.as_bytes(), project).is_none());
        assert!(parse_session_meta(&TUI_META.as_bytes()[..200], project).is_none());
        assert!(parse_session_meta(b"", project).is_none());
    }

    #[test]
    fn older_tui_without_thread_source_is_a_user_thread() {
        let older = TUI_META
            .replace("codex-tui", "codex_cli_rs")
            .replace(",\"thread_source\":\"user\"", "");
        assert!(parse_session_meta(older.as_bytes(), Path::new("/Users/me/project")).is_some());
    }

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-codex-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn write_rollout(day: &Path, id: &str, cwd: &Path, modified: SystemTime) {
        let meta = TUI_META
            .replace("01a1b4e5-cce0-7f72-ac75-8c15a2f6678e", id)
            .replace("/Users/me/project", cwd.to_str().unwrap());
        let path = day.join(format!("rollout-2026-11-07T08-25-21-{id}.jsonl"));
        std::fs::write(&path, format!("{meta}\n{{\"type\":\"event_msg\"}}\n")).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(modified).unwrap();
    }

    #[test]
    fn latest_rollout_written_since_start_in_this_directory_is_the_thread() {
        let sessions = scratch("rollouts");
        let project = sessions.join("project");
        let other = sessions.join("other");
        let today = sessions.join("2026/11/07");
        let earlier = sessions.join("2026/10/30");
        for directory in [&project, &other, &today, &earlier] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let started = SystemTime::now() - Duration::from_secs(600);
        let minute = Duration::from_secs(60);
        let older = "01a1a000-0000-7000-8000-000000000001";
        let resumed = "01a1a000-0000-7000-8000-000000000002";
        let before_start = "01a1a000-0000-7000-8000-000000000003";
        let elsewhere = "01a1a000-0000-7000-8000-000000000004";
        write_rollout(&today, older, &project, started + minute);
        // A resumed thread keeps writing to its rollout in the day it was created.
        write_rollout(&earlier, resumed, &project, started + 2 * minute);
        write_rollout(&today, before_start, &project, started - minute);
        write_rollout(&today, elsewhere, &other, started + 3 * minute);
        std::fs::write(today.join("notes.jsonl"), TUI_META).unwrap();

        let found = latest_rollout(&sessions, &project, started);
        assert_eq!(found.as_ref().map(ResumeSession::id), Some(resumed));
        assert!(latest_rollout(&sessions, &project, started + 5 * minute).is_none());
        assert!(latest_rollout(&sessions.join("missing"), &project, started).is_none());
        std::fs::remove_dir_all(sessions).unwrap();
    }

    #[test]
    fn rollout_with_an_unterminated_first_line_is_skipped() {
        let sessions = scratch("truncated");
        let project = sessions.join("project");
        let today = sessions.join("2026/11/07");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&today).unwrap();
        let started = SystemTime::now() - Duration::from_secs(60);
        let id = "01a1a000-0000-7000-8000-000000000005";
        std::fs::write(
            today.join(format!("rollout-2026-11-07T08-25-21-{id}.jsonl")),
            TUI_META.replace("/Users/me/project", project.to_str().unwrap()),
        )
        .unwrap();
        assert!(latest_rollout(&sessions, &project, started).is_none());
        std::fs::remove_dir_all(sessions).unwrap();
    }
}
