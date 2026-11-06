//! The conversation a running agent process owns, read from the CLI's own local state.
//!
//! Works for agents the user typed into a shell as well as ones Hive launched: Claude Code
//! records `<config>/sessions/<pid>.json` for every interactive process, Codex holds
//! `thread-writer-locks/<thread id>.lock` open while it writes a thread, and Antigravity
//! keeps the latest conversation of each workspace in one cache file.
mod process;

use super::{AgentKind, launch::ResumeSession};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Claude's per-process record is a few hundred bytes; anything larger is not that file.
const CLAUDE_RECORD_LIMIT: u64 = 64 * 1024;
/// One short entry per workspace; anything larger is not that cache.
const ANTIGRAVITY_CACHE_LIMIT: u64 = 1024 * 1024;
const CODEX_LOCK_DIRECTORY: &str = "thread-writer-locks";
/// Documented at <https://antigravity.google/docs/cli/commands/resume>.
const ANTIGRAVITY_CACHE: &str = ".gemini/antigravity-cli/cache/last_conversations.json";

/// Blocking: the conversation one of `pids` (an agent's foreground job) is running, if the
/// CLI has recorded one yet. `None` for agents without a discoverable conversation.
pub fn discover(kind: AgentKind, pids: &[u32]) -> Option<ResumeSession> {
    match kind {
        AgentKind::ClaudeCode => {
            let sessions = claude_config_directory()?.join("sessions");
            pids.iter().find_map(|&pid| claude_session(&sessions, pid))
        }
        AgentKind::Codex => {
            let paths: Vec<PathBuf> = pids
                .iter()
                .flat_map(|&pid| process::open_files(pid))
                .collect();
            codex_thread(&paths)
        }
        AgentKind::Antigravity => {
            let cache = crate::state::home_dir()?.join(ANTIGRAVITY_CACHE);
            let started = pids
                .iter()
                .filter_map(|&pid| process::start_time(pid))
                .max()?;
            let directory = pids
                .iter()
                .find_map(|&pid| crate::terminal::working_directory(pid).ok())?;
            antigravity_conversation(&cache, &directory, started)
        }
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRecord {
    pid: u32,
    session_id: String,
}

/// Claude Code honours `CLAUDE_CONFIG_DIR`; shells started by Hive take it from the login shell.
fn claude_config_directory() -> Option<PathBuf> {
    crate::host::environment::login()
        .and_then(|environment| environment.get("CLAUDE_CONFIG_DIR"))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| crate::state::home_dir().map(|home| home.join(".claude")))
}

fn claude_session(sessions: &Path, pid: u32) -> Option<ResumeSession> {
    let bytes = read_bounded(&sessions.join(format!("{pid}.json")), CLAUDE_RECORD_LIMIT)?;
    parse_claude_record(&bytes, pid)
}

fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// A record for another pid is a stale file left by a process whose pid was reused.
fn parse_claude_record(bytes: &[u8], pid: u32) -> Option<ResumeSession> {
    let record: ClaudeRecord = serde_json::from_slice(bytes).ok()?;
    (record.pid == pid)
        .then(|| ResumeSession::new(AgentKind::ClaudeCode, &record.session_id))
        .flatten()
}

/// A Codex process also locks the threads of subagents it spawns. Thread ids are UUIDv7,
/// so the smallest is the earliest thread: the conversation the user started or resumed.
fn codex_thread(open_files: &[PathBuf]) -> Option<ResumeSession> {
    open_files
        .iter()
        .filter(|path| {
            path.parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == CODEX_LOCK_DIRECTORY)
        })
        .filter_map(|path| path.file_name()?.to_str()?.strip_suffix(".lock"))
        .filter_map(|id| ResumeSession::new(AgentKind::Codex, id))
        .min_by(|left, right| left.id().cmp(right.id()))
}

/// The cache names only the latest conversation of each workspace, not of each process, so
/// an entry counts only when the cache was written after this `agy` started: before its first
/// message the entry still names an older conversation of the same workspace.
fn antigravity_conversation(
    cache: &Path,
    directory: &Path,
    started: SystemTime,
) -> Option<ResumeSession> {
    let written = std::fs::metadata(cache).ok()?.modified().ok()?;
    if written < started {
        return None;
    }
    let bytes = read_bounded(cache, ANTIGRAVITY_CACHE_LIMIT)?;
    parse_antigravity_cache(&bytes, directory)
}

