//! Past conversations started in a directory, read from the CLIs' own stores so a closed tile
//! can be resumed later.
//!
//! Claude Code keeps one transcript per conversation in `<config>/projects/<encoded cwd>/`; the
//! title lines it appends (`custom-title`, `ai-title`, `last-prompt`) sit near the end of the
//! file, so only a bounded tail of each transcript is read. Codex threads come from the same
//! rollouts and acceptance rules as [`super::discover`].
use super::{claude_config_directory, read_tail};
use crate::agent::{AgentKind, launch::ResumeSession};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Newest transcripts read per CLI; older ones are left unread.
pub(super) const SCAN_LIMIT: usize = 500;
/// Conversations returned in all.
const LIST_LIMIT: usize = 200;
/// Every local transcript carried its title lines within its last 32 KiB.
const CLAUDE_TAIL: u64 = 64 * 1024;
const TITLE_LIMIT: usize = 200;

/// Which client started a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The CLI in a terminal.
    Cli,
    /// The Claude desktop app, which shares Claude Code's store.
    ClaudeApp,
}

/// One past conversation that the CLI can resume in its directory.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    session: ResumeSession,
    title: Option<String>,
    origin: Origin,
    updated: SystemTime,
}

impl Conversation {
    /// What to pass to the CLI to continue it.
    pub fn session(&self) -> &ResumeSession {
        &self.session
    }

    /// The name the user or the CLI gave it, else the last prompt; one bounded line.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Which client started it.
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// When its transcript was last written.
    pub fn updated(&self) -> SystemTime {
        self.updated
    }

    /// Human age of the last write: "just now", "5m ago", "2d ago", then the date.
    pub fn age(&self) -> String {
        let updated: chrono::DateTime<chrono::Utc> = self.updated.into();
        crate::git::history::relative_date(&updated.to_rfc3339())
    }
}

/// Blocking: Claude Code and Codex conversations started in `directory`, newest first.
/// A missing or unreadable store contributes nothing.
pub fn list(directory: &Path) -> Vec<Conversation> {
    let claude = claude_config_directory()
        .map(|config| claude_conversations(&config.join("projects"), directory))
        .unwrap_or_default();
    let codex = super::codex::home()
        .map(|home| super::codex::history::conversations(&home, directory))
        .unwrap_or_default();
    merge(claude.into_iter().chain(codex))
}

/// One entry per conversation, the newest write winning, newest first.
fn merge(conversations: impl IntoIterator<Item = Conversation>) -> Vec<Conversation> {
    let mut newest: HashMap<ResumeSession, Conversation> = HashMap::new();
    for conversation in conversations {
        match newest.get(&conversation.session) {
            Some(kept) if kept.updated >= conversation.updated => {}
            _ => {
                newest.insert(conversation.session.clone(), conversation);
            }
        }
    }
    let mut list: Vec<Conversation> = newest.into_values().collect();
    list.sort_by(|left, right| {
        right
            .updated
            .cmp(&left.updated)
            .then_with(|| left.session.id().cmp(right.session.id()))
    });
    list.truncate(LIST_LIMIT);
    list
}

pub(super) fn conversation(
    session: ResumeSession,
    title: Option<&str>,
    origin: Origin,
    updated: SystemTime,
) -> Conversation {
    Conversation {
        session,
        title: title.and_then(one_line),
        origin,
        updated,
    }
}

/// Titles and prompts span lines and can be long; a list row shows one bounded line.
fn one_line(text: &str) -> Option<String> {
    let line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TITLE_LIMIT)
        .collect();
    (!line.is_empty()).then_some(line)
}

/// Claude Code names a project's folder after its absolute path with every character other
/// than an ASCII letter or digit replaced by `-`, one per UTF-16 unit.
fn claude_project_folder(directory: &Path) -> String {
    directory
        .to_string_lossy()
        .chars()
        .flat_map(|character| {
            let replacement = if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            };
            std::iter::repeat_n(replacement, character.len_utf16())
        })
        .collect()
}

