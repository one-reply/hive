//! Test identities observed in completed Rust libtest and Jest-compatible runner reports.
use super::RegressionTest;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How much of a command's stdout is read for test names.
pub(super) const REPORT_STDOUT: usize = 8 * 1024 * 1024;
/// Saved names of one command, in bytes: they are written to `state.json` with the run.
const PASSED_NAMES: usize = 256 * 1024;
/// Lines that may start a JSON report, tried from the end of stdout.
const REPORT_CANDIDATES: usize = 8;

/// Unambiguous names a completed harness reported as passing on the `stdout` of a command
/// that exited successfully, sorted.
pub(super) fn passed_tests(stdout: &str) -> Vec<String> {
    let mut names: Vec<String> = match js_report(stdout) {
        Some(report) if !report.passed_cleanly() => return vec![],
        Some(report) => report
            .test_results
            .into_iter()
            .flat_map(|s| s.assertion_results)
            .filter(|a| a.status == "passed" && !a.full_name.trim().is_empty())
            .map(|a| a.full_name)
            .collect(),
        None => cargo_passes(stdout),
    };
    names.sort_unstable();
    let mut size = 0;
    // ponytail: names past the budget are dropped and need other evidence; save name hashes
    // instead if suites outgrow it.
    names
        .chunk_by(|a, b| a == b)
        .filter(|group| group.len() == 1)
        .map(|group| group[0].clone())
        .take_while(|name| {
            size += name.len();
            size <= PASSED_NAMES
        })
        .collect()
}

/// The last Jest-compatible report on stdout. A package manager prints its banner before the
/// report and a runner may log after it, so the report rarely starts the output.
fn js_report(stdout: &str) -> Option<JsReport> {
    stdout
        .rmatch_indices("\n{")
        .map(|(newline, _)| newline + 1)
        .chain(stdout.starts_with('{').then_some(0))
        .take(REPORT_CANDIDATES)
        .find_map(|start| {
            serde_json::Deserializer::from_str(&stdout[start..])
                .into_iter::<JsReport>()
                .next()?
                .ok()
        })
}

fn cargo_passes(output: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut started = false;
    let mut suite = Vec::new();
    for line in output.lines() {
        if let Some(count) = line
            .strip_prefix("running ")
            .and_then(|s| s.split_once(' '))
            && matches!(count.1, "test" | "tests")
            && count.0.parse::<usize>().is_ok()
        {
            started = true;
            suite.clear();
        } else if started {
            if let Some(name) = line
                .strip_prefix("test ")
                .and_then(|s| s.strip_suffix(" ... ok"))
            {
                suite.push(name.to_owned());
            } else if line.starts_with("test result:") {
                if let Some((passed, _)) = line
                    .strip_prefix("test result: ok. ")
                    .and_then(|s| s.split_once(" passed; 0 failed;"))
                    && passed.parse::<usize>() == Ok(suite.len())
                {
                    names.append(&mut suite);
                }
                started = false;
            }
        }
    }
    names
}

