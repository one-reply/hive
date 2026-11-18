use super::*;

fn run_with(projects: &[&str], limit: u8) -> Run {
    let mut template = Template {
        lanes: (0..projects.len())
            .map(|i| LanePreset {
                name: format!("Role {i}"),
                ..Default::default()
            })
            .collect(),
        ..Template::default()
    };
    for review in &mut template.reviews {
        review.limit = limit;
    }
    let lanes = projects
        .iter()
        .enumerate()
        .map(|(i, p)| Lane {
            name: format!("Role {i}"),
            project: p.into(),
            worktree: p.into(),
            base: "HEAD".into(),
            ..Default::default()
        })
        .collect();
    Run::new(
        19,
        "/owner".into(),
        "Align the API and web".into(),
        template,
        lanes,
    )
    .unwrap()
}

fn report(run: &mut Run, value: Report) {
    let t = run.pending()[0];
    run.report(&t, value).unwrap();
}

fn plan(run: &mut Run) {
    let assignments = (0..run.lanes.len())
        .map(|lane| Assignment {
            lane,
            task: format!("Task {lane}"),
            criteria: "Build and tests pass".into(),
        })
        .collect();
    report(run, Report::Plan { assignments });
    assert_eq!(run.phase, Phase::ApprovePlan);
    run.approve().unwrap();
}

fn developed() -> Report {
    Report::Developed {
        summary: "Updated the contract and tests".into(),
    }
}
fn review(findings: Vec<Finding>) -> Report {
    Report::Review {
        summary: "Checked the API contract".into(),
        findings,
    }
}
fn issue(lane: usize) -> Finding {
    Finding {
        lane,
        description: "Server returns id but client reads task_id".into(),
    }
}

#[test]
fn two_repositories_join_before_review_and_only_affected_lane_repairs() {
    let mut run = run_with(&["/api", "/web"], 3);
    plan(&mut run);
    let tickets = run.pending();
    assert_eq!(tickets.len(), 2);
    run.report(&tickets[1], developed()).unwrap();
    assert_eq!(run.phase, Phase::Development);
    assert!(run.report(&tickets[1], developed()).is_err());
    run.report(&tickets[0], developed()).unwrap();
    assert_eq!(run.phase, Phase::Review(0));
    report(&mut run, review(vec![]));
    report(&mut run, review(vec![issue(1)]));
    assert_eq!(run.pending().len(), 1);
    assert_eq!(run.pending()[0].lane, Some(1));
    assert!(run.prompt(&run.pending()[0]).unwrap().contains("task_id"));
    report(&mut run, developed());
    assert_eq!(run.phase, Phase::Review(0));
    report(&mut run, review(vec![]));
    assert_eq!(run.phase, Phase::Review(1));
    report(&mut run, review(vec![]));
    assert_eq!(run.phase, Phase::Acceptance);
    let mut another_pass = run.clone();
    another_pass.request_changes(" ").unwrap();
    assert_eq!(another_pass.phase, Phase::Development);
    run.accept().unwrap();
    assert_eq!(run.phase, Phase::Accepted);
}

#[test]
fn lanes_sharing_the_actual_worktree_are_serial_even_after_retry_or_restore() {
    let mut run = run_with(&["/a", "/alias", "/b"], 3);
    run.lanes[1].worktree = run.lanes[0].worktree.clone();
    plan(&mut run);
    let pending = run.pending();
    assert_eq!(
        pending.iter().map(|t| t.lane).collect::<Vec<_>>(),
        [Some(0), Some(2)]
    );
    let old = pending[0];
    let replacement = run.restart_attempt(&old).unwrap();
    assert!(run.report(&old, developed()).is_err());
    let mut loaded: Run = serde_json::from_slice(&serde_json::to_vec(&run).unwrap()).unwrap();
    assert!(loaded.valid_snapshot());
    assert_eq!(loaded.pending()[0], replacement);
    loaded.report(&replacement, developed()).unwrap();
    assert_eq!(loaded.pending()[0].lane, Some(1));
    loaded.cancel().unwrap();
    assert!(loaded.pending().is_empty());
    assert!(loaded.report(&pending[1], developed()).is_err());
}

