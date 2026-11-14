//! Antigravity's structured `/usage` command reports the same pools as the signed-in CLI.
//! Remote OAuth summaries can omit Gemini while returning an unused Claude/GPT bucket.

use super::{UsageError, UsageWindow, parse_time};
use crate::{command, host::Host};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(25);
const OUTPUT_LIMIT: usize = 1024 * 1024;
static NEXT_PROBE: AtomicU64 = AtomicU64::new(0);

pub(super) fn fetch() -> Result<Vec<UsageWindow>, UsageError> {
    let executable = command::executable("agy")
        .ok_or_else(|| UsageError::Http("Antigravity CLI unavailable; run `agy`".into()))?;
    let guard = BrowserGuard::new().map_err(|_| {
        UsageError::Http("Could not prepare a noninteractive Antigravity usage check".into())
    })?;
    let mut command = Host::Local.command(
        &executable.to_string_lossy(),
        &[
            "-p",
            "/usage",
            "--output-format",
            "json",
            "--print-timeout",
            "20s",
        ],
        Some(&guard.0),
    );
    guard.configure(&mut command).map_err(|_| {
        UsageError::Http("Could not configure a noninteractive Antigravity usage check".into())
    })?;
    let output = command::run(
        command,
        &[],
        &command::Cancellation::default(),
        TIMEOUT,
        OUTPUT_LIMIT,
    )
    .map_err(|error| UsageError::Http(format!("Antigravity usage check: {error}")))?;
    read_report(output)
}

