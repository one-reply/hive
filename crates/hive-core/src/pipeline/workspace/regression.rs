//! Run one pinned regression with fixed local tool arguments. Baseline reruns never rewrite the candidate.
use super::{snapshot::Scratch, *};
use crate::pipeline::test_output::{cargo_outcome, js_outcome};
use crate::pipeline::{Phase, RegressionStep, RegressionTest, Run, TestRunner};
use serde::Deserialize;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;

const TEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const REPORT_LIMIT: usize = 4 * 1024 * 1024;
/// Extensions of the script configuration files Jest, Vitest and Vite accept.
const SCRIPT_EXTENSIONS: [&str; 6] = ["js", "mjs", "cjs", "ts", "mts", "cts"];

/// Outcome for the exact command phase that scheduled it.
pub enum RegressionResult {
    /// The selected test ran and met the expected base or candidate outcome.
    Matched {
        /// Immutable blob of the test file.
        blob: String,
        /// Fingerprint of the candidate observed during verification.
        revision: String,
        /// Bounded human-readable result.
        detail: String,
    },
    /// The workflow needs a human decision.
    NeedsDecision {
        /// Result or setup failure visible to the user.
        detail: String,
        /// Exact replacement test, when the candidate changed it.
        changed_blob: Option<String>,
    },
}

fn issue(detail: impl Into<String>) -> RegressionResult {
    RegressionResult::NeedsDecision {
        detail: detail.into(),
        changed_blob: None,
    }
}

fn text_path(path: &Path) -> Result<&str, GitError> {
    path.to_str()
        .ok_or_else(|| GitError::Invalid("The local runner needs a UTF-8 path.".into()))
}

/// Execute the independently authored test on one side of the comparison.
pub fn run_regression(run: &Run, stop: &Cancellation) -> Result<RegressionResult, GitError> {
    let r = run
        .regression()
        .ok_or_else(|| GitError::Invalid("No regression test configured.".into()))?;
    let test = r
        .test
        .as_ref()
        .ok_or_else(|| GitError::Invalid("No test was submitted.".into()))?;
    test.validate()
        .map_err(|e| GitError::Invalid(e.to_string()))?;
    let [lane] = run.lanes() else {
        return Err(GitError::Invalid(
            "Select exactly one regression lane.".into(),
        ));
    };
    let baseline = match run.phase() {
        Phase::Regression(RegressionStep::Baseline) => true,
        Phase::Regression(RegressionStep::Candidate) => false,
        _ => {
            return Err(GitError::Invalid(
                "No regression command is pending.".into(),
            ));
        }
    };
    let (revision_before, trees) = reviewed_trees(run.id(), run.lanes(), stop)?;
    let tree = &trees[0];
    let entry = git::run(
        &Host::Local,
        &lane.worktree,
        &["ls-tree", tree, "--", &test.path],
    )?;
    let blob = entry
        .split_once('\t')
        .and_then(|(meta, _)| {
            meta.strip_prefix("100644 blob ")
                .or_else(|| meta.strip_prefix("100755 blob "))
        })
        .filter(|id| is_object_name(id));
    let Some(blob) = blob else {
        return Ok(issue(
            "The dedicated test file is missing, ignored or no longer a regular file.",
        ));
    };
    if r.blob.as_deref().is_some_and(|expected| expected != blob) {
        return Ok(RegressionResult::NeedsDecision {
            detail: format!(
                "{} changed from {} to {blob}. Restore the approved test or approve this version.",
                test.path,
                r.blob.as_deref().unwrap_or_default()
            ),
            changed_blob: Some(blob.into()),
        });
    }
    if baseline && !r.after_development {
        let paths = git::run(
            &Host::Local,
            &lane.worktree,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--name-only",
                "--no-renames",
                &lane.base,
                tree,
                "--",
            ],
        )?;
        if paths != test.path {
            return Ok(issue(
                "The independent author must change only the selected test file relative to the base commit.",
            ));
        }
    }
    let scratch = Scratch::new(run.id())?;
    let base_dir = scratch.0.join("base");
    let cwd = if baseline {
        // The private clone shares immutable Git objects, including the retained test tree.
        git::run_stoppable(
            &Host::Local,
            &scratch.0,
            &[
                "clone",
                "--local",
                "--shared",
                "--no-checkout",
                "--",
                text_path(&lane.project)?,
                "base",
            ],
            stop,
        )?;
        git::run_stoppable(
            &Host::Local,
            &base_dir,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "checkout",
                "--detach",
                &lane.base,
            ],
            stop,
        )?;
        if let Err(e) = provision(lane, &base_dir, stop) {
            return Ok(issue(format!("Baseline setup: {e}")));
        }
        if !git::run(
            &Host::Local,
            &base_dir,
            &["diff", "--name-only", &lane.base, "--"],
        )?
        .is_empty()
        {
            return Ok(issue("Baseline setup modified tracked source files."));
        }
        git::run_stoppable(
            &Host::Local,
            &base_dir,
            &[
                "restore",
                &format!("--source={tree}"),
                "--worktree",
                "--",
                &test.path,
            ],
            stop,
        )?;
        &base_dir
    } else {
        &lane.worktree
    };
    let outcome = execute(test, cwd, &scratch.0, baseline, stop);
    let detail = match outcome {
        Ok(detail) => detail,
        Err(e) => return Ok(issue(e)),
    };
    if revision_before != revision(run.id(), run.lanes(), stop)? {
        return Ok(issue(
            "The candidate changed during verification. Retry on a stable revision.",
        ));
    }
    Ok(RegressionResult::Matched {
        blob: blob.into(),
        revision: revision_before,
        detail,
    })
}

