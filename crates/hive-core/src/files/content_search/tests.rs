use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::atomic::AtomicU64;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-content-search-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        name.into()
    }
    fn search(&self, paths: &[PathBuf], query: &Query) -> (Vec<FileMatches>, Summary) {
        let mut found = Vec::new();
        let summary = search(&self.0, paths, query, &Cancellation::default(), |file| {
            found.push(file)
        })
        .unwrap();
        (found, summary)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::set_permissions(self.0.join("locked"), fs::Permissions::from_mode(0o600));
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn literal(text: &str) -> Query {
    Query {
        text: text.into(),
        ..Query::default()
    }
}

fn lines_of(text: &str, query: &Query) -> Vec<LineMatch> {
    lines::matches(text, &compile(query).unwrap(), MAX_MATCHED_LINES)
}

fn highlighted(line: &LineMatch) -> Vec<&str> {
    line.ranges
        .iter()
        .map(|range| &line.preview[range.clone()])
        .collect()
}

#[test]
fn several_matches_on_one_line_share_one_row() {
    let found = lines_of("a foo b foo\nnone\nfoo", &literal("foo"));
    assert_eq!(found.len(), 2);
    assert_eq!((found[0].line, found[0].column), (0, 2));
    assert_eq!(highlighted(&found[0]), ["foo", "foo"]);
    assert_eq!((found[1].line, found[1].column), (2, 0));
}

#[test]
fn crlf_lines_end_before_the_carriage_return() {
    let query = Query {
        text: "foo$".into(),
        regex: true,
        ..Query::default()
    };
    let found = lines_of("x foo\r\nfoo\r\n", &query);
    assert_eq!(found.iter().map(|l| l.line).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(found[0].preview, "x foo");
}

#[test]
fn column_counts_chars_not_bytes() {
    let found = lines_of("перший\nпривіт світ 世界 foo", &literal("foo"));
    assert_eq!(found[0].line, 1);
    assert_eq!(found[0].column, 15);
    assert_eq!(highlighted(&found[0]), ["foo"]);
}

#[test]
fn match_across_lines_is_cut_at_its_first_line() {
    let query = Query {
        text: r"foo\nbar".into(),
        regex: true,
        ..Query::default()
    };
    let found = lines_of("a foo\nbar b", &query);
    assert_eq!(found.len(), 1);
    assert_eq!(highlighted(&found[0]), ["foo"]);
}

#[test]
fn empty_matches_produce_no_rows() {
    let query = Query {
        text: "^".into(),
        regex: true,
        ..Query::default()
    };
    assert!(lines_of("a\nb\n", &query).is_empty());
}

#[test]
fn indentation_is_trimmed_but_never_past_the_match() {
    let found = lines_of("        let value = foo();", &literal("foo"));
    assert_eq!(found[0].preview, "let value = foo();");
    assert_eq!(highlighted(&found[0]), ["foo"]);
    let found = lines_of("   x", &literal("  x"));
    assert_eq!(found[0].preview, "  x");
    assert_eq!(highlighted(&found[0]), ["  x"]);
}

#[test]
fn long_lines_are_windowed_around_the_first_match() {
    let line = format!("{}needle{}", "é".repeat(200), "x".repeat(500));
    let found = lines_of(&line, &literal("needle"));
    let preview = &found[0].preview;
    assert!(preview.starts_with('…') && preview.ends_with('…'));
    assert!(preview.len() <= 240 + 2 * '…'.len_utf8());
    assert_eq!(highlighted(&found[0]), ["needle"]);
    assert_eq!(found[0].column, 200);
}

#[test]
fn literal_text_escapes_pattern_syntax() {
    let found = lines_of("call(a.b)\ncallXaYb)", &literal("(a.b)"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].line, 0);
}

#[test]
fn case_and_whole_word_options_narrow_matches() {
    let text = "Foo foo foobar";
    assert_eq!(highlighted(&lines_of(text, &literal("foo"))[0]).len(), 3);
    let exact = Query {
        case_sensitive: true,
        ..literal("Foo")
    };
    assert_eq!(highlighted(&lines_of(text, &exact)[0]), ["Foo"]);
    let word = Query {
        whole_word: true,
        ..literal("foo")
    };
    assert_eq!(highlighted(&lines_of(text, &word)[0]), ["Foo", "foo"]);
}

#[test]
fn invalid_pattern_reports_one_line() {
    let query = Query {
        text: "(foo".into(),
        regex: true,
        ..Query::default()
    };
    let Err(Error::InvalidPattern(message)) = compile(&query) else {
        panic!("pattern should not compile");
    };
    assert_eq!(message, "unclosed group");
}

#[test]
fn files_the_editor_cannot_open_are_skipped() {
    let f = Fixture::new();
    let mut paths = vec![
        f.write("text.rs", "needle"),
        f.write("bom.rs", "\u{feff}needle"),
        f.write("binary.bin", b"needle\0"),
        f.write("latin1.txt", b"needle \xff"),
        f.write("large.txt", format!("needle{}", "x".repeat(MAX_FILE_BYTES))),
        f.write("locked", "needle"),
        "missing.rs".into(),
        ".git/config".into(),
        "../outside".into(),
    ];
    fs::set_permissions(f.0.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    symlink(f.0.join("text.rs"), f.0.join("link.rs")).unwrap();
    fs::create_dir(f.0.join("real")).unwrap();
    f.write("real/inner.rs", "needle");
    symlink(f.0.join("real"), f.0.join("linked")).unwrap();
    paths.extend(["link.rs".into(), "linked/inner.rs".into()]);
    paths.sort();
    let (found, summary) = f.search(&paths, &literal("needle"));
    let names: Vec<_> = found.iter().map(|file| file.path.clone()).collect();
    assert_eq!(names, [PathBuf::from("bom.rs"), "text.rs".into()]);
    assert_eq!(found[0].lines[0].column, 0);
    assert!(!summary.truncated);
}

#[test]
fn results_arrive_in_index_order() {
    let f = Fixture::new();
    let paths: Vec<_> = (0..300)
        .map(|ix| f.write(&format!("dir/{ix:04}.txt"), "x\nneedle\n".repeat(ix % 3)))
        .collect();
    let (found, summary) = f.search(&paths, &literal("needle"));
    let expected: Vec<_> = paths
        .iter()
        .enumerate()
        .filter(|(ix, _)| ix % 3 != 0)
        .map(|(_, path)| path.clone())
        .collect();
    assert_eq!(
        found.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
        expected
    );
    assert!(!summary.truncated);
}

#[test]
fn the_line_cap_truncates_results() {
    let f = Fixture::new();
    let paths: Vec<_> = (0..4)
        .map(|ix| f.write(&format!("{ix}.txt"), "needle\n".repeat(2_000)))
        .collect();
    let (found, summary) = f.search(&paths, &literal("needle"));
    let total: usize = found.iter().map(|file| file.lines.len()).sum();
    assert_eq!(total, MAX_MATCHED_LINES);
    assert!(summary.truncated);
    assert_eq!(found[0].path, paths[0]);
}

#[test]
fn one_file_reports_matches_beyond_the_line_cap() {
    let f = Fixture::new();
    let paths = [f.write("a.txt", "needle\n".repeat(MAX_MATCHED_LINES + 1))];
    let (found, summary) = f.search(&paths, &literal("needle"));
    assert_eq!(found[0].lines.len(), MAX_MATCHED_LINES);
    assert!(summary.truncated);
}

#[test]
fn exactly_the_line_cap_is_a_complete_result() {
    let f = Fixture::new();
    let paths = [f.write("a.txt", "needle\n".repeat(MAX_MATCHED_LINES))];
    let (found, summary) = f.search(&paths, &literal("needle"));
    assert_eq!(found[0].lines.len(), MAX_MATCHED_LINES);
    assert!(!summary.truncated);
}

#[test]
fn cancelled_search_emits_nothing() {
    let f = Fixture::new();
    let paths = [f.write("a.txt", "needle")];
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let mut emitted = 0;
    let result = search(&f.0, &paths, &literal("needle"), &cancellation, |_| {
        emitted += 1
    });
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(emitted, 0);
}

#[test]
fn empty_query_returns_without_reading() {
    let mut emitted = 0;
    let summary = search(
        Path::new("/hive-missing-root"),
        &["a".into()],
        &Query::default(),
        &Cancellation::default(),
        |_| emitted += 1,
    )
    .unwrap();
    assert_eq!((emitted, summary.truncated), (0, false));
}

#[test]
fn query_tolerates_missing_fields() {
    let query: Query = serde_json::from_str(r#"{"text":"a","future":1}"#).unwrap();
    assert_eq!(query, literal("a"));
}
