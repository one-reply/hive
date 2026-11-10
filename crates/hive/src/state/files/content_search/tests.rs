use super::{ContentSearch, ContentStatus, Finished, SearchOption, run};
use hive_core::{
    command::Cancellation,
    files::content_search::{FileMatches, LineMatch, Query},
    host::Host,
    projects::Target,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(Target);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-content-state-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(Target {
            project: path.clone(),
            path,
            host: Host::Local,
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0.path).unwrap();
    }
}

fn file(path: &str, lines: usize) -> FileMatches {
    FileMatches {
        path: path.into(),
        lines: (0..lines)
            .map(|line| LineMatch {
                line: line as u32,
                column: 0,
                preview: "x".into(),
                ranges: Vec::new(),
            })
            .collect(),
    }
}

fn done() -> Result<Finished, String> {
    Ok(Finished {
        truncated: false,
        index_truncated: false,
    })
}

fn paths(search: &ContentSearch) -> Vec<PathBuf> {
    search
        .results
        .iter()
        .map(|file| file.path.clone())
        .collect()
}

#[test]
fn previous_results_stay_until_the_next_run_delivers() {
    let mut search = ContentSearch::default();
    let first = search.begin();
    assert!(search.receive(first, vec![file("old", 2)]));
    assert!(search.finish(first, done()));
    let epoch = search.epoch;

    let second = search.begin();
    assert_eq!(paths(&search), [PathBuf::from("old")]);
    assert_eq!(search.status, ContentStatus::Searching);
    assert!(search.receive(second, vec![file("a", 1), file("b", 3)]));
    assert_eq!(paths(&search), [PathBuf::from("a"), "b".into()]);
    assert_eq!(search.matched_lines, 4);
    assert_eq!(search.epoch, epoch + 1);

    assert!(search.receive(second, vec![file("c", 1)]));
    assert_eq!(search.epoch, epoch + 1, "appending keeps the result set");
    assert_eq!(search.matched_lines, 5);
}

#[test]
fn a_run_without_matches_clears_stale_results() {
    let mut search = ContentSearch::default();
    let first = search.begin();
    search.receive(first, vec![file("old", 1)]);
    let second = search.begin();
    assert!(search.finish(second, done()));
    assert!(search.results.is_empty());
    assert_eq!(search.matched_lines, 0);
    assert_eq!(
        search.status,
        ContentStatus::Done {
            truncated: false,
            index_truncated: false
        }
    );
}

#[test]
fn superseded_runs_are_ignored() {
    let mut search = ContentSearch::default();
    let old = search.begin();
    let current = search.begin();
    assert!(!search.receive(old, vec![file("late", 1)]));
    assert!(!search.finish(old, Err("late failure".into())));
    assert_eq!(search.status, ContentStatus::Searching);
    assert!(search.results.is_empty());
    assert!(search.receive(current, vec![file("now", 1)]));
}

#[test]
fn a_failed_run_drops_results_and_reports_the_error() {
    let mut search = ContentSearch::default();
    let first = search.begin();
    search.receive(first, vec![file("partial", 1)]);
    assert!(search.finish(first, Err("Invalid regular expression: x".into())));
    assert!(search.results.is_empty());
    assert_eq!(
        search.status,
        ContentStatus::Failed("Invalid regular expression: x".into())
    );
}

#[test]
fn clearing_cancels_the_run_and_keeps_options() {
    let mut search = ContentSearch::default();
    search.query.text = "needle".into();
    search.toggle(SearchOption::Regex);
    search.toggle(SearchOption::MatchCase);
    let run = search.begin();
    search.receive(run, vec![file("a", 1)]);
    search.clear();
    assert!(!search.receive(run, vec![file("late", 1)]));
    assert!(search.results.is_empty());
    assert_eq!(search.status, ContentStatus::Idle);
    assert_eq!(search.query.text, "needle");

    search.reset();
    assert!(search.query.text.is_empty());
    assert!(search.query.regex && search.query.case_sensitive && !search.query.whole_word);
}

#[test]
fn run_searches_a_plain_directory_in_path_order() {
    let f = Fixture::new();
    fs::create_dir(f.0.path.join("src")).unwrap();
    fs::write(f.0.path.join("src/b.rs"), "fn needle() {}\n").unwrap();
    fs::write(f.0.path.join("a.rs"), "needle\nneedle\n").unwrap();
    fs::write(f.0.path.join("c.rs"), "nothing\n").unwrap();
    let mut found = Vec::new();
    let query = Query {
        text: "needle".into(),
        ..Query::default()
    };
    let finished = run(&f.0, &query, &Cancellation::default(), |file| {
        found.push(file)
    })
    .unwrap();
    assert!(!finished.truncated && !finished.index_truncated);
    let found: Vec<_> = found
        .iter()
        .map(|file| (file.path.clone(), file.lines.len()))
        .collect();
    assert_eq!(found, [("a.rs".into(), 2), ("src/b.rs".into(), 1)]);
}

#[test]
fn run_reports_an_invalid_pattern() {
    let f = Fixture::new();
    fs::write(f.0.path.join("a.rs"), "x").unwrap();
    let query = Query {
        text: "(".into(),
        regex: true,
        ..Query::default()
    };
    let Err(error) = run(&f.0, &query, &Cancellation::default(), |_| {}) else {
        panic!("an invalid pattern must fail");
    };
    assert!(error.starts_with("Invalid regular expression"), "{error}");
}
