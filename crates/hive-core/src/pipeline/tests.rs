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
        severity: Severity::Major,
        description: "Server returns id but client reads task_id".into(),
    }
}
fn note(lane: usize) -> Finding {
    Finding {
        lane,
        severity: Severity::Minor,
        description: "Rename the helper for clarity".into(),
    }
}
fn check(success: bool) -> CommandResult {
    CommandResult {
        lane: 0,
        command: "cargo test".into(),
        success,
        exit_code: Some(i32::from(!success)),
        output: if success {
            "passed"
        } else {
            "expected 3, got 2"
        }
        .into(),
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
fn a_red_check_returns_to_development_before_any_review_and_cannot_be_waived() {
    let mut run = run_with(&["/repo"], 3);
    run.lanes[0].command = "cargo test".into();
    plan(&mut run);
    for cycle in 0..=3 {
        report(&mut run, developed());
        assert_eq!(run.phase, Phase::Commands(0));
        assert!(run.pending().is_empty());
        assert!(run.command_results(vec![]).is_err());
        run.command_results(vec![check(false)]).unwrap();
        assert_eq!(run.cycles(0), (cycle + 1).min(3));
    }
    assert_eq!(run.phase, Phase::Decision(0));
    let reviewed = |run: &Run| {
        run.reports
            .iter()
            .any(|(_, _, report)| matches!(report, Report::Review { .. }))
    };
    assert!(!reviewed(&run), "a red check must not cost a reviewer turn");
    assert!(run.dismiss_finding(0, "Nothing to waive").is_err());
    assert!(run.accept().is_err());
    run.retry_gate("Fix that failing test").unwrap();
    let repair = run.prompt(&run.pending()[0]).unwrap();
    assert!(
        repair.contains("Required checks failed. Fix that failing test"),
        "{repair}"
    );
    assert!(repair.contains("expected 3, got 2"), "{repair}");
    report(&mut run, developed());
    run.command_results(vec![check(true)]).unwrap();
    assert_eq!(run.phase, Phase::Review(0));
    // The fingerprint is taken only now, after the checks stopped writing to the worktree.
    assert_eq!(run.review_revision, None);
    let reviewer = run.prompt(&run.pending()[0]).unwrap();
    assert!(
        reviewer.contains("- Role 0 · cargo test · passed"),
        "{reviewer}"
    );
    report(&mut run, review(vec![]));
    assert_eq!(run.phase, Phase::Review(1));
    assert_eq!(run.cycles(0), 3);
}

#[test]
fn minor_findings_pass_the_review_and_wait_for_the_user_at_acceptance() {
    let mut run = run_with(&["/repo"], 3);
    plan(&mut run);
    report(&mut run, developed());
    report(&mut run, review(vec![note(0)]));
    assert_eq!(run.phase, Phase::Review(1));
    assert_eq!(run.cycles(0), 0);
    assert!(run.findings.is_empty());
    report(&mut run, review(vec![issue(0), note(0)]));
    assert_eq!(run.phase, Phase::Development);
    assert_eq!(run.findings, [issue(0)]);
    let repair = run.prompt(&run.pending()[0]).unwrap();
    assert!(repair.contains("task_id"), "{repair}");
    assert!(!repair.contains("Rename the helper"), "{repair}");
    // A new pass over changed code makes the earlier notes stale.
    report(&mut run, developed());
    assert!(run.notes.is_empty());
    report(&mut run, review(vec![]));
    report(&mut run, review(vec![note(0)]));
    assert_eq!(run.phase, Phase::Acceptance);
    assert_eq!(run.notes(), [(1, note(0))]);
    run.request_changes("Do the rename").unwrap();
    let request = run.prompt(&run.pending()[0]).unwrap();
    assert!(request.contains("Do the rename"), "{request}");
    assert!(
        request.contains("- Security & performance · Role 0: Rename the helper"),
        "{request}"
    );
}

#[test]
fn a_finding_without_or_with_an_unreadable_severity_still_blocks() {
    for severity in ["", r#","severity":"critical""#, r#","severity":7"#] {
        let text = format!(
            r#"{{"kind":"review","summary":"Checked","findings":[{{"lane":0{severity},"description":"Broken"}}]}}"#
        );
        let mut run = run_with(&["/repo"], 3);
        plan(&mut run);
        report(&mut run, developed());
        report(&mut run, serde_json::from_str(&text).unwrap());
        assert_eq!(run.phase, Phase::Development, "{text}");
        assert_eq!(run.findings[0].severity, Severity::Major);
    }
    let minor: Finding =
        serde_json::from_str(r#"{"lane":0,"severity":"minor","description":"Nit"}"#).unwrap();
    assert!(!minor.severity.blocks());
}

#[test]
fn a_repair_brief_reaches_only_the_lane_the_finding_was_routed_to() {
    let mut other = issue(1);
    other.description = "Client drops the error body".into();
    let reviewed = |return_lane: Option<&str>| {
        let mut run = run_with(&["/api", "/web"], 3);
        run.template.reviews[0].return_lane = return_lane.map(Into::into);
        plan(&mut run);
        for ticket in run.pending() {
            run.report(&ticket, developed()).unwrap();
        }
        report(&mut run, review(vec![issue(0), other.clone()]));
        let prompts: Vec<_> = run
            .pending()
            .iter()
            .map(|ticket| run.prompt(ticket).unwrap())
            .collect();
        (run, prompts)
    };

    let (run, prompts) = reviewed(None);
    assert_eq!(prompts.len(), 2);
    assert!(prompts[0].contains("task_id") && !prompts[0].contains("error body"));
    assert!(prompts[1].contains("error body") && !prompts[1].contains("task_id"));
    // The reviewers and the history still see every finding.
    assert!(run.feedback.contains("task_id") && run.feedback.contains("error body"));

    // A review with a fixed return lane sends that lane everything, wherever it was found.
    let (_, prompts) = reviewed(Some("Role 0"));
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains("task_id") && prompts[0].contains("error body"));
}

#[test]
fn reviewer_prompt_bounds_the_changes_and_lists_findings_the_user_accepted() {
    let mut run = run_with(&["/repo"], 0);
    run.lanes[0].base = "4f2c9d1".into();
    plan(&mut run);
    report(&mut run, developed());
    let first = run.prompt(&run.pending()[0]).unwrap();
    assert!(first.contains("Base commit: 4f2c9d1"), "{first}");
    assert!(!first.contains("Do not report them again"), "{first}");
    report(&mut run, review(vec![issue(0)]));
    assert_eq!(run.phase, Phase::Decision(0));
    run.dismiss_finding(0, "Tracked separately").unwrap();
    assert_eq!(run.phase, Phase::Review(1));
    let other = run.prompt(&run.pending()[0]).unwrap();
    assert!(!other.contains("Do not report them again"), "{other}");
    report(&mut run, review(vec![]));
    run.request_changes("One more pass").unwrap();
    report(&mut run, developed());
    let again = run.prompt(&run.pending()[0]).unwrap();
    assert!(
        again.contains("Do not report them again:\n- Role 0: Server returns id"),
        "{again}"
    );
    // The same text is not reopened, even re-rated as a note.
    let mut rerated = note(0);
    rerated.description = issue(0).description;
    report(&mut run, review(vec![issue(0), rerated, note(0)]));
    assert_eq!(run.phase, Phase::Review(1));
    assert_eq!(run.notes(), [(0, note(0))]);
}

#[test]
fn a_run_saved_while_its_checks_followed_the_first_review_still_loads_and_concludes() {
    let mut run = run_with(&["/repo"], 3);
    run.lanes[0].command = "cargo test".into();
    plan(&mut run);
    report(&mut run, developed());
    // 1.0.27 fingerprinted the changes, ran the first review and only then the checks,
    // parking the review's findings in the run meanwhile.
    let saved = |phase: Phase, outstanding: serde_json::Value, findings: serde_json::Value| {
        let mut saved = serde_json::to_value(&run).unwrap();
        let fields = saved.as_object_mut().unwrap();
        fields.remove("notes");
        fields.remove("briefs");
        fields.remove("continuations");
        fields["phase"] = serde_json::to_value(phase).unwrap();
        fields["outstanding"] = outstanding;
        fields["findings"] = findings;
        fields["review_revision"] = "reviewed".into();
        fields["last_attempt"] = 9.into();
        let loaded: Run = serde_json::from_value(saved).unwrap();
        assert!(loaded.valid_snapshot());
        loaded
    };
    let none = || serde_json::json!([]);
    let broken = || serde_json::json!([{"lane": 0, "description": "Broken"}]);

    let mut repaired = saved(Phase::Commands(0), none(), broken());
    repaired.command_results(vec![check(true)]).unwrap();
    assert_eq!(repaired.phase, Phase::Development);
    assert!(
        repaired
            .prompt(&repaired.pending()[0])
            .unwrap()
            .contains("Broken")
    );

    // Its review already passed this content, so green checks conclude it as they used to.
    let mut passed = saved(Phase::Commands(0), none(), none());
    passed.command_results(vec![check(true)]).unwrap();
    assert_eq!(passed.phase, Phase::Review(1));

    // Saved mid-review, it still owes the checks that build would have run afterwards.
    let reviewer = serde_json::json!([{"run": 19, "attempt": 9, "lane": null}]);
    let mut reviewing = saved(Phase::Review(0), reviewer, none());
    report(&mut reviewing, review(vec![]));
    assert_eq!(reviewing.phase, Phase::Commands(0));
    reviewing.command_results(vec![check(true)]).unwrap();
    assert_eq!(reviewing.phase, Phase::Review(1));
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
fn reviewer_prompt_lists_only_lanes_with_assigned_work() {
    let mut run = run_with(&["/api", "/web"], 3);
    let assignments = vec![Assignment {
        lane: 1,
        task: "Update the client".into(),
        criteria: "Tests pass".into(),
    }];
    report(&mut run, Report::Plan { assignments });
    run.approve().unwrap();
    report(&mut run, developed());
    assert!(matches!(run.phase, Phase::Review(_)));
    let prompt = run.prompt(&run.pending()[0]).unwrap();
    assert!(prompt.contains("Lane 1: Role 1"), "{prompt}");
    assert!(!prompt.contains("Lane 0:"), "{prompt}");
    let ticket = run.pending()[0];
    let error = run.report(&ticket, review(vec![issue(0)])).unwrap_err();
    assert!(matches!(error, Error::InvalidFinding(0)), "{error}");
    assert_eq!(run.pending(), [ticket]);
}

#[test]
fn a_report_or_recipe_field_from_a_newer_hive_does_not_drop_the_run() {
    let mut run = run_with(&["/repo"], 3);
    plan(&mut run);
    let mut saved = serde_json::to_value(&run).unwrap();
    saved["reports"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!([run.pending()[0], "development", {"kind": "future"}]));
    saved["timings"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"phase": "future_gate", "start_ms": 1}));
    saved["template"].as_object_mut().unwrap().remove("lanes");
    let loaded: Run = serde_json::from_value(saved).unwrap();
    assert_eq!(loaded.reports, run.reports);
    assert_eq!(loaded.timings, run.timings);
    assert_eq!(loaded.template.lanes, Template::default().lanes);
}

#[test]
fn check_history_keeps_only_the_tail_of_a_long_log() {
    let mut run = run_with(&["/repo"], 3);
    run.lanes[0].command = "cargo test".into();
    plan(&mut run);
    report(&mut run, developed());
    let output = format!("{}expected 3, got 2", "compiling\n".repeat(10_000));
    run.command_results(vec![CommandResult {
        lane: 0,
        command: "cargo test".into(),
        success: false,
        exit_code: Some(1),
        output: output.clone(),
    }])
    .unwrap();
    let entry = run
        .history
        .iter()
        .find(|h| h.contains("cargo test"))
        .unwrap();
    assert!(entry.ends_with("expected 3, got 2"));
    assert!(entry.len() < 4300, "{}", entry.len());
    assert_eq!(run.commands[0].output, output);
}

#[test]
fn repair_handoff_keeps_diagnostic_tail_on_a_character_boundary() {
    let log = format!("{}test failed", "я".repeat(6000));
    let excerpt = super::transitions::check_excerpt(&log);
    assert!(excerpt.ends_with("test failed"));
    assert!(excerpt.len() < 4200);
}

#[test]
fn lane_names_differing_only_by_whitespace_are_duplicates_like_in_the_template() {
    let lane = |name: &str| Lane {
        name: name.into(),
        project: "/repo".into(),
        base: "HEAD".into(),
        ..Default::default()
    };
    let result = Run::new(
        19,
        "/owner".into(),
        "Task".into(),
        Template::default(),
        vec![lane("Web"), lane("Web ")],
    );
    assert!(matches!(result, Err(Error::InvalidInput(_))));
}

#[test]
fn another_round_continues_the_lanes_conversation_and_a_retry_starts_fresh() {
    use crate::agent::AgentKind;
    use crate::agent::launch::ResumeSession;
    let session = |agent, id: &str| ResumeSession::new(agent, id).unwrap();
    let codex = session(AgentKind::Codex, "0195a1f0-0000-7000-8000-000000000001");
    let mut run = run_with(&["/api", "/web"], 3);
    plan(&mut run);
    let first = run.pending();
    // The first round has nothing to continue.
    assert!(run.continuation(&first[0]).is_none());
    run.record_session(first[0], codex.clone());
    // A conversation of another CLI cannot be continued by this lane's agent.
    run.record_session(
        first[1],
        session(
            AgentKind::ClaudeCode,
            "6f1c2d3e-0000-4000-8000-000000000002",
        ),
    );
    for ticket in &first {
        run.report(ticket, developed()).unwrap();
    }
    let mut other = issue(1);
    other.description = "Client drops the error body".into();
    report(&mut run, review(vec![issue(0), other]));
    let repair = run.pending();
    let (continued, message) = run.continuation(&repair[0]).unwrap();
    assert_eq!(continued, &codex);
    assert!(
        message.starts_with("Continue your earlier work"),
        "{message}"
    );
    assert!(
        message.contains("task_id") && !message.contains("error body"),
        "{message}"
    );
    assert!(message.contains(r#"{"kind":"developed""#), "{message}");
    assert!(run.continuation(&repair[1]).is_none());
    assert!(run.prompt(&repair[1]).unwrap().contains("error body"));

    // Whatever stopped the attempt may be the conversation itself.
    let retried = run.restart_attempt(&repair[0]).unwrap();
    assert!(run.continuation(&retried).is_none());
    assert!(run.prompt(&retried).unwrap().contains("task_id"));

    // The user's own request for changes continues it as well, until QA starts again.
    for ticket in run.pending() {
        run.report(&ticket, developed()).unwrap();
    }
    assert!(run.continuations.is_empty());
    report(&mut run, review(vec![]));
    report(&mut run, review(vec![]));
    run.request_changes("Rename the endpoint").unwrap();
    let again = run.pending();
    let (continued, message) = run.continuation(&again[0]).unwrap();
    assert_eq!(continued, &codex);
    assert!(message.contains("Rename the endpoint"), "{message}");
    let loaded: Run = serde_json::from_value(serde_json::to_value(&run).unwrap()).unwrap();
    assert!(loaded.valid_snapshot());
    assert_eq!(loaded.continuation(&again[0]).unwrap().0, &codex);
}

#[test]
fn edit_approval_is_a_lanes_own_choice_and_an_unknown_saved_value_asks() {
    let mut run = run_with(&["/api", "/web"], 3);
    run.lanes[1].edits = EditApproval::AcceptEdits;
    let planner = run.pending()[0];
    assert_eq!(run.edit_approval(&planner), EditApproval::CliDefault);
    plan(&mut run);
    let developers = run.pending();
    assert_eq!(run.edit_approval(&developers[0]), EditApproval::CliDefault);
    assert_eq!(run.edit_approval(&developers[1]), EditApproval::AcceptEdits);

    let mut saved = serde_json::to_value(&run).unwrap();
    assert_eq!(saved["lanes"][1]["edits"], "accept_edits");
    // A lane saved before the setting existed, and one saved by a build with a wider choice.
    saved["lanes"][0].as_object_mut().unwrap().remove("edits");
    saved["lanes"][1]["edits"] = "everything".into();
    let loaded: Run = serde_json::from_value(saved).unwrap();
    assert!(loaded.valid_snapshot());
    for ticket in &developers {
        assert_eq!(loaded.edit_approval(ticket), EditApproval::CliDefault);
    }
}