pub(super) fn cargo_outcome(output: &str, name: &str, expected: &str, baseline: bool) -> bool {
    let result = format!("test {name} ... {}", if baseline { "FAILED" } else { "ok" });
    if !output.lines().any(|l| l == "running 1 test")
        || output.lines().filter(|l| *l == result).count() != 1
    {
        return false;
    }
    if baseline {
        output
            .split_once(&format!("---- {name} stdout ----"))
            .and_then(|(_, rest)| rest.split_once("\nfailures:"))
            .is_some_and(|(failure, _)| failure.contains(expected))
            && output.contains("test result: FAILED. 0 passed; 1 failed;")
    } else {
        output.contains("test result: ok. 1 passed; 0 failed;")
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsReport {
    success: bool,
    num_failed_tests: usize,
    num_failed_test_suites: usize,
    #[serde(default)]
    num_runtime_error_test_suites: usize,
    #[serde(default)]
    was_interrupted: bool,
    test_results: Vec<JsSuite>,
}

impl JsReport {
    /// Whether the run finished and nothing in it failed.
    fn passed_cleanly(&self) -> bool {
        self.success
            && !self.was_interrupted
            && self.num_failed_tests == 0
            && self.num_failed_test_suites == 0
            && self.num_runtime_error_test_suites == 0
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsSuite {
    name: PathBuf,
    assertion_results: Vec<JsAssertion>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsAssertion {
    full_name: String,
    status: String,
    #[serde(default)]
    failure_messages: Vec<String>,
}

pub(super) fn js_outcome(bytes: &[u8], path: &Path, test: &RegressionTest, baseline: bool) -> bool {
    let Ok(report) = serde_json::from_slice::<JsReport>(bytes) else {
        return false;
    };
    if report.was_interrupted
        || report.num_runtime_error_test_suites != 0
        || report.success == baseline
        || report.num_failed_tests != usize::from(baseline)
        || (report.num_failed_test_suites != 0) != baseline
    {
        return false;
    }
    let mut executed = report
        .test_results
        .iter()
        .flat_map(|s| s.assertion_results.iter().map(move |a| (&s.name, a)))
        .filter(|(_, a)| matches!(a.status.as_str(), "failed" | "passed"));
    let Some((file, a)) = executed.next() else {
        return false;
    };
    executed.next().is_none()
        && (file == path
            || path
                .canonicalize()
                .is_ok_and(|canonical| file == &canonical))
        && a.full_name == test.name
        && if baseline {
            a.status == "failed"
                && a.failure_messages
                    .iter()
                    .any(|m| m.contains(&test.expected))
        } else {
            a.status == "passed"
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::TestRunner;

    /// The captured failing report, as the same run prints it when its only test passes.
    fn passing_report(fixture: &str) -> serde_json::Value {
        let mut report: serde_json::Value = serde_json::from_str(fixture).unwrap();
        report["success"] = true.into();
        report["numFailedTests"] = 0.into();
        report["numFailedTestSuites"] = 0.into();
        report["testResults"][0]["assertionResults"][0]["status"] = "passed".into();
        report
    }

    #[test]
    fn a_js_report_is_read_after_a_package_manager_banner_and_before_later_logs() {
        let report = passing_report(include_str!("fixtures/jest-failed.json"));
        let banner = "\n> web@1.0.0 test\n> jest --json\n\n";
        for body in [
            report.to_string(),
            serde_json::to_string_pretty(&report).unwrap(),
        ] {
            let stdout = format!("{banner}{body}\nDone in 1.2s\n{{\"level\":\"info\"}}\n");
            assert_eq!(passed_tests(&stdout), ["returns_two"], "{stdout}");
        }
        // A failed run proves nothing, whatever else the log looks like.
        let failed = include_str!("fixtures/jest-failed.json");
        let libtest =
            "running 1 test\ntest returns_two ... ok\ntest result: ok. 1 passed; 0 failed;";
        assert!(passed_tests(&format!("{libtest}\n{failed}")).is_empty());
        // Only the last few candidate lines are parsed: a flood of JSON log lines after the
        // report hides it rather than costing one parse per line.
        let flood = "{\"level\":\"info\"}\n".repeat(REPORT_CANDIDATES);
        assert!(passed_tests(&format!("{report}\n{flood}")).is_empty());
    }

    #[test]
    fn saved_names_stay_within_their_budget() {
        let names = ["a", "b", "c"].map(|letter| letter.repeat(PASSED_NAMES / 2 - 1));
        let lines: String = names.iter().map(|n| format!("test {n} ... ok\n")).collect();
        let stdout = format!("running 3 tests\n{lines}test result: ok. 3 passed; 0 failed;");
        assert_eq!(passed_tests(&stdout), names[..2]);
    }

    #[test]
    fn cargo_requires_a_completed_named_harness_and_the_expected_assertion() {
        let failure = include_str!("fixtures/cargo-failed.txt");
        assert!(cargo_outcome(
            failure,
            "returns_two",
            "returns two expected",
            true
        ));
        assert!(!cargo_outcome(
            failure,
            "returns_two",
            "unrelated assertion",
            true
        ));
        assert!(!cargo_outcome(
            failure,
            "returns_three",
            "returns two expected",
            true
        ));
        assert!(!cargo_outcome(
            "error: could not compile",
            "returns_two",
            "error",
            true
        ));
        assert!(!cargo_outcome(
            failure,
            "returns_two",
            "returns two expected",
            false
        ));
        let passed = "running 1 test\ntest returns_two ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;";
        assert!(cargo_outcome(passed, "returns_two", "", false));
        assert_eq!(passed_tests(passed), ["returns_two"]);
        for unverified in [
            "running 0 tests",
            "test test_a ... ok",
            "running 1 test\ntest test_a ... ok",
            failure,
            &passed.replace(" ... ok", " ... ignored"),
            &format!("{passed}\n{passed}"),
        ] {
            assert!(passed_tests(unverified).is_empty(), "{unverified}");
        }
    }

    #[test]
    fn js_reports_require_a_matching_file_test_outcome_and_diagnostic() {
        for (runner, file, failed) in [
            (
                TestRunner::Jest,
                "returns.test.js",
                include_str!("fixtures/jest-failed.json"),
            ),
            (
                TestRunner::Vitest,
                "returns.test.mjs",
                include_str!("fixtures/vitest-failed.json"),
            ),
        ] {
            let test = RegressionTest {
                runner,
                path: file.into(),
                name: "returns_two".into(),
                expected: "returns two expected".into(),
            };
            let file = Path::new("/fixture").join(file);
            assert!(js_outcome(failed.as_bytes(), &file, &test, true));
            assert!(!js_outcome(
                failed.as_bytes(),
                Path::new("/another/file"),
                &test,
                true
            ));
            assert!(!js_outcome(
                failed
                    .replace("returns two expected", "different error")
                    .as_bytes(),
                &file,
                &test,
                true
            ));
            let mut report: serde_json::Value = serde_json::from_str(failed).unwrap();
            report["success"] = true.into();
            report["numFailedTests"] = 0.into();
            report["numFailedTestSuites"] = 0.into();
            for (status, verified) in [("passed", true), ("pending", false), ("skipped", false)] {
                report["testResults"][0]["assertionResults"][0]["status"] = status.into();
                let output = report.to_string();
                assert_eq!(js_outcome(output.as_bytes(), &file, &test, false), verified);
                assert_eq!(
                    passed_tests(&format!("{output}\nrunner diagnostics")).len(),
                    usize::from(verified)
                );
            }
            report["testResults"][0]["assertionResults"] = serde_json::json!([]);
            assert!(!js_outcome(
                report.to_string().as_bytes(),
                &file,
                &test,
                false
            ));
            assert!(!js_outcome(b"{", &file, &test, true));
        }
    }
}
