//! Test identities observed in completed Rust libtest and Jest-compatible runner reports.
use super::{CommandResult, RegressionTest};
use serde::Deserialize;
use std::path::{Path, PathBuf};

impl CommandResult {
    /// Unambiguous names reported as passing by a completed test harness.
    pub fn passed_tests(&self) -> Vec<String> {
        if !self.success || self.exit_code != Some(0) {
            return vec![];
        }
        // The command log can contain stderr after the JSON written to stdout.
        let mut names: Vec<String> = if let Some(Ok(report)) =
            serde_json::Deserializer::from_str(&self.output)
                .into_iter::<JsReport>()
                .next()
        {
            if !report.success
                || report.was_interrupted
                || report.num_failed_tests != 0
                || report.num_failed_test_suites != 0
                || report.num_runtime_error_test_suites != 0
            {
                return vec![];
            }
            report
                .test_results
                .into_iter()
                .flat_map(|s| s.assertion_results)
                .filter(|a| a.status == "passed" && !a.full_name.trim().is_empty())
                .map(|a| a.full_name)
                .collect()
        } else {
            cargo_passes(&self.output)
        };
        names.sort_unstable();
        names
            .chunk_by(|a, b| a == b)
            .filter(|group| group.len() == 1)
            .map(|group| group[0].clone())
            .collect()
    }
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

    fn passes(output: &str) -> Vec<String> {
        CommandResult {
            lane: 0,
            command: "check".into(),
            success: true,
            exit_code: Some(0),
            output: output.into(),
        }
        .passed_tests()
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
        assert_eq!(passes(passed), ["returns_two"]);
        for unverified in [
            "running 0 tests",
            "test test_a ... ok",
            "running 1 test\ntest test_a ... ok",
            failure,
            &passed.replace(" ... ok", " ... ignored"),
            &format!("{passed}\n{passed}"),
        ] {
            assert!(passes(unverified).is_empty(), "{unverified}");
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
                    passes(&format!("{output}\nrunner diagnostics")).len(),
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