fn read_report(output: command::Output) -> Result<Vec<UsageWindow>, UsageError> {
    if output.code != Some(0) {
        return Err(UsageError::Http(
            "Antigravity usage check failed; open `agy` and run /usage".into(),
        ));
    }
    let report: Report = serde_json::from_slice(&output.stdout)
        .map_err(|_| UsageError::Json("Invalid Antigravity usage report".into()))?;
    if report.status != "SUCCESS" || report.command.name != "usage" {
        return Err(UsageError::Json(
            "Antigravity usage unavailable; open `agy` and run /usage".into(),
        ));
    }
    let windows = parse(&report.command.data);
    if windows.is_empty() {
        return Err(UsageError::Json(
            "Antigravity returned no usable quota windows".into(),
        ));
    }
    Ok(windows)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Report {
    status: String,
    command: CommandReport,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct CommandReport {
    name: String,
    data: Value,
}

/// A background usage refresh must never launch OAuth or update the CLI installation.
struct BrowserGuard(PathBuf);
impl BrowserGuard {
    fn new() -> std::io::Result<Self> {
        let directory = std::env::temp_dir().join(format!(
            "hive-agy-usage-{}-{}",
            std::process::id(),
            NEXT_PROBE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let guard = Self(directory);
        for name in ["open", "xdg-open"] {
            symlink("/usr/bin/false", guard.0.join(name))?;
        }
        Ok(guard)
    }
    fn configure(
        &self,
        command: &mut std::process::Command,
    ) -> Result<(), std::env::JoinPathsError> {
        let path = crate::host::environment::path();
        let paths =
            std::iter::once(self.0.clone()).chain(path.iter().flat_map(std::env::split_paths));
        command
            .env("PATH", std::env::join_paths(paths)?)
            .env("BROWSER", self.0.join("open"))
            .env("AGY_CLI_DISABLE_AUTO_UPDATE", "true");
        Ok(())
    }
}
impl Drop for BrowserGuard {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            tracing::warn!(%error, "could not clean up Antigravity usage launchers");
        }
    }
}

/// Show the most used enabled pool for each window, including exhausted (zero remaining) pools.
fn parse(json: &Value) -> Vec<UsageWindow> {
    let buckets = json["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group["buckets"].as_array())
        .flatten();
    let mut windows: Vec<UsageWindow> = Vec::new();
    for bucket in buckets {
        if bucket["disabled"].as_bool() == Some(true) {
            continue;
        }
        let Some(window) = bucket_window(bucket) else {
            continue;
        };
        match windows.iter_mut().find(|known| known.label == window.label) {
            Some(known) if known.percent >= window.percent => {}
            Some(known) => *known = window,
            None => windows.push(window),
        }
    }
    windows.sort_by_key(|window| window.label != "5h");
    windows
}

fn bucket_window(bucket: &Value) -> Option<UsageWindow> {
    let label = match bucket["window"].as_str()? {
        "5h" => "5h",
        "weekly" => "7d",
        _ => return None,
    };
    let remaining = bucket["remaining_fraction"]
        .as_f64()
        .filter(|fraction| (0. ..=1.).contains(fraction))?;
    Some(UsageWindow {
        label,
        percent: (1. - remaining) * 100.,
        resets_at: parse_time(&bucket["reset_time"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // agy 1.5.9 /usage, captured 2026-11-14; only quota fields retained.
    const REPORT: &str = r#"{"status":"SUCCESS","num_turns":0,"command":{"name":"usage","data":{"groups":[
        {"name":"Gemini Models","buckets":[{"id":"gemini-weekly","window":"weekly","remaining_fraction":0,"reset_time":"2026-11-21T19:41:31Z"}]},
        {"name":"Claude and GPT models","buckets":[{"id":"3p-weekly","window":"weekly","remaining_fraction":1,"reset_time":"2026-11-21T20:40:36Z"}]}
    ]}}}"#;
    fn output(code: i32, report: &str) -> command::Output {
        command::Output {
            code: Some(code),
            stdout: report.as_bytes().into(),
            stderr: b"private-account-marker".to_vec(),
        }
    }
    #[test]
    fn cli_usage_includes_exhausted_gemini_quota_missing_from_remote_summary() {
        let windows = read_report(output(0, REPORT)).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].remaining_percent(), 0.);
        assert_eq!(
            windows[0].resets_at,
            parse_time(&serde_json::json!("2026-11-21T19:41:31Z"))
        );
    }
    #[test]
    fn failed_empty_and_wrong_command_reports_never_show_full_quota_or_private_output() {
        for report in [
            "",
            "{",
            "null",
            "{}",
            r#"{"status":"FAILED"}"#,
            r#"{"status":"SUCCESS","command":{"name":"other","data":{}}}"#,
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{}}}"#,
        ] {
            let error = read_report(output(0, report)).unwrap_err();
            assert!(!error.to_string().contains("private-account-marker"));
        }
        assert!(read_report(output(1, REPORT)).is_err());
    }
    #[test]
    fn most_used_enabled_pool_wins_without_inventing_missing_windows() {
        let data = serde_json::json!({"groups":[{"buckets":[
            {"window":"5h","remaining_fraction":0.8},
            {"window":"5h","remaining_fraction":0.25},
            {"window":"weekly","remaining_fraction":0.5},
            {"window":"weekly","remaining_fraction":0,"disabled":true},
            {"window":"daily","remaining_fraction":0},
            {"window":"weekly","remaining_fraction":-1},
            {"window":"weekly","remaining_fraction":2},
            {"window":"weekly","remaining_fraction":"0"},
            {"window":"weekly"}
        ]}]});
        let windows = parse(&data);
        assert_eq!(
            windows
                .iter()
                .map(|w| (w.label, w.percent))
                .collect::<Vec<_>>(),
            [("5h", 75.), ("7d", 50.)]
        );
        assert!(parse(&serde_json::json!({"groups":[{"buckets":[{"window":"weekly","disabled":true,"remaining_fraction":1}]}]})).is_empty());
    }
    #[test]
    fn usage_probe_blocks_browser_launchers_and_cleans_them_up() {
        let guard = BrowserGuard::new().unwrap();
        let path = guard.0.clone();
        for name in ["open", "xdg-open"] {
            assert_eq!(
                fs::read_link(path.join(name)).unwrap(),
                PathBuf::from("/usr/bin/false")
            );
        }
        let mut command =
            Host::Local.command("/bin/sh", &["-c", "open https://example.com"], Some(&path));
        guard.configure(&mut command).unwrap();
        let environment: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment[std::ffi::OsStr::new("AGY_CLI_DISABLE_AUTO_UPDATE")],
            Some(std::ffi::OsStr::new("true"))
        );
        let result = command::run(
            command,
            &[],
            &command::Cancellation::default(),
            Duration::from_secs(5),
            1024,
        )
        .unwrap();
        assert_eq!(result.code, Some(1));
        drop(guard);
        assert!(!path.exists());
    }
}
