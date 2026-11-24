//! Reviews that run together: one reviewer each, one repair round for all their findings.
use super::*;

/// Code review on its own, then QA and Security & performance together.
fn grouped() -> Template {
    let mut recipe = Template::default();
    recipe.join_review(2, 1, true).unwrap();
    recipe
}

/// A run of `grouped` over `projects` with development finished and code review passed:
/// the group of QA (1) and Security & performance (2) is at work.
fn in_group(projects: &[&str], limit: u8) -> Run {
    let mut run = run_with_recipe(projects, limit, grouped());
    plan(&mut run);
    for ticket in run.pending() {
        run.report(&ticket, developed()).unwrap();
    }
    assert_eq!(run.phase, Phase::Review(0));
    report(&mut run, review(vec![]));
    assert_eq!(run.phase, Phase::Review(1));
    run
}

/// The pending reviewer of `review`.
fn reviewer_of(run: &Run, review: usize) -> Ticket {
    let pending = run.pending();
    *pending
        .iter()
        .find(|ticket| ticket.review == Some(review))
        .unwrap_or_else(|| panic!("no reviewer of {review} among {pending:?}"))
}

fn report_review(run: &mut Run, review_index: usize, findings: Vec<Finding>) {
    let ticket = reviewer_of(run, review_index);
    run.report(&ticket, review(findings)).unwrap();
}

fn leak(lane: usize) -> Finding {
    Finding {
        lane,
        severity: Severity::Blocker,
        description: "The session token is written to the log".into(),
    }
}

#[test]
fn reviews_of_a_group_get_a_reviewer_each_and_the_run_waits_for_all() {
    let mut run = in_group(&["/api"], 3);
    let reviewers: Vec<_> = run.pending().iter().map(|ticket| ticket.review).collect();
    assert_eq!(reviewers, [Some(1), Some(2)]);
    let security = reviewer_of(&run, 2);
    assert_eq!(run.step(&security).unwrap().name, "Security & performance");
    assert_eq!(run.ticket_phase(&security), Phase::Review(2));
    assert!(Stage::Review(1).is_active(&run) && Stage::Review(2).is_active(&run));
    assert!(!Stage::Review(0).is_active(&run));

    report_review(&mut run, 2, vec![]);
    assert_eq!(run.phase, Phase::Review(1), "QA is still at work");
    assert!(Stage::Review(1).is_active(&run) && !Stage::Review(2).is_active(&run));
    assert!(matches!(
        run.report(&security, review(vec![])),
        Err(Error::InactiveAttempt)
    ));
    report_review(&mut run, 1, vec![]);
    assert_eq!(run.phase, Phase::Acceptance);
    // Each report stands under its own review.
    for stage in [Stage::Review(0), Stage::Review(1), Stage::Review(2)] {
        assert_eq!(run.stage_reports(stage).count(), 1, "{stage:?}");
    }
}

#[test]
fn findings_of_a_group_go_back_in_one_round_each_by_its_own_reviews_route() {
    let mut run = in_group(&["/api", "/web"], 3);
    // Security sends everything to the second lane; QA sends a finding to the lane it names.
    run.template.reviews[2].return_lane = Some("Role 1".into());
    report_review(&mut run, 1, vec![issue(0)]);
    assert_eq!(run.phase, Phase::Review(1));
    report_review(&mut run, 2, vec![leak(0)]);

    assert_eq!(run.phase, Phase::Development);
    assert_eq!(run.rounds, [0, 1, 1]);
    assert_eq!(run.reviewer(&issue(0)), Some(1));
    assert_eq!(run.reviewer(&leak(0)), Some(2));
    let briefs = run.briefs.clone();
    assert_eq!(briefs.len(), 2);
    let (qa, security) = (&briefs[0], &briefs[1]);
    assert_eq!((qa.0, security.0), (0, 1));
    assert!(qa.1.starts_with("QA and Security & performance requested fixes."));
    assert!(qa.1.contains(&issue(0).description) && !qa.1.contains(&leak(0).description));
    assert!(security.1.contains(&leak(0).description) && !security.1.contains("task_id"));

    // After the one repair the pass starts over, from the first review.
    for ticket in run.pending() {
        run.report(&ticket, developed()).unwrap();
    }
    assert_eq!(run.phase, Phase::Review(0));
}

#[test]
fn only_the_reviews_that_asked_for_fixes_spend_a_round() {
    let mut run = in_group(&["/api"], 3);
    report_review(&mut run, 1, vec![]);
    report_review(&mut run, 2, vec![leak(0)]);
    assert_eq!(run.phase, Phase::Development);
    assert_eq!(run.rounds, [0, 0, 1]);
    assert!(
        run.feedback
            .starts_with("Security & performance requested fixes.")
    );
}