/// Unknown or malformed entries of other workspaces do not hide this one.
fn parse_antigravity_cache(bytes: &[u8], directory: &Path) -> Option<ResumeSession> {
    let entries: HashMap<String, serde_json::Value> = serde_json::from_slice(bytes).ok()?;
    let id = entries.get(directory.to_str()?)?.as_str()?;
    ResumeSession::new(AgentKind::Antigravity, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from Claude Code 2.1.317 on macOS, trimmed.
    const CLAUDE_RECORD: &str = r#"{"pid":10834,"sessionId":"88ff5cc9-2be5-40e5-b624-2603e7746d4d","cwd":"/Users/me/project","startedAt":1793969031376,"procStart":"Fri Nov  6 12:43:50 2026","version":"2.1.317","kind":"interactive","status":"idle"}"#;

    #[test]
    fn claude_record_names_the_conversation_of_its_own_pid() {
        let session = parse_claude_record(CLAUDE_RECORD.as_bytes(), 10834).unwrap();
        assert_eq!(session.agent(), AgentKind::ClaudeCode);
        assert_eq!(session.id(), "88ff5cc9-2be5-40e5-b624-2603e7746d4d");
    }

    #[test]
    fn claude_record_of_a_reused_pid_or_malformed_is_ignored() {
        assert!(parse_claude_record(CLAUDE_RECORD.as_bytes(), 10835).is_none());
        assert!(parse_claude_record(b"{\"pid\":10834", 10834).is_none());
        assert!(parse_claude_record(b"{\"pid\":10834}", 10834).is_none());
        assert!(parse_claude_record(br#"{"pid":10834,"sessionId":"--resume"}"#, 10834).is_none());
    }

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-conversation-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn claude_session_is_read_from_the_pid_file() {
        let directory = scratch("claude");
        std::fs::write(directory.join("10834.json"), CLAUDE_RECORD).unwrap();
        assert_eq!(
            claude_session(&directory, 10834).unwrap().id(),
            "88ff5cc9-2be5-40e5-b624-2603e7746d4d"
        );
        assert!(claude_session(&directory, 1).is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_thread_is_the_earliest_locked_thread() {
        let locks = Path::new("/Users/me/.codex/thread-writer-locks");
        let paths = [
            PathBuf::from("/Users/me/.codex/logs_2.sqlite"),
            locks.join(".coordination.lock"),
            locks.join("01a1b1a0-a3d4-7833-ab5a-1e45ec49f814.lock"),
            locks.join("01a1abef-96e9-7011-8064-bea0638f0ccb.lock"),
            PathBuf::from("/tmp/elsewhere/01a1a000-0000-7000-8000-000000000000.lock"),
        ];
        let session = codex_thread(&paths).unwrap();
        assert_eq!(session.agent(), AgentKind::Codex);
        assert_eq!(session.id(), "01a1abef-96e9-7011-8064-bea0638f0ccb");
    }

    #[test]
    fn codex_without_a_thread_lock_has_no_conversation() {
        assert!(codex_thread(&[]).is_none());
        assert!(
            codex_thread(&[PathBuf::from(
                "/Users/me/.codex/thread-writer-locks/$(touch x).lock"
            )])
            .is_none()
        );
    }

    /// The example from the Antigravity CLI documentation, plus an unrelated malformed entry.
    const ANTIGRAVITY_CACHE_SAMPLE: &str = r#"{
    "/usr/local/google/home/username/Develop/my-project": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "/usr/local/google/home/username/Develop/another-repo": "f9e8d7c6-b5a4-3210-fedc-ba9876543210",
    "/broken": 42
}"#;

    #[test]
    fn antigravity_cache_names_the_workspace_conversation() {
        let session = parse_antigravity_cache(
            ANTIGRAVITY_CACHE_SAMPLE.as_bytes(),
            Path::new("/usr/local/google/home/username/Develop/my-project"),
        )
        .unwrap();
        assert_eq!(session.agent(), AgentKind::Antigravity);
        assert_eq!(session.id(), "a1b2c3d4-e5f6-7890-abcd-ef1234567890");
    }

    #[test]
    fn antigravity_cache_without_this_workspace_or_malformed_is_ignored() {
        let sample = ANTIGRAVITY_CACHE_SAMPLE.as_bytes();
        assert!(parse_antigravity_cache(sample, Path::new("/elsewhere")).is_none());
        assert!(parse_antigravity_cache(sample, Path::new("/broken")).is_none());
        assert!(parse_antigravity_cache(b"[1,2]", Path::new("/broken")).is_none());
        assert!(parse_antigravity_cache(br#"{"/p":"--continue"}"#, Path::new("/p")).is_none());
    }

    #[test]
    fn antigravity_cache_older_than_the_process_is_not_its_conversation() {
        let directory = scratch("antigravity");
        let cache = directory.join("last_conversations.json");
        std::fs::write(&cache, br#"{"/p":"a1b2c3d4-e5f6-7890-abcd-ef1234567890"}"#).unwrap();
        let written = std::fs::metadata(&cache).unwrap().modified().unwrap();
        let second = std::time::Duration::from_secs(1);
        assert_eq!(
            antigravity_conversation(&cache, Path::new("/p"), written - second)
                .as_ref()
                .map(ResumeSession::id),
            Some("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        );
        assert!(antigravity_conversation(&cache, Path::new("/p"), written + second).is_none());
        assert!(
            antigravity_conversation(&directory.join("missing"), Path::new("/p"), written)
                .is_none()
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn codex_conversation_is_found_through_open_files() {
        let directory = scratch("codex");
        let path = directory.join(CODEX_LOCK_DIRECTORY);
        std::fs::create_dir(&path).unwrap();
        let held =
            std::fs::File::create(path.join("01a1abef-96e9-7011-8064-bea0638f0ccb.lock")).unwrap();
        assert_eq!(
            discover(AgentKind::Codex, &[std::process::id()])
                .as_ref()
                .map(ResumeSession::id),
            Some("01a1abef-96e9-7011-8064-bea0638f0ccb")
        );
        drop(held);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
