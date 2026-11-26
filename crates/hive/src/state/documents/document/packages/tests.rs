//! Rows that wait, answer, follow edits and survive a failed lookup.
use super::{PackageLens, PackageRow};
use hive_core::package_lens::{Dependency, Error, Lens, Manifest, TextEdit, Versions};

const SOURCE: &str = "{\"dependencies\": {\n  \"a\": \"^1.0.0\",\n  \"b\": \"2.0.0\",\n  \"local\": \"file:../local\"\n}}";

fn entries(text: &str) -> Vec<Dependency> {
    Manifest::parse(text).unwrap().entries
}

fn answer(dependency: &Dependency) -> Lens {
    Lens {
        dependency: dependency.clone(),
        resolved: None,
        versions: Versions::Found(Vec::new()),
        advisories: Vec::new(),
        more_advisories: false,
    }
}

fn lines(lens: &PackageLens) -> Vec<(&str, usize, bool)> {
    lens.rows()
        .iter()
        .map(|row| {
            let d = row.dependency();
            (d.name.as_str(), d.line, matches!(row, PackageRow::Ready(_)))
        })
        .collect()
}

#[test]
fn registry_entries_wait_and_keep_answers_that_still_match() {
    let parsed = entries(SOURCE);
    let mut lens = PackageLens::default();
    lens.expect(&parsed);
    assert_eq!(
        lines(&lens),
        [("a", 1, false), ("b", 2, false)],
        "a local path is never looked up"
    );
    lens.answer(answer(&parsed[1]));
    assert_eq!(lines(&lens), [("a", 1, false), ("b", 2, true)]);
    // The same text parsed again keeps the answer instead of waiting again.
    lens.expect(&entries(SOURCE));
    assert_eq!(lines(&lens), [("a", 1, false), ("b", 2, true)]);
}

#[test]
fn an_edit_moves_answers_below_it_and_drops_the_entry_it_touched() {
    let parsed = entries(SOURCE);
    let mut lens = PackageLens::default();
    lens.expect(&parsed);
    lens.finish(Ok(parsed[..2].iter().map(answer).collect()));
    // A new line typed above both entries.
    let at = SOURCE.find("  \"a\"").unwrap();
    let inserted = "  \"c\": \"1\",\n";
    lens.follow([TextEdit {
        start: at,
        old_end: at,
        new_end: at + inserted.len(),
        old_end_line: 1,
        new_end_line: 2,
    }]);
    assert_eq!(lines(&lens), [("a", 2, true), ("b", 3, true)]);
    let after = format!("{}{inserted}{}", &SOURCE[..at], &SOURCE[at..]);
    for row in lens.rows() {
        let d = row.dependency();
        assert_eq!(after[d.value.clone()], format!("\"{}\"", d.spec));
    }
    // Typing inside the first requirement: only that entry waits for the next lookup.
    let digit = lens.rows()[0].dependency().value.start + 2;
    lens.follow([TextEdit {
        start: digit,
        old_end: digit + 1,
        new_end: digit + 1,
        old_end_line: 2,
        new_end_line: 2,
    }]);
    assert_eq!(lines(&lens), [("b", 3, true)]);
}

#[test]
fn a_failed_lookup_stops_waiting_and_keeps_earlier_answers() {
    let parsed = entries(SOURCE);
    let mut lens = PackageLens::default();
    lens.expect(&parsed);
    lens.answer(answer(&parsed[0]));
    lens.finish(Err(Error::Manifest));
    assert_eq!(lines(&lens), [("a", 1, true)]);
    assert_eq!(lens.status(), Some("Invalid or ambiguous package.json"));
    // A later lookup that succeeds clears the failure.
    lens.finish(Ok(vec![answer(&parsed[1])]));
    assert_eq!(lines(&lens), [("b", 2, true)]);
    assert_eq!(lens.status(), None);
}

#[test]
fn an_answer_for_an_entry_not_yet_expected_lands_in_source_order() {
    let parsed = entries(SOURCE);
    let mut lens = PackageLens::default();
    lens.answer(answer(&parsed[1]));
    lens.answer(answer(&parsed[0]));
    assert_eq!(lines(&lens), [("a", 1, true), ("b", 2, true)]);
}