#[test]
fn a_group_stops_for_a_decision_when_one_review_is_out_of_rounds_and_charges_nobody() {
    let mut run = in_group(&["/api"], 3);
    run.template.reviews[2].limit = 0;
    report_review(&mut run, 2, vec![leak(0)]);
    report_review(&mut run, 1, vec![issue(0)]);
    assert_eq!(run.phase, Phase::Decision(2));
    assert_eq!(run.rounds, [0, 0, 0]);
    assert_eq!(
        run.history.last().map(String::as_str),
        Some("Security & performance needs a decision")
    );
    assert_eq!(run.findings.len(), 2);

    // Each dismissed finding is remembered for the review that raised it.
    let qa_finding = run.findings.iter().position(|f| *f == issue(0)).unwrap();
    run.dismiss_finding(qa_finding, "Tracked separately")
        .unwrap();
    assert_eq!(run.phase, Phase::Decision(2));
    run.dismiss_finding(0, "Accepted risk").unwrap();
    assert_eq!(run.dismissed, [(1, issue(0)), (2, leak(0))]);
    assert_eq!(run.phase, Phase::Acceptance);
}

#[test]
fn another_round_after_a_decision_repairs_every_open_finding_without_a_charge() {
    let mut run = in_group(&["/api"], 3);
    run.template.reviews[1].limit = 0;
    report_review(&mut run, 1, vec![issue(0)]);
    report_review(&mut run, 2, vec![leak(0)]);
    assert_eq!(run.phase, Phase::Decision(1));
    run.retry_gate("One more try").unwrap();
    assert_eq!(run.phase, Phase::Development);
    assert_eq!(run.rounds, [0, 0, 0]);
    let brief = &run.briefs[0].1;
    assert!(brief.contains(&issue(0).description) && brief.contains(&leak(0).description));
}

#[test]
fn a_finding_the_user_accepted_is_not_raised_again_by_the_review_that_reported_it() {
    let mut run = in_group(&["/api"], 3);
    run.template.reviews[2].limit = 0;
    report_review(&mut run, 1, vec![]);
    report_review(&mut run, 2, vec![leak(0)]);
    run.dismiss_finding(0, "Accepted risk").unwrap();
    assert_eq!(run.phase, Phase::Acceptance);
    // A new pass: the same words from QA are a finding, from Security they are not.
    run.recheck().unwrap();
    report(&mut run, review(vec![]));
    report_review(&mut run, 2, vec![leak(0)]);
    assert!(run.findings.is_empty());
    report_review(&mut run, 1, vec![leak(0)]);
    assert_eq!(run.findings, [leak(0)]);
    assert_eq!(run.reviewer(&leak(0)), Some(1));
}

#[test]
fn checking_one_criterion_again_runs_qa_alone_though_it_shares_a_group() {
    let mut run = in_group(&["/api"], 3);
    report_review(&mut run, 1, vec![]);
    report_review(&mut run, 2, vec![]);
    assert_eq!(run.phase, Phase::Acceptance);
    run.verify_criterion(0, "Build and tests pass", "revision")
        .unwrap_err();
    let mut run = in_group(&["/api"], 3);
    run.review_revision = Some("revision".into());
    report_review(&mut run, 1, vec![]);
    report_review(&mut run, 2, vec![]);
    run.verify_criterion(0, "Build and tests pass", "revision")
        .unwrap();
    assert_eq!(run.phase, Phase::Review(1));
    let pending = run.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].review, Some(1));
    assert!(run.valid_snapshot());
    report_review(&mut run, 1, vec![]);
    assert_eq!(run.phase, Phase::Acceptance);
}

#[test]
fn a_retried_reviewer_keeps_its_review_and_its_old_ticket_is_inactive() {
    let mut run = in_group(&["/api"], 3);
    let old = reviewer_of(&run, 2);
    let retried = run.restart_attempt(&old).unwrap();
    assert_eq!(retried.review, Some(2));
    assert_ne!(retried.attempt, old.attempt);
    assert!(matches!(
        run.report(&old, review(vec![])),
        Err(Error::InactiveAttempt)
    ));
    assert_eq!(reviewer_of(&run, 1).review, Some(1), "QA was not restarted");
    run.report(&retried, review(vec![])).unwrap();
    assert_eq!(run.phase, Phase::Review(1));
    assert!(run.valid_snapshot());
}