fn execute(
    test: &RegressionTest,
    cwd: &Path,
    scratch: &Path,
    baseline: bool,
    stop: &Cancellation,
) -> Result<String, String> {
    let path = Path::new(&test.path);
    let report = scratch.join("result.json");
    let mut cmd = match test.runner {
        TestRunner::Cargo => {
            let package = path
                .parent()
                .and_then(Path::parent)
                .ok_or("Invalid Cargo test path")?;
            let target = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("Invalid Cargo test target")?;
            let mut cmd = Host::Local.command(
                "cargo",
                &["test", "--color", "never", "--manifest-path"],
                Some(cwd),
            );
            cmd.arg(package.join("Cargo.toml")).args([
                "--test", target, "--", &test.name, "--exact", "--format", "pretty", "--color",
                "never",
            ]);
            cmd
        }
        _ => js_command(test, cwd, &report)?,
    };
    cmd.env("CI", "1").env("NO_COLOR", "1");
    let out = command::run_with_log_tail(cmd, stop, TEST_TIMEOUT, CHECK_OUTPUT)
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let observed = match test.runner {
        TestRunner::Cargo => cargo_outcome(&stdout, &test.name, &test.expected, baseline),
        _ => js_outcome(
            &read_regular_file(&report).map_err(|error| {
                format!(
                    "Cannot read the {:?} report for {}: {error}. {}",
                    test.runner,
                    test.name,
                    String::from_utf8_lossy(&out.stderr)
                        .chars()
                        .take(600)
                        .collect::<String>()
                )
            })?,
            &cwd.join(path),
            test,
            baseline,
        ),
    };
    let code_ok = if baseline {
        out.code.is_some_and(|code| code != 0)
    } else {
        out.code == Some(0)
    };
    if !code_ok || !observed {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail = if stderr.trim().is_empty() {
            stdout.as_ref()
        } else {
            stderr.as_ref()
        };
        let tail = tail
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "{} did not produce the expected {} (exit {:?}).\n{}",
            test.name,
            if baseline {
                "named assertion failure"
            } else {
                "passing result"
            },
            out.code,
            tail.chars().take(1400).collect::<String>()
        ));
    }
    Ok(format!(
        "{} · {} · {}",
        test.path,
        test.name,
        if baseline {
            format!("failed as expected: {}", test.expected)
        } else {
            "passed".into()
        }
    ))
}

/// The runner installed nearest to the test, started where it finds its configuration.
pub(super) fn js_command(
    test: &RegressionTest,
    checkout: &Path,
    report: &Path,
) -> Result<std::process::Command, String> {
    let jest = test.runner == TestRunner::Jest;
    let name = if jest { "jest" } else { "vitest" };
    let file = checkout.join(&test.path);
    let folders = || {
        file.ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(checkout))
    };
    let exe = folders()
        .map(|dir| dir.join("node_modules/.bin").join(name))
        .find(|exe| exe.is_file())
        .ok_or_else(|| {
            format!("The project has no installed {name} executable. Check worktree setup.")
        })?;
    // Neither runner looks above its working directory for configuration, so a test in a
    // nested package starts from the nearest folder that configures the runner.
    let root = folders()
        .find(|dir| configures(dir, test.runner))
        .unwrap_or(checkout);
    let path = file
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .ok_or("Invalid test path")?;
    let mut cmd = Host::Local.command(
        exe.to_str().ok_or("Invalid local runner path")?,
        &[],
        Some(root),
    );
    let exact = format!("^{}$", regex::escape(&test.name));
    if jest {
        cmd.args([
            "--runInBand",
            "--no-watchman",
            "--json",
            "--runTestsByPath",
            path,
            "--testNamePattern",
            &exact,
        ]);
    } else {
        cmd.args(["run", path, "--reporter=json", "--testNamePattern", &exact]);
    }
    cmd.arg("--outputFile").arg(report);
    Ok(cmd)
}

/// The one key of `package.json` that matters here: any `jest` section configures Jest.
#[derive(Deserialize)]
struct Manifest {
    jest: Option<serde::de::IgnoredAny>,
}

/// Whether `dir` holds the configuration `runner` reads from its working directory.
fn configures(dir: &Path, runner: TestRunner) -> bool {
    let has = |stem: &str, extensions: &[&str]| {
        extensions
            .iter()
            .any(|extension| dir.join(format!("{stem}.{extension}")).is_file())
    };
    match runner {
        TestRunner::Cargo => false,
        TestRunner::Jest => {
            has("jest.config", &SCRIPT_EXTENSIONS)
                || has("jest.config", &["json"])
                || read_regular_file(&dir.join("package.json"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
                    .is_some_and(|manifest| manifest.jest.is_some())
        }
        TestRunner::Vitest => {
            has("vitest.config", &SCRIPT_EXTENSIONS) || has("vite.config", &SCRIPT_EXTENSIONS)
        }
    }
}

/// Reads a file the checkout or a test run controls: never through a link, never unbounded.
fn read_regular_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("Not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(REPORT_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > REPORT_LIMIT {
        return Err(std::io::Error::other("File exceeds 4 MiB"));
    }
    Ok(bytes)
}