fn claude_conversations(projects: &Path, directory: &Path) -> Vec<Conversation> {
    let mut folders = vec![claude_project_folder(directory)];
    if let Ok(resolved) = directory.canonicalize() {
        folders.push(claude_project_folder(&resolved));
    }
    folders.dedup();
    let mut transcripts: Vec<(SystemTime, PathBuf)> = folders
        .iter()
        .flat_map(|folder| transcripts(&projects.join(folder)))
        .collect();
    transcripts.sort_by_key(|(updated, _)| std::cmp::Reverse(*updated));
    transcripts
        .into_iter()
        .take(SCAN_LIMIT)
        .filter_map(|(updated, path)| claude_conversation(&path, updated))
        .collect()
}

/// Top-level `<uuid>.jsonl` files with content; subagent transcripts live in subfolders or,
/// in older versions, as `agent-<id>.jsonl`.
fn transcripts(folder: &Path) -> Vec<(SystemTime, PathBuf)> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %folder.display(), %error, "cannot list Claude conversations");
            }
            return Vec::new();
        }
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?;
            let is_transcript = path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
                && is_uuid(stem);
            let metadata = entry.metadata().ok()?;
            (is_transcript && metadata.is_file() && metadata.len() > 0)
                .then(|| Some((metadata.modified().ok()?, path)))
                .flatten()
        })
        .collect()
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.char_indices().all(|(ix, character)| match ix {
            8 | 13 | 18 | 23 => character == '-',
            _ => character.is_ascii_hexdigit(),
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeLine {
    #[serde(rename = "type", default)]
    kind: String,
    custom_title: Option<String>,
    ai_title: Option<String>,
    last_prompt: Option<String>,
    entrypoint: Option<String>,
}

fn claude_conversation(path: &Path, updated: SystemTime) -> Option<Conversation> {
    let session = ResumeSession::new(AgentKind::ClaudeCode, path.file_stem()?.to_str()?)?;
    let tail = read_tail(path, CLAUDE_TAIL)?;
    let summary = summarize(&tail)?;
    Some(conversation(
        session,
        summary.title.as_deref(),
        summary.origin,
        updated,
    ))
}

struct Summary {
    title: Option<String>,
    origin: Origin,
}

/// `None` when nothing in the tail parses, or when an SDK or another client wrote it: those
/// conversations belong to tools, not to the person.
fn summarize(tail: &[u8]) -> Option<Summary> {
    let mut parsed = false;
    let (mut custom, mut generated, mut prompt, mut entrypoint) = (None, None, None, None);
    for line in tail.split(|&byte| byte == b'\n') {
        let Ok(line) = serde_json::from_slice::<ClaudeLine>(line) else {
            continue;
        };
        parsed = true;
        let non_blank = |text: Option<String>| text.filter(|text| !text.trim().is_empty());
        match line.kind.as_str() {
            "custom-title" => custom = non_blank(line.custom_title).or(custom),
            "ai-title" => generated = non_blank(line.ai_title).or(generated),
            "last-prompt" => prompt = non_blank(line.last_prompt).or(prompt),
            _ => {}
        }
        entrypoint = line.entrypoint.or(entrypoint);
    }
    let origin = match entrypoint.as_deref() {
        None | Some("cli") => Origin::Cli,
        Some("claude-desktop") => Origin::ClaudeApp,
        Some(_) => return None,
    };
    parsed.then(|| Summary {
        title: custom.or(generated).or(prompt),
        origin,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::time::Duration;

    const ID: &str = "88ff5cc9-2be5-40e5-b624-2603e7746d4d";
    /// Shapes captured from Claude Code 2.1.319 on macOS, values replaced.
    const USER_LINE: &str = r#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"Fix the build"},"uuid":"0b5d","timestamp":"2026-11-12T19:58:29.466Z","cwd":"/Users/me/project","sessionId":"88ff5cc9-2be5-40e5-b624-2603e7746d4d","version":"2.1.319","gitBranch":"main","entrypoint":"cli"}"#;
    const CUSTOM_TITLE: &str = r#"{"type":"custom-title","customTitle":"Fix the release build","sessionId":"88ff5cc9-2be5-40e5-b624-2603e7746d4d"}"#;
    const AI_TITLE: &str = r#"{"type":"ai-title","aiTitle":"Release build investigation","sessionId":"88ff5cc9-2be5-40e5-b624-2603e7746d4d"}"#;
    const LAST_PROMPT: &str = r#"{"type":"last-prompt","lastPrompt":"now run the tests\nand commit","leafUuid":"5a1e","sessionId":"88ff5cc9-2be5-40e5-b624-2603e7746d4d"}"#;

    pub(in crate::agent::conversation) fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn summary(lines: &[&str]) -> Option<Summary> {
        summarize(lines.join("\n").as_bytes())
    }

    fn write(path: &Path, lines: &[&str], modified: SystemTime) {
        std::fs::write(path, lines.join("\n") + "\n").unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(modified).unwrap();
    }

    fn id(n: u32) -> String {
        format!("88ff5cc9-2be5-40e5-b624-{n:012}")
    }

    #[test]
    fn project_folder_replaces_every_non_alphanumeric_unit() {
        assert_eq!(
            claude_project_folder(Path::new("/Users/me/projects/hive")),
            "-Users-me-projects-hive"
        );
        assert_eq!(
            claude_project_folder(Path::new("/Users/me/.claude/worktrees/my_app.v2")),
            "-Users-me--claude-worktrees-my-app-v2"
        );
        // Cyrillic letters are one UTF-16 unit; an emoji is two.
        assert_eq!(
            claude_project_folder(Path::new("/tmp/проєкт🐝")),
            "-tmp---------"
        );
    }

    #[test]
    fn custom_title_beats_ai_title_beats_last_prompt_and_the_newest_wins() {
        let newer = CUSTOM_TITLE.replace("Fix the release build", "Ship 1.0");
        let all = summary(&[USER_LINE, CUSTOM_TITLE, AI_TITLE, &newer, LAST_PROMPT]).unwrap();
        assert_eq!(all.title.as_deref(), Some("Ship 1.0"));
        let blank_custom = CUSTOM_TITLE.replace("Fix the release build", "  ");
        let generated = summary(&[USER_LINE, AI_TITLE, &blank_custom, LAST_PROMPT]).unwrap();
        assert_eq!(
            generated.title.as_deref(),
            Some("Release build investigation")
        );
        let prompt = summary(&[USER_LINE, LAST_PROMPT]).unwrap();
        assert_eq!(
            prompt.title.as_deref(),
            Some("now run the tests\nand commit")
        );
    }

    #[test]
    fn title_is_one_line_of_bounded_length() {
        let session = ResumeSession::new(AgentKind::ClaudeCode, ID).unwrap();
        let long = "слово ".repeat(100);
        let made = conversation(session.clone(), Some(&long), Origin::Cli, SystemTime::now());
        let title = made.title().unwrap();
        assert_eq!(title.chars().count(), TITLE_LIMIT);
        assert!(!title.contains('\n'));
        let made = conversation(session, Some("a\n\n  b\tc"), Origin::Cli, SystemTime::now());
        assert_eq!(made.title(), Some("a b c"));
    }

    #[test]
    fn truncated_and_malformed_lines_do_not_hide_the_conversation() {
        let cut = &CUSTOM_TITLE[..30];
        let found = summary(&[cut, "{not json", USER_LINE, LAST_PROMPT, "{\"type\":"]).unwrap();
        assert_eq!(
            found.title.as_deref(),
            Some("now run the tests\nand commit")
        );
        assert!(summary(&["{not json", ""]).is_none());
        assert!(summarize(b"").is_none());
    }

    #[test]
    fn claude_app_conversations_are_listed_with_their_origin() {
        let desktop = USER_LINE.replace("\"cli\"", "\"claude-desktop\"");
        let found = summary(&[&desktop, CUSTOM_TITLE]).unwrap();
        assert_eq!(found.origin, Origin::ClaudeApp);
        let older = USER_LINE.replace(",\"entrypoint\":\"cli\"", "");
        assert_eq!(summary(&[&older]).unwrap().origin, Origin::Cli);
        let sdk = USER_LINE.replace("\"cli\"", "\"sdk-ts\"");
        assert!(summary(&[&sdk, CUSTOM_TITLE]).is_none());
    }

    #[test]
    fn transcript_without_title_lines_is_listed_untitled() {
        let found = summary(&[USER_LINE]).unwrap();
        assert_eq!(found.title, None);
    }

    #[test]
    fn large_transcript_is_read_from_its_tail_only() {
        let projects = scratch("tail");
        let directory = Path::new("/Users/me/project");
        let folder = projects.join(claude_project_folder(directory));
        std::fs::create_dir_all(&folder).unwrap();
        // A title early in the file is past the tail and does not count.
        let early_title = CUSTOM_TITLE.replace("Fix the release build", "Too early");
        let padding = USER_LINE.replace("Fix the build", &"x".repeat(1000));
        let mut lines = vec![early_title.as_str()];
        lines.extend(std::iter::repeat_n(padding.as_str(), 200));
        lines.push(LAST_PROMPT);
        write(
            &folder.join(format!("{ID}.jsonl")),
            &lines,
            SystemTime::now(),
        );
        let found = claude_conversations(&projects, directory);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title(), Some("now run the tests and commit"));
        std::fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn sidechain_sdk_and_foreign_files_are_not_conversations() {
        let projects = scratch("foreign");
        let directory = Path::new("/Users/me/project");
        let folder = projects.join(claude_project_folder(directory));
        std::fs::create_dir_all(folder.join(ID).join("subagents")).unwrap();
        let now = SystemTime::now();
        write(&folder.join(format!("{ID}.jsonl")), &[USER_LINE], now);
        write(&folder.join("agent-3f2a1b.jsonl"), &[USER_LINE], now);
        write(
            &folder.join(ID).join("subagents/agent-1.jsonl"),
            &[USER_LINE],
            now,
        );
        write(&folder.join("notes.txt"), &[USER_LINE], now);
        std::fs::write(folder.join(format!("{}.jsonl", id(1))), "").unwrap();
        let sdk = USER_LINE.replace("\"cli\"", "\"sdk-cli\"");
        write(&folder.join(format!("{}.jsonl", id(2))), &[&sdk], now);
        let found = claude_conversations(&projects, directory);
        let ids: Vec<&str> = found.iter().map(|found| found.session().id()).collect();
        assert_eq!(ids, [ID]);
        std::fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn missing_config_or_project_folder_lists_nothing() {
        let projects = scratch("missing");
        assert!(claude_conversations(&projects, Path::new("/Users/me/project")).is_empty());
        assert!(claude_conversations(&projects.join("absent"), Path::new("/x")).is_empty());
        std::fs::remove_dir_all(projects).unwrap();
    }

    #[test]
    fn conversations_are_newest_first_deduplicated_and_capped() {
        let now = SystemTime::now();
        let made = |n: u32, age: u64| {
            let session = ResumeSession::new(AgentKind::ClaudeCode, &id(n)).unwrap();
            conversation(session, None, Origin::Cli, now - Duration::from_secs(age))
        };
        let list = merge([made(1, 30), made(2, 10), made(1, 5), made(3, 20)]);
        let order: Vec<(String, SystemTime)> = list
            .iter()
            .map(|found| (found.session().id().to_owned(), found.updated()))
            .collect();
        assert_eq!(
            order,
            [
                (id(1), now - Duration::from_secs(5)),
                (id(2), now - Duration::from_secs(10)),
                (id(3), now - Duration::from_secs(20)),
            ]
        );
        let many = merge((0..LIST_LIMIT as u32 + 10).map(|n| made(n, u64::from(n))));
        assert_eq!(many.len(), LIST_LIMIT);
        assert_eq!(many[0].session().id(), id(0));
    }

    #[test]
    fn age_reads_like_the_commit_history() {
        let session = ResumeSession::new(AgentKind::ClaudeCode, ID).unwrap();
        let recent = conversation(session.clone(), None, Origin::Cli, SystemTime::now());
        assert_eq!(recent.age(), "just now");
        let hours = SystemTime::now() - Duration::from_secs(3 * 3600 + 30);
        assert_eq!(
            conversation(session, None, Origin::Cli, hours).age(),
            "3h ago"
        );
    }
}