#[test]
fn a_review_saved_before_tickets_named_their_review_still_reports() {
    // One review at a time, as every run saved before groups: the phase names the review.
    let mut run = run_with_recipe(&["/api"], 3, Template::default());
    plan(&mut run);
    report(&mut run, developed());
    let mut saved = serde_json::to_value(&run).unwrap();
    assert_eq!(saved["outstanding"][0]["review"], 0);
    saved["outstanding"][0]
        .as_object_mut()
        .unwrap()
        .remove("review");
    let mut loaded: Run = serde_json::from_value(saved).unwrap();
    assert!(loaded.valid_snapshot());
    let ticket = loaded.pending()[0];
    assert_eq!(ticket.review, None);
    assert_eq!(loaded.step(&ticket).unwrap().name, "Code review");
    loaded.report(&ticket, review(vec![issue(0)])).unwrap();
    assert_eq!(loaded.phase, Phase::Development);
    assert_eq!(loaded.rounds, [1, 0, 0]);
    assert_eq!(loaded.reviewer(&issue(0)), Some(0));
}

#[test]
fn a_run_saved_in_the_middle_of_a_group_is_read_back_and_a_stray_reviewer_is_not() {
    let mut run = in_group(&["/api"], 3);
    report_review(&mut run, 2, vec![leak(0)]);
    let saved = serde_json::to_value(&run).unwrap();
    let loaded: Run = serde_json::from_value(saved.clone()).unwrap();
    assert!(loaded.valid_snapshot());
    assert_eq!(loaded, run);

    // A reviewer of a review outside the group that runs, two reviewers of one review, and
    // a reviewer left behind in development are all states no transition produces.
    let corrupted = |change: &dyn Fn(&mut serde_json::Value)| {
        let mut saved = saved.clone();
        change(&mut saved);
        serde_json::from_value::<Run>(saved).unwrap()
    };
    assert!(!corrupted(&|run| run["outstanding"][0]["review"] = 0.into()).valid_snapshot());
    let twice = corrupted(&|run| {
        let mut second = run["outstanding"][0].clone();
        second["attempt"] = run["last_attempt"].clone();
        run["outstanding"].as_array_mut().unwrap().push(second);
    });
    assert!(!twice.valid_snapshot());
    let stray = corrupted(&|run| run["phase"] = serde_json::json!("planning"));
    assert!(!stray.valid_snapshot());
}

#[test]
fn a_conditional_review_of_a_group_is_left_out_and_the_rest_of_the_group_runs() {
    let mut recipe = Template::default();
    recipe.reviews[2].only_paths = vec!["crates/auth/".into()];
    // All three together, so the group is the first thing the pass reviews.
    recipe.join_review(1, 0, true).unwrap();
    recipe.join_review(2, 1, true).unwrap();
    let mut run = run_with_recipe(&["/api"], 3, recipe);
    plan(&mut run);
    report(&mut run, developed());
    assert_eq!(run.pending().len(), 3);
    let changed = vec!["README.md".to_owned()];
    run.record_review_trees("pass".into(), vec!["a".repeat(40)], Some(&changed))
        .unwrap();
    assert_eq!(run.skipped_reviews(), [2]);
    assert_eq!(run.phase, Phase::Review(0));
    let reviewers: Vec<_> = run.pending().iter().map(|ticket| ticket.review).collect();
    assert_eq!(reviewers, [Some(0), Some(1)]);
    report_review(&mut run, 0, vec![]);
    report_review(&mut run, 1, vec![]);
    assert_eq!(run.phase, Phase::Acceptance);
}

#[test]
fn a_group_left_out_whole_is_passed_over() {
    let mut recipe = Template::default();
    for review in [0, 2] {
        recipe.reviews[review].only_paths = vec!["*.sql".into()];
    }
    // Code review and Security & performance together, then QA.
    recipe.join_review(2, 0, true).unwrap();
    let mut run = run_with_recipe(&["/api"], 3, recipe);
    plan(&mut run);
    report(&mut run, developed());
    let changed = vec!["src/lib.rs".to_owned()];
    run.record_review_trees("pass".into(), vec!["a".repeat(40)], Some(&changed))
        .unwrap();
    assert_eq!(run.skipped_reviews(), [0, 1]);
    assert_eq!(run.phase, Phase::Review(2));
    assert_eq!(run.pending().len(), 1);
    assert_eq!(run.step(&run.pending()[0]).unwrap().name, "QA");
}

#[test]
fn the_timeline_keeps_a_group_together_and_puts_its_decision_after_it() {
    let mut run = in_group(&["/api"], 3);
    run.template.reviews[1].limit = 0;
    report_review(&mut run, 1, vec![issue(0)]);
    report_review(&mut run, 2, vec![]);
    assert_eq!(run.phase, Phase::Decision(1));
    let stages = Stage::recipe(&run);
    let reviews = stages
        .iter()
        .position(|stage| *stage == Stage::Review(0))
        .unwrap();
    assert_eq!(
        stages[reviews..],
        [
            Stage::Review(0),
            Stage::Review(1),
            Stage::Review(2),
            Stage::Decision(1),
            Stage::Accept,
        ]
    );
}