#[test]
fn third_unsuccessful_repair_pauses_and_a_red_command_cannot_be_waived() {
    let mut run = run_with(&["/repo"], 3);
    run.lanes[0].command = "cargo test".into();
    plan(&mut run);
    for cycle in 0..=3 {
        report(&mut run, developed());
        report(&mut run, review(vec![issue(0)]));
        assert_eq!(run.phase, Phase::Commands(0));
        assert!(run.command_results(vec![]).is_err());
        let result = CommandResult {
            lane: 0,
            command: "cargo test".into(),
            success: false,
            exit_code: Some(1),
            output: "expected 3, got 2".into(),
        };
        run.command_results(vec![result]).unwrap();
        assert_eq!(run.cycles(0), (cycle + 1).min(3));
    }
    assert_eq!(run.phase, Phase::Decision(0));
    assert!(run.dismiss_finding(0, " ").is_err());
    run.dismiss_finding(0, "Compatibility tested separately")
        .unwrap();
    assert_eq!(run.phase, Phase::Decision(0));
    assert!(run.accept().is_err());
    run.retry_gate("Fix that failing test").unwrap();
    report(&mut run, developed());
    report(&mut run, review(vec![issue(0)]));
    run.command_results(vec![CommandResult {
        lane: 0,
        command: "cargo test".into(),
        success: true,
        exit_code: Some(0),
        output: "passed".into(),
    }])
    .unwrap();
    assert_eq!(run.phase, Phase::Review(1));
    assert_eq!(run.cycles(0), 3);
}

#[test]
fn wrong_or_duplicate_assignments_leave_the_planner_attempt_available() {
    let mut run = run_with(&["/a"], 3);
    let ticket = run.pending()[0];
    let assignment = Assignment {
        lane: 0,
        task: "task".into(),
        criteria: "criteria".into(),
    };
    for report in [
        developed(),
        Report::Plan {
            assignments: vec![assignment.clone(), assignment.clone()],
        },
        Report::Plan {
            assignments: vec![Assignment {
                lane: 55,
                ..assignment.clone()
            }],
        },
    ] {
        assert!(run.report(&ticket, report).is_err());
        assert_eq!(run.phase, Phase::Planning);
        assert!(run.reports.is_empty());
        assert_eq!(run.pending(), [ticket]);
    }
    run.report(
        &ticket,
        Report::Plan {
            assignments: vec![assignment],
        },
    )
    .unwrap();
    assert!(
        matches!(run.reports.as_slice(), [(id,Phase::Planning,Report::Plan { .. })] if *id == ticket)
    );
    let mut legacy = serde_json::to_value(&run).unwrap();
    legacy.as_object_mut().unwrap().remove("reports");
    let loaded: Run = serde_json::from_value(legacy).unwrap();
    assert!(loaded.reports.is_empty() && loaded.valid_snapshot());
    let mut without_note = run.clone();
    without_note.replan("").unwrap();
    assert_eq!(without_note.phase, Phase::Planning);
    run.replan("Add API compatibility criteria").unwrap();
    assert!(
        run.prompt(&run.pending()[0])
            .unwrap()
            .contains("Add API compatibility criteria")
    );
    assert!(run.report(&ticket, developed()).is_err());
}

#[test]
fn review_budget_survives_an_intervening_green_qa_gate() {
    let mut run = run_with(&["/repo"], 1);
    plan(&mut run);
    for _ in 0..2 {
        report(&mut run, developed());
        report(&mut run, review(vec![]));
        report(&mut run, review(vec![issue(0)]));
    }
    assert_eq!(run.phase, Phase::Decision(1));
    run.dismiss_finding(0, "Accepted compatibility behavior")
        .unwrap();
    assert!(
        run.history
            .last()
            .unwrap()
            .contains("Accepted compatibility behavior")
    );
    run.request_changes("Add docs").unwrap();
    assert_eq!(run.phase, Phase::Development);
    assert!(run.prompt(&run.pending()[0]).unwrap().contains("Add docs"));
}

#[test]
fn repair_handoff_keeps_diagnostic_tail_on_a_character_boundary() {
    let log = format!("{}test failed", "я".repeat(6000));
    let excerpt = super::transitions::check_excerpt(&log);
    assert!(excerpt.ends_with("test failed"));
    assert!(excerpt.len() < 4200);
}
