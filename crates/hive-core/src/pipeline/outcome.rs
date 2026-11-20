//! Results of a run summarised for people: the required-check tally and readable history.
use super::Run;

/// Required checks of assigned lanes, and how many passed with the configured command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckTally {
    /// Assigned lanes with a mandatory command.
    pub required: usize,
    /// Those whose latest result for that exact command passed.
    pub passed: usize,
}

impl Run {
    /// Before approval the UI previews configured checks; afterwards only assigned work counts.
    pub fn shows_check(&self, lane: &super::Lane) -> bool {
        if matches!(
            self.phase(),
            super::Phase::Planning | super::Phase::ApprovePlan
        ) {
            !lane.command.trim().is_empty()
        } else {
            lane.requires_check()
        }
    }

    /// When the run started, in Unix milliseconds; `None` for a run saved before stage
    /// timings existed.
    pub fn started_ms(&self) -> Option<u64> {
        self.timings().iter().map(|row| row.start_ms).min()
    }

    /// Human age of the start at `now` (Unix milliseconds): "just now", "5m ago", "2d ago",
    /// then the date.
    pub fn age(&self, now: u64) -> Option<String> {
        let at = |ms: u64| chrono::DateTime::from_timestamp_millis(i64::try_from(ms).ok()?);
        let started = at(self.started_ms()?)?;
        Some(crate::git::history::relative_date_at(
            &started.to_rfc3339(),
            at(now)?,
        ))
    }

    /// Counts only results for the command the lane requires now, so an edited or
    /// unrelated command cannot satisfy it.
    pub fn check_tally(&self) -> CheckTally {
        let mut tally = CheckTally::default();
        for (ix, lane) in self.lanes.iter().enumerate() {
            if !lane.requires_check() {
                continue;
            }
            tally.required += 1;
            if self
                .commands
                .iter()
                .any(|c| c.lane == ix && c.command == lane.command && c.success)
            {
                tally.passed += 1;
            }
        }
        tally
    }
}

/// A history entry as people read it. Entries saved by early builds wrote the exit code as a
/// Rust `Option` (`(Some(0))`); the saved text is never rewritten, only shown readably.
pub fn readable_history(entry: &str) -> String {
    if let Some((prefix, rest)) = entry.split_once("(Some(")
        && let Some((code, suffix)) = rest.split_once("))")
        && code.parse::<i32>().is_ok()
    {
        return format!("{prefix}(exit {code}){suffix}");
    }
    entry.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{CommandResult, Lane, Template};

    #[test]
    fn old_structured_exit_codes_are_readable_without_rewriting_raw_test_output() {
        assert_eq!(
            readable_history("API · cargo test · passed (Some(0))"),
            "API · cargo test · passed (exit 0)"
        );
        assert_eq!(readable_history("app: Some(value)"), "app: Some(value)");
    }

    #[test]
    fn a_run_reports_its_age_from_the_first_recorded_stage() {
        let lane = Lane {
            name: "api".into(),
            project: "/api".into(),
            ..Default::default()
        };
        let mut run = Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![lane],
        )
        .unwrap();
        assert_eq!(run.started_ms(), None);
        assert_eq!(run.age(10), None);
        const HOUR: u64 = 3_600_000;
        run.record_stage(None, 5 * HOUR);
        run.record_stage(None, 6 * HOUR);
        assert_eq!(run.started_ms(), Some(5 * HOUR));
        assert_eq!(run.age(5 * HOUR + 30_000).as_deref(), Some("just now"));
        assert_eq!(run.age(8 * HOUR).as_deref(), Some("3h ago"));
        assert_eq!(run.age(53 * HOUR).as_deref(), Some("2d ago"));
    }

    #[test]
    fn check_list_shows_only_assigned_commands_after_approval() {
        use crate::pipeline::{Assignment, Phase, Report};
        let lanes = ["api", "web"].map(|name| Lane {
            name: name.into(),
            project: std::path::Path::new("/").join(name),
            command: "cargo test".into(),
            ..Default::default()
        });
        let mut run = Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            lanes.to_vec(),
        )
        .unwrap();
        let count = |run: &Run| {
            run.lanes()
                .iter()
                .filter(|lane| run.shows_check(lane))
                .count()
        };
        assert_eq!(count(&run), 2);
        let ticket = run.pending()[0];
        run.report(
            &ticket,
            Report::Plan {
                contract: String::new(),
                assignments: vec![Assignment {
                    lane: 1,
                    task: "Implement".into(),
                    criteria: "Pass".into(),
                }],
            },
        )
        .unwrap();
        assert_eq!(count(&run), 2);
        run.approve().unwrap();
        assert_eq!(run.phase(), Phase::Development);
        assert_eq!(count(&run), 1);
        assert!(run.shows_check(&run.lanes()[1]));
        assert_eq!(run.check_tally().required, 1);
    }

    #[test]
    fn check_tally_requires_a_matching_passing_result_for_assigned_work() {
        let mut run = Run::new(
            1,
            "/repo".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "Development".into(),
                project: "/repo".into(),
                command: "cargo test".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let tally = |run: &Run| {
            let t = run.check_tally();
            (t.passed, t.required)
        };
        assert_eq!(tally(&run), (0, 0));
        run.lanes[0].assignment = "Implement the task".into();
        assert_eq!(tally(&run), (0, 1));
        run.commands.push(CommandResult {
            lane: 0,
            command: "different check".into(),
            success: true,
            output: String::new(),
            exit_code: Some(0),
        });
        assert_eq!(tally(&run), (0, 1));
        run.commands[0].command = "cargo test".into();
        assert_eq!(tally(&run), (1, 1));
        run.commands[0].success = false;
        assert_eq!(tally(&run), (0, 1));
    }
}
