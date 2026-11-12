//! Past Codex TUI threads of a directory, named from Codex's own thread index.
use super::super::history::{Conversation, Origin, SCAN_LIMIT, conversation};
use super::{SESSION_META_LIMIT, parse_session_meta, read_bounded_line, rollout_files};
use serde::Deserialize;
use std::{collections::HashMap, path::Path, time::SystemTime};

/// `session_index.jsonl` gains a line per rename; its recent end names every live thread.
const INDEX_TAIL: u64 = 1024 * 1024;

#[derive(Deserialize)]
struct IndexLine {
    id: String,
    thread_name: Option<String>,
}

/// The newest rollouts under `<home>/sessions` that are the user's TUI threads in `directory`.
pub(in crate::agent::conversation) fn conversations(
    home: &Path,
    directory: &Path,
) -> Vec<Conversation> {
    let mut rollouts: Vec<(SystemTime, std::path::PathBuf)> = rollout_files(&home.join("sessions"))
        .into_iter()
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    rollouts.sort_by_key(|(updated, _)| std::cmp::Reverse(*updated));
    let threads: Vec<_> = rollouts
        .into_iter()
        .take(SCAN_LIMIT)
        .filter_map(|(updated, path)| {
            let line = read_bounded_line(&path, SESSION_META_LIMIT)?;
            Some((parse_session_meta(&line, directory)?, updated))
        })
        .collect();
    if threads.is_empty() {
        return Vec::new();
    }
    let names = thread_names(&home.join("session_index.jsonl"));
    threads
        .into_iter()
        .map(|(session, updated)| {
            let title = names.get(session.id()).map(String::as_str);
            conversation(session, title, Origin::Cli, updated)
        })
        .collect()
}

/// The latest name of each thread; malformed or truncated lines are skipped.
fn thread_names(index: &Path) -> HashMap<String, String> {
    let Some(tail) = super::super::read_tail(index, INDEX_TAIL) else {
        return HashMap::new();
    };
    tail.split(|&byte| byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<IndexLine>(line).ok())
        .filter_map(|line| Some((line.id, line.thread_name?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::history::tests::scratch;
    use super::super::tests::{SUBAGENT_META, TUI_META};
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn rollout(home: &Path, meta: &str, id: &str, cwd: &Path, modified: SystemTime) -> PathBuf {
        let day = home.join("sessions/2026/11/07");
        std::fs::create_dir_all(&day).unwrap();
        let meta = meta
            .replace("01a1b4e5-cce0-7f72-ac75-8c15a2f6678e", id)
            .replace("/Users/me/project", cwd.to_str().unwrap());
        let path = day.join(format!("rollout-2026-11-07T08-25-21-{id}.jsonl"));
        std::fs::write(&path, format!("{meta}\n{{\"type\":\"event_msg\"}}\n")).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(modified).unwrap();
        path
    }

    fn id(n: u32) -> String {
        format!("01a1a000-0000-7000-8000-{n:012}")
    }

    fn index(home: &Path, lines: &[String]) {
        std::fs::write(home.join("session_index.jsonl"), lines.join("\n") + "\n").unwrap();
    }

    fn entry(id: &str, name: &str) -> String {
        format!(r#"{{"id":"{id}","thread_name":"{name}","updated_at":"2026-11-12T19:53:20Z"}}"#)
    }

    fn titles(found: &[Conversation]) -> Vec<(String, Option<String>)> {
        found
            .iter()
            .map(|found| {
                (
                    found.session().id().to_owned(),
                    found.title().map(str::to_owned),
                )
            })
            .collect()
    }

    #[test]
    fn tui_threads_of_the_directory_are_listed_with_their_index_names() {
        let home = scratch("codex-names");
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let now = SystemTime::now();
        rollout(&home, TUI_META, &id(1), &project, now);
        index(&home, &[entry(&id(1), "Fix the build")]);
        let found = conversations(&home, &project);
        assert_eq!(titles(&found), [(id(1), Some("Fix the build".to_owned()))]);
        assert_eq!(found[0].origin(), Origin::Cli);
        assert_eq!(found[0].updated(), now);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn latest_index_entry_names_the_thread() {
        let home = scratch("codex-rename");
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        rollout(&home, TUI_META, &id(1), &project, SystemTime::now());
        index(
            &home,
            &[
                entry(&id(1), "First name"),
                "{broken".to_owned(),
                entry(&id(1), "Renamed"),
            ],
        );
        let found = conversations(&home, &project);
        assert_eq!(titles(&found), [(id(1), Some("Renamed".to_owned()))]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn desktop_subagent_and_other_directory_rollouts_are_not_listed() {
        let home = scratch("codex-foreign");
        let project = home.join("project");
        let other = home.join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let now = SystemTime::now();
        rollout(&home, TUI_META, &id(1), &project, now);
        let desktop = TUI_META.replace("codex-tui", "Codex Desktop");
        rollout(&home, &desktop, &id(2), &project, now);
        rollout(&home, SUBAGENT_META, &id(3), &project, now);
        rollout(&home, TUI_META, &id(4), &other, now);
        let found = conversations(&home, &project);
        assert_eq!(titles(&found), [(id(1), None)]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn thread_without_an_index_entry_is_listed_untitled() {
        let home = scratch("codex-untitled");
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        rollout(&home, TUI_META, &id(1), &project, SystemTime::now());
        index(&home, &[entry(&id(9), "Someone else")]);
        assert_eq!(titles(&conversations(&home, &project)), [(id(1), None)]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_index_and_unterminated_meta_degrade() {
        let home = scratch("codex-degrade");
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let now = SystemTime::now();
        rollout(&home, TUI_META, &id(1), &project, now);
        let cut = rollout(&home, TUI_META, &id(2), &project, now);
        std::fs::write(
            &cut,
            TUI_META.replace("/Users/me/project", project.to_str().unwrap()),
        )
        .unwrap();
        assert_eq!(titles(&conversations(&home, &project)), [(id(1), None)]);
        assert!(conversations(&home.join("absent"), &project).is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn only_the_newest_rollouts_are_scanned() {
        let home = scratch("codex-limit");
        let project = home.join("project");
        let other = home.join("other");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let now = SystemTime::now();
        rollout(
            &home,
            TUI_META,
            &id(0),
            &project,
            now - Duration::from_secs(3600),
        );
        for n in 1..=SCAN_LIMIT as u32 {
            rollout(
                &home,
                TUI_META,
                &id(n),
                &other,
                now - Duration::from_secs(u64::from(n)),
            );
        }
        assert!(conversations(&home, &project).is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
}
