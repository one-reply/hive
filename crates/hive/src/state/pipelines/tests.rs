use super::{Pipelines, Preview, PreviewKey, Store, check_branches};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use hive_core::state::ProjectRef;
use hive_core::{
    host::Host,
    pipeline::{Lane, Phase, Report, Run, Template},
};
use std::path::{Path, PathBuf};

fn recovering(cx: &mut TestAppContext) -> Entity<Pipelines> {
    let store = cx.new(Store::with_held_recovery);
    store.update(cx, |s, _| {
        let path = PathBuf::from("/unused-pipeline-test-project");
        s.state
            .projects
            .push(ProjectRef::new(path.clone(), Host::Local));
        let lane = Lane {
            name: "Web".into(),
            project: path.clone(),
            base: "HEAD".into(),
            ..Lane::default()
        };
        let run = Run::new(42, path, "Task".into(), Template::default(), vec![lane]).unwrap();
        s.state.pipelines.runs.push(run);
    });
    let agents = cx.new(|cx| crate::state::agents::Agents::new(None, cx));
    cx.new(|cx| Pipelines::new(store, &agents, cx))
}

/// A saved run at the acceptance gate, as the state file would hold it; the transitions to
/// get there need real worktrees.
fn awaiting_acceptance(run: &Run, revision: &str) -> Run {
    let mut saved = serde_json::to_value(run).unwrap();
    saved["phase"] = serde_json::to_value(Phase::Acceptance).unwrap();
    saved["review_revision"] = revision.into();
    serde_json::from_value(saved).unwrap()
}

#[gpui::test]
fn duplicates_the_entire_recipe_and_edits_only_the_chosen_copy(cx: &mut TestAppContext) {
    recovering(cx).update(cx, |p, cx| {
        let source = p
            .store
            .read(cx)
            .state
            .pipelines
            .templates
            .last()
            .unwrap()
            .clone();
        let first = p.duplicate_template(&source, cx).unwrap();
        let mut expected = source.clone();
        expected.name = format!("{} copy", source.name);
        assert_eq!(first, expected);
        let second = p.duplicate_template(&source, cx).unwrap();
        assert_eq!(second.name, format!("{} 2", first.name));
        let mut edited = first.clone();
        edited.planner.prompt = None;
        edited.planner.instruction.push_str(" Check both lanes.");
        p.save_template(Some(&first), edited.clone(), cx).unwrap();
        let saved = &p.store.read(cx).state.pipelines.templates;
        assert!(saved.contains(&source) && saved.contains(&second) && saved.contains(&edited));
        assert!(p.duplicate_template(&first, cx).is_err());
    });
}

#[gpui::test]
fn saving_a_template_refreshes_a_stage_whose_prompt_changed_while_it_was_open(
    cx: &mut TestAppContext,
) {
    recovering(cx).update(cx, |p, cx| {
        let loaded = p.store.read(cx).state.pipelines.templates[0].clone();
        let id = loaded.planner.prompt.unwrap();
        let run = p.run(42, cx).unwrap().clone();
        p.save_prompt(Some(id), "House planner", "Plan the house way.", cx)
            .unwrap();
        assert_eq!(p.run(42, cx).unwrap(), &run);

        // The editor still holds the recipe as it was before the prompt changed.
        let mut draft = loaded.clone();
        draft.reviews[0].limit = 1;
        p.save_template(Some(&loaded), draft, cx).unwrap();
        let saved = &p.store.read(cx).state.pipelines.templates[0];
        assert_eq!(saved.planner.instruction, "Plan the house way.");
        assert_eq!(saved.planner.prompt, Some(id));
        assert_eq!(saved.reviews[0].limit, 1);

        let copy = p.duplicate_prompt(id, cx).unwrap();
        p.delete_prompt(id, cx).unwrap();
        let library = &p.store.read(cx).state.pipelines;
        assert_eq!(library.prompt(copy).unwrap().name, "House planner copy");
        assert_eq!(library.templates[0].planner.prompt, None);
        assert_eq!(
            library.templates[0].planner.instruction,
            "Plan the house way."
        );
    });
}

#[gpui::test]
fn rejected_prompt_edit_changes_nothing_and_is_not_announced(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    let store = pipelines.read_with(cx, |p, _| p.store.clone());
    let announced = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = announced.clone();
    let _observer = cx.update(|cx| cx.observe(&store, move |_, _| counter.set(counter.get() + 1)));
    pipelines.update(cx, |p, cx| {
        let before = p.store.read(cx).state.pipelines.clone();
        let taken = before.prompts()[1].name.clone();
        assert!(
            p.save_prompt(Some(before.prompts()[0].id), &taken, "Text", cx)
                .is_err()
        );
        assert!(p.save_prompt(None, " ", "Text", cx).is_err());
        assert!(p.duplicate_prompt(u64::MAX, cx).is_err());
        assert!(p.delete_prompt(u64::MAX, cx).is_err());
        assert_eq!(p.store.read(cx).state.pipelines, before);
    });
    cx.run_until_parked();
    assert_eq!(announced.get(), 0);
    pipelines.update(cx, |p, cx| p.save_prompt(None, "Mine", "Text", cx).unwrap());
    cx.run_until_parked();
    assert_eq!(announced.get(), 1);
}

#[gpui::test]
fn deleting_a_template_preserves_runs_and_rejects_stale_confirmations(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let run = p.run(42, cx).unwrap().clone();
        let original = run.template().clone();
        let mut edited = original.clone();
        edited.planner.prompt = None;
        edited.planner.instruction.push_str(" Review the plan.");
        p.save_template(Some(&original), edited.clone(), cx)
            .unwrap();
        let before = p.store.read(cx).state.pipelines.templates.clone();
        assert!(p.delete_template(&original, cx).is_err());
        assert_eq!(p.store.read(cx).state.pipelines.templates, before);

        p.delete_template(&edited, cx).unwrap();
        assert!(p.delete_template(&edited, cx).is_err());
        let remaining: Vec<_> = before.into_iter().filter(|t| t != &edited).collect();
        assert_eq!(p.store.read(cx).state.pipelines.templates, remaining);
        assert_eq!(p.run(42, cx).unwrap(), &run);
    });
}

#[gpui::test]
fn restarted_attempt_requires_explicit_retry_and_failed_checkpoint_prevents_processes(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let ticket = p.run(42, cx).unwrap().pending()[0];
        assert!(p.error(42).is_some());
        assert!(p.session(&ticket).is_none());
        assert_eq!(p.attention(cx).len(), 1);
        p.report(
            7,
            ticket,
            Report::Developed {
                summary: "unauthorized".into(),
            },
            |answer| assert!(answer.is_err()),
            cx,
        );
        assert!(p.resume(&ticket, cx).is_err());
        p.retry(42, cx).unwrap();
        assert_ne!(ticket, p.run(42, cx).unwrap().pending()[0]);
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, cx| {
        assert!(p.error(42).unwrap().contains("checkpoint"));
        assert!(
            p.run(42, cx).unwrap().lanes()[0]
                .worktree
                .as_os_str()
                .is_empty()
        );
        assert!(p.live.is_empty());
    });
}

#[gpui::test]
fn refuses_to_start_a_run_that_cannot_be_saved(cx: &mut TestAppContext) {
    recovering(cx).update(cx, |p, cx| {
        let before = p.store.read(cx).state.pipelines.runs.clone();
        let owner = before[0].owner().to_path_buf();
        let lanes = before[0].lanes().to_vec();
        let error = p
            .start(
                owner,
                "Another".into(),
                Template::default(),
                lanes,
                Default::default(),
                cx,
            )
            .unwrap_err();
        assert!(error.contains("cannot be saved"));
        assert_eq!(p.store.read(cx).state.pipelines.runs, before);
    });
}

#[gpui::test]
fn retrying_a_failed_checkpoint_keeps_the_number_of_a_ticket_that_never_launched(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| p.retry(42, cx).unwrap());
    cx.run_until_parked();
    let ticket = pipelines.update(cx, |p, cx| {
        assert!(p.error(42).unwrap().contains("checkpoint"));
        let ticket = p.run(42, cx).unwrap().pending()[0];
        p.retry(42, cx).unwrap();
        ticket
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, cx| {
        let run = p.run(42, cx).unwrap();
        assert_eq!(run.pending()[0], ticket);
        let retries = run.history().iter().filter(|h| h.starts_with("Retried"));
        assert_eq!(retries.count(), 1);
    });
}

#[gpui::test]
fn cancelling_while_checkpoint_is_pending_ignores_late_dispatch_and_clears_attention(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.retry(42, cx).unwrap();
        p.cancel(42, cx).unwrap();
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, cx| {
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Cancelled);
        assert!(p.error(42).is_none());
        assert!(p.attention(cx).is_empty());
        assert!(p.live.is_empty());
    });
}

#[gpui::test]
fn invalid_decision_does_not_partially_edit_the_run(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        assert!(
            p.edit(42, cx, |run| {
                run.record_worktree(0, "/partial-edit".into());
                Ok(run.approve()?)
            })
            .is_err()
        );
        let run = p.run(42, cx).unwrap();
        assert!(run.lanes()[0].worktree.as_os_str().is_empty());
    });
}

#[gpui::test]
fn a_decision_that_moves_the_run_replaces_the_failed_step_it_leaves(cx: &mut TestAppContext) {
    use hive_core::pipeline::CriterionDecision;
    const STALE: &str = "The acceptance check could not read the worktree.";
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, cx| {
            let run = &mut s.state.pipelines.runs[0];
            let reviewed = reviewed(run, Phase::Acceptance, &"a".repeat(40));
            let mut saved = serde_json::to_value(reviewed).unwrap();
            saved["lanes"][0]["criteria"] = "Sign-in works".into();
            *run = serde_json::from_value(saved).unwrap();
            s.set_test_writer(cx, |_| Ok(()));
        });
        p.fail(42, STALE.into(), cx);

        // A note on a criterion leaves the run at the gate whose check failed.
        let observed = CriterionDecision::Manual("Signed in by hand".into());
        p.edit(42, cx, |run| {
            Ok(run.decide_criterion(0, "Sign-in works", "reviewed", observed)?)
        })
        .unwrap();
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);
        assert_eq!(p.error(42), Some(STALE));

        p.edit(42, cx, |run| {
            Ok(run.request_changes("Handle the expired session")?)
        })
        .unwrap();
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Development);
        assert_eq!(p.error(42), None);
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        // The development step ran instead of waiting for a retry: it got as far as the
        // fixture's checkout, which Git never made.
        let failure = p
            .error(42)
            .expect("the fixture has no checkout to develop in");
        assert_ne!(failure, STALE);
    });
}

#[gpui::test]
fn an_edit_that_leaves_work_in_the_same_phase_replaces_the_failed_step_too(
    cx: &mut TestAppContext,
) {
    const STALE: &str = "Preparing Web: the checkout is busy.";
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, cx| {
            let run = &mut s.state.pipelines.runs[0];
            let mut developing = reviewed(run, Phase::Acceptance, &"a".repeat(40));
            developing
                .request_changes("Handle the expired session")
                .unwrap();
            *run = developing;
            s.set_test_writer(cx, |_| Ok(()));
        });
        p.fail(42, STALE.into(), cx);
        let ticket = p.run(42, cx).unwrap().pending()[0];
        // No decision does this today; the rule must not depend on that.
        p.edit(42, cx, |run| Ok(run.restart_attempt(&ticket).map(drop)?))
            .unwrap();
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Development);
        assert_eq!(p.error(42), None);
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        let failure = p
            .error(42)
            .expect("the fixture has no checkout to develop in");
        assert_ne!(failure, STALE);
    });
}

#[gpui::test]
fn only_the_reviewed_revision_can_be_accepted(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        let store = p.store.clone();
        let ready = |s: &mut Store| {
            let run = &mut s.state.pipelines.runs[0];
            *run = awaiting_acceptance(run, "reviewed");
        };
        store.update(cx, |s, _| ready(s));
        let runtime = p.runtime_mut(42);
        runtime.accept_job = Some(1);
        runtime.generation = 1;
        p.finish_acceptance(42, 0, Ok("stale callback".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);
        assert!(p.accepting(42));
        p.finish_acceptance(42, 1, Ok("manual edit".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Review(0));
        assert!(p.run(42, cx).unwrap().review_revision().is_none());
        store.update(cx, |s, _| ready(s));
        let runtime = p.runtime_mut(42);
        runtime.generation = 3;
        runtime.accept_job = Some(3);
        p.finish_acceptance(42, 3, Ok("reviewed".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Accepted);
    });
}

#[gpui::test]
fn a_checkpoint_that_could_not_be_saved_is_forgotten_once_a_later_one_is(cx: &mut TestAppContext) {
    use hive_core::pipeline::CriterionDecision;
    fn note(p: &mut Pipelines, criterion: &str, cx: &mut gpui::Context<Pipelines>) {
        let observed = CriterionDecision::Manual("Checked by hand".into());
        p.edit(42, cx, |run| {
            Ok(run.decide_criterion(0, criterion, "reviewed", observed)?)
        })
        .unwrap();
    }
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            let reviewed = reviewed(run, Phase::Acceptance, &"a".repeat(40));
            let mut saved = serde_json::to_value(reviewed).unwrap();
            saved["lanes"][0]["criteria"] = "Sign-in works\nSign-out works".into();
            *run = serde_json::from_value(saved).unwrap();
        });
        // The fixture cannot save yet.
        note(p, "Sign-in works", cx);
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        assert!(p.error(42).unwrap().contains("checkpoint"));
        p.store
            .update(cx, |s, cx| s.set_test_writer(cx, |_| Ok(())));
        // The next edit stays at the gate and schedules nothing, but its save holds both.
        note(p, "Sign-out works", cx);
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        assert_eq!(p.error(42), None);

        // Any other failure is not about saving and outlives a save.
        p.fail(42, "The checkout is busy.".into(), cx);
        note(p, "Sign-in works", cx);
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        assert_eq!(p.error(42), Some("The checkout is busy."))
    });
}

#[gpui::test]
fn retry_repeats_the_acceptance_check_only_when_that_check_is_what_failed(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    let tree = "a".repeat(40);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            let decision = reviewed(run, Phase::Decision(1), &tree);
            let mut saved = serde_json::to_value(decision).unwrap();
            saved["findings"] = serde_json::json!([{"lane": 0, "description": "Left open"}]);
            *run = serde_json::from_value(saved).unwrap();
        });
        // The user's own decision reaches the gate; the fixture cannot save it.
        p.edit(42, cx, |run| {
            Ok(run.dismiss_finding(0, "Tracked separately")?)
        })
        .unwrap();
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        assert!(p.error(42).unwrap().contains("checkpoint"));
        p.retry(42, cx).unwrap();
        assert!(!p.accepting(42), "saving again is not accepting");
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);
        assert!(p.error(42).unwrap().contains("checkpoint"));

        // The check Accept started could not read the worktree: Retry asks it again.
        let runtime = p.runtime_mut(42);
        runtime.generation = 7;
        runtime.accept_job = Some(7);
        p.finish_acceptance(42, 7, Err("fatal: not a git repository".into()), cx);
        assert_eq!(p.error(42), Some("fatal: not a git repository"));
        p.retry(42, cx).unwrap();
        assert!(p.accepting(42));
        assert_eq!(p.error(42), None);
    });
}

#[gpui::test]
fn cancelling_drops_everything_held_for_the_run(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let worker = std::sync::Arc::new(hive_core::command::Cancellation::default());
        let runtime = p.runtime_mut(42);
        runtime.error = Some(super::runtime::Failure::Step("Checkout failed".into()));
        runtime.worker = Some(worker.clone());
        runtime.review = Some(Err("unreadable".into()));
        runtime.review_job = Some(7);
        runtime.accept_job = Some(1);
        p.cancel(42, cx).unwrap();
        assert!(p.runtime(42).is_none());
        assert!(worker.is_cancelled());
        assert!(p.error(42).is_none());
        assert!(!p.accepting(42));
        assert!(p.review(42).is_none());
    });
}

fn attach_session(
    pipelines: &mut Pipelines,
    ticket: super::Ticket,
    cx: &mut gpui::Context<Pipelines>,
) -> u64 {
    use hive_core::terminal::{Terminal, TerminalSize};
    let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(80, 24)).unwrap();
    let session = cx.new(|_| super::TerminalSession::for_test(terminal));
    let id = session.entity_id().as_u64();
    pipelines.suspended.remove(&ticket);
    pipelines.live.insert(
        ticket,
        super::Live {
            session,
            phase: pipelines.run(ticket.run, cx).unwrap().phase(),
            _subscriptions: vec![],
        },
    );
    id
}

fn submit(
    p: &mut Pipelines,
    from: u64,
    ticket: super::Ticket,
    report: Report,
    cx: &mut gpui::Context<Pipelines>,
) -> std::sync::mpsc::Receiver<Result<(), String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    p.report(
        from,
        ticket,
        report,
        move |answer| tx.send(answer).unwrap(),
        cx,
    );
    rx
}

fn plan(lanes: usize) -> Report {
    Report::Plan {
        contract: String::new(),
        assignments: (0..lanes)
            .map(|lane| hive_core::pipeline::Assignment {
                lane,
                task: "Implement".into(),
                criteria: "Tests pass".into(),
            })
            .collect(),
    }
}

#[gpui::test]
fn report_retries_after_write_failure_without_closing_the_session_or_reapplying_the_transition(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    let (ticket, from, first) = pipelines.update(cx, |p, cx| {
        let ticket = p.run(42, cx).unwrap().pending()[0];
        let from = attach_session(p, ticket, cx);
        let first = submit(p, from, ticket, plan(1), cx);
        assert!(matches!(
            first.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        assert!(p.edit(42, cx, |run| Ok(run.approve()?)).is_err());
        assert_eq!(p.live_attempts(42), [(ticket, Phase::Planning)]);
        (ticket, from, first)
    });
    cx.run_until_parked();
    assert!(first.try_recv().unwrap().is_err());
    let (retry, duplicate) = pipelines.update(cx, |p, cx| {
        assert!(p.session(&ticket).is_some());
        p.store.update(cx, |s, cx| {
            s.set_test_writer(cx, |state| {
                assert_eq!(state.pipelines.runs[0].phase(), Phase::ApprovePlan);
                assert_eq!(state.pipelines.runs[0].reports().len(), 1);
                Ok(())
            })
        });
        let bad = Report::Developed {
            summary: "replacement".into(),
        };
        assert!(
            submit(p, from, ticket, bad, cx)
                .try_recv()
                .unwrap()
                .is_err()
        );
        let retry = submit(p, from, ticket, plan(1), cx);
        let duplicate = submit(p, from, ticket, plan(1), cx);
        assert!(retry.try_recv().is_err() && duplicate.try_recv().is_err());
        (retry, duplicate)
    });
    cx.run_until_parked();
    assert_eq!(retry.try_recv().unwrap(), Ok(()));
    assert_eq!(duplicate.try_recv().unwrap(), Ok(()));
    pipelines.read_with(cx, |p, cx| {
        assert!(p.session(&ticket).is_none());
        assert!(p.error(42).is_none());
        assert_eq!(p.run(42, cx).unwrap().reports().len(), 1);
    });
}

#[gpui::test]
fn parallel_reports_share_the_latest_durable_checkpoint_before_dispatch(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    let replies = pipelines.update(cx, |p, cx| {
        p.store.update(cx, |s, cx| {
            let lanes = ["web", "api"].map(|name| Lane {
                name: name.into(),
                project: PathBuf::from("/fixture").join(name),
                base: "HEAD".into(),
                ..Default::default()
            });
            let mut run = Run::new(
                42,
                "/fixture".into(),
                "Task".into(),
                Template::default(),
                lanes.to_vec(),
            )
            .unwrap();
            let planner = run.pending()[0];
            run.report(&planner, plan(2)).unwrap();
            run.approve().unwrap();
            s.state.pipelines.runs[0] = run;
            s.set_test_writer(cx, |_| Ok(()));
        });
        let tickets = p.run(42, cx).unwrap().pending();
        assert_eq!(tickets.len(), 2);
        let mut replies = vec![];
        for ticket in tickets {
            let from = attach_session(p, ticket, cx);
            replies.push(submit(
                p,
                from,
                ticket,
                Report::Developed {
                    summary: "done".into(),
                },
                cx,
            ));
        }
        assert_eq!(p.live_attempts(42).len(), 2);
        assert!(replies.iter().all(|reply| reply.try_recv().is_err()));
        replies
    });
    cx.run_until_parked();
    assert!(
        replies
            .iter()
            .all(|reply| reply.try_recv().unwrap().is_ok())
    );
    pipelines.read_with(cx, |p, cx| {
        assert!(p.live.is_empty());
        assert_eq!(p.run(42, cx).unwrap().reports().len(), 3);
        assert!(p.error(42).unwrap().contains("registered"));
    });
}

/// Every `RunWaits` the entity emits from now on, in order.
fn announcements(
    pipelines: &Entity<Pipelines>,
    cx: &mut TestAppContext,
) -> (
    std::rc::Rc<std::cell::RefCell<Vec<super::RunWaits>>>,
    gpui::Subscription,
) {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = seen.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(pipelines, move |_, event: &super::RunWaits, _| {
            sink.borrow_mut().push(event.clone())
        })
    });
    (seen, subscription)
}

#[gpui::test]
fn a_gate_reached_by_an_agents_report_is_announced_once(cx: &mut TestAppContext) {
    use super::status::{Gate, Waiting};
    let pipelines = recovering(cx);
    let (seen, _subscription) = announcements(&pipelines, cx);
    let replies = pipelines.update(cx, |p, cx| {
        p.store
            .update(cx, |s, cx| s.set_test_writer(cx, |_| Ok(())));
        let ticket = p.run(42, cx).unwrap().pending()[0];
        let from = attach_session(p, ticket, cx);
        // A repeated report of the same ticket is the same gate, not a second one.
        [(); 2].map(|()| submit(p, from, ticket, plan(1), cx))
    });
    cx.run_until_parked();
    for reply in replies {
        assert_eq!(reply.try_recv().unwrap(), Ok(()));
    }
    let plan_ready = super::RunWaits {
        run: 42,
        reason: Waiting::Gate(Gate::Plan),
    };
    assert_eq!(*seen.borrow(), std::slice::from_ref(&plan_ready));
    pipelines.read_with(cx, |p, cx| {
        assert_eq!(
            p.notice(&plan_ready, cx),
            Some(("Plan ready for approval".into(), "Task".into()))
        );
    });
}

#[gpui::test]
fn a_gate_the_user_reaches_by_their_own_decision_is_not_announced(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    let (seen, _subscription) = announcements(&pipelines, cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            let mut saved = serde_json::to_value(&*run).unwrap();
            saved["phase"] = serde_json::to_value(Phase::Decision(1)).unwrap();
            saved["outstanding"] = serde_json::json!([]);
            saved["findings"] = serde_json::json!([{"lane": 0, "description": "Left open"}]);
            *run = serde_json::from_value(saved).unwrap();
        });
        p.edit(42, cx, |run| {
            Ok(run.dismiss_finding(0, "Tracked separately")?)
        })
        .unwrap();
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);
    });
    assert!(seen.borrow().is_empty());
}

#[gpui::test]
fn a_step_failure_is_announced_once_until_it_changes(cx: &mut TestAppContext) {
    use super::status::Waiting;
    let pipelines = recovering(cx);
    let (seen, _subscription) = announcements(&pipelines, cx);
    pipelines.update(cx, |p, cx| {
        p.fail(42, "Checkout failed".into(), cx);
        p.fail(42, "Checkout failed".into(), cx);
    });
    assert_eq!(
        *seen.borrow(),
        [super::RunWaits {
            run: 42,
            reason: Waiting::Failed("Checkout failed".into()),
        }]
    );
    pipelines.update(cx, |p, cx| p.fail(42, "Disk full".into(), cx));
    assert_eq!(seen.borrow().len(), 2);
}

/// The agent of the fixture's planning attempt, as the agents entity sees it.
struct StepAgent {
    agents: Entity<crate::state::agents::Agents>,
    pane: gpui::EntityId,
}

impl StepAgent {
    /// Makes the attempt live in a pipeline pane watched as one `kind` was launched in.
    fn attach(
        pipelines: &Entity<Pipelines>,
        kind: hive_core::agent::AgentKind,
        cx: &mut TestAppContext,
    ) -> Self {
        pipelines.update(cx, |p, cx| {
            let ticket = p.run(42, cx).unwrap().pending()[0];
            attach_session(p, ticket, cx);
            let session = p.session(&ticket).unwrap();
            session.update(cx, |session, _| {
                let grant = hive_core::agent::api::ReportGrant::generate().unwrap();
                session.report_grant = Some(grant);
            });
            p.agents.update(cx, |agents, cx| {
                let terminal = session.read(cx).terminal().unwrap();
                agents.watch(&session, &Host::Local, Some(kind), Path::new("/"), terminal);
            });
            Self {
                agents: p.agents.clone(),
                pane: session.entity_id(),
            }
        })
    }

    /// The agent reports `status`, as its hooks or OSC 7501 would.
    fn report(&self, status: hive_core::agent::AgentStatus, cx: &mut TestAppContext) {
        self.agents.update(cx, |agents, cx| {
            agents.report_for_test(self.pane, status, cx)
        });
    }

    /// Time passes with the agent silent, and the poll looks again.
    fn stay_quiet(&self, cx: &mut TestAppContext) {
        self.agents
            .update(cx, |agents, cx| agents.settle_quiet_turns_for_test(cx));
    }
}

fn run_status(pipelines: &Entity<Pipelines>, cx: &mut TestAppContext) -> super::RunStatus {
    pipelines.read_with(cx, |p, cx| p.status(p.run(42, cx).unwrap(), cx))
}

#[gpui::test]
fn a_step_whose_agent_went_idle_after_working_without_a_report_waits_for_the_user(
    cx: &mut TestAppContext,
) {
    use super::RunStatus;
    use hive_core::agent::{AgentKind, AgentStatus};
    let pipelines = recovering(cx);
    let agent = StepAgent::attach(&pipelines, AgentKind::Codex, cx);
    let redraws = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = redraws.clone();
    let _observer =
        cx.update(|cx| cx.observe(&pipelines, move |_, _| counter.set(counter.get() + 1)));

    // Codex is idle until it takes its prompt: the step has only started, however long.
    agent.report(AgentStatus::Idle, cx);
    agent.stay_quiet(cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);
    agent.report(AgentStatus::Working, cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);

    // Idle in the middle of a turn: it works again before anyone is asked anything.
    agent.report(AgentStatus::Idle, cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);
    agent.report(AgentStatus::Working, cx);
    agent.stay_quiet(cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);
    pipelines.read_with(cx, |p, cx| assert!(p.attention(cx).is_empty()));
    let before = redraws.get();

    // It ended the turn with a question in plain text instead of a report.
    agent.report(AgentStatus::Idle, cx);
    agent.stay_quiet(cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::NeedsYou);
    pipelines.read_with(cx, |p, cx| {
        let attention = p.attention(cx);
        assert_eq!(attention.len(), 1);
        assert!(attention[0].2.ends_with("Agent waits for a reply"));
        // The request is announced by the agents; the slow-attempt notice stays out of it.
        let ticket = p.run(42, cx).unwrap().pending()[0];
        assert!(p.agent_waits(&ticket, cx));
    });
    assert!(redraws.get() > before, "views of the run are told");

    // The user answered in the session; the next unreported turn asks again.
    agent.report(AgentStatus::Working, cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);
    agent.report(AgentStatus::Idle, cx);
    agent.stay_quiet(cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::NeedsYou);
}

#[gpui::test]
fn claude_code_going_idle_in_the_middle_of_a_turn_keeps_its_step_running(cx: &mut TestAppContext) {
    use super::RunStatus;
    use hive_core::agent::{AgentKind, AgentStatus};
    let pipelines = recovering(cx);
    let agent = StepAgent::attach(&pipelines, AgentKind::ClaudeCode, cx);
    // `UserPromptSubmit`, then the `SessionStart` of a compaction.
    agent.report(AgentStatus::Working, cx);
    agent.report(AgentStatus::Idle, cx);
    agent.stay_quiet(cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::Running);
    pipelines.read_with(cx, |p, cx| assert!(p.attention(cx).is_empty()));
    // Its own `done` is what ends a turn without a report.
    agent.report(AgentStatus::Done, cx);
    assert_eq!(run_status(&pipelines, cx), RunStatus::NeedsYou);
}

#[gpui::test]
fn an_attempt_past_its_time_budget_is_announced_once(cx: &mut TestAppContext) {
    use super::status::Waiting;
    const STARTED: u64 = 1_000;
    const BUDGET: u64 = 30 * 60 * 1000;
    let pipelines = recovering(cx);
    let (seen, _subscription) = announcements(&pipelines, cx);
    pipelines.update(cx, |p, cx| {
        let ticket = p.run(42, cx).unwrap().pending()[0];
        attach_session(p, ticket, cx);
        p.store.update(cx, |s, _| {
            s.state.pipelines.runs[0].start_attempt_at(ticket, STARTED)
        });
        p.announce_slow_attempts(STARTED + BUDGET - 1, cx);
    });
    assert!(seen.borrow().is_empty());
    pipelines.update(cx, |p, cx| {
        p.announce_slow_attempts(STARTED + BUDGET, cx);
        p.announce_slow_attempts(STARTED + 2 * BUDGET, cx);
    });
    let seen = seen.borrow();
    assert_eq!(seen.len(), 1);
    assert!(matches!(seen[0].reason, Waiting::Slow { minutes: 30, .. }));
    let (title, body) = seen[0].reason.notice("Task");
    assert!(title.ends_with("is still running after 30 min"), "{title}");
    assert_eq!(body, "Task");
}

#[gpui::test]
fn only_a_finished_run_can_be_deleted_and_nothing_is_kept_for_it(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let error = p.delete_run(42, cx).unwrap_err();
        assert!(error.contains("Stop the run"), "{error}");
        assert!(p.run(42, cx).is_ok());
        p.cancel(42, cx).unwrap();
        p.runtime_mut(42).error = Some(super::runtime::Failure::Step("A stale error".into()));
        p.delete_run(42, cx).unwrap();
        assert!(p.run(42, cx).is_err());
        assert!(p.runtime(42).is_none());
        assert!(p.attention(cx).is_empty());
        assert!(p.delete_run(42, cx).is_err());
    });
}

#[gpui::test]
fn a_pull_request_needs_accepted_work_and_its_outcome_is_kept_for_the_lane(
    cx: &mut TestAppContext,
) {
    use super::Delivery;
    use hive_core::git::pull_request::Draft;
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let draft = || Draft {
            title: "Add login".into(),
            ..Draft::default()
        };
        let early = p.open_pull_request(42, 0, draft(), cx).unwrap_err();
        assert!(early.contains("Accept the changes"), "{early}");
        p.suspended.clear();
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            let mut saved = serde_json::to_value(&*run).unwrap();
            saved["phase"] = serde_json::to_value(Phase::Accepted).unwrap();
            saved["outstanding"] = serde_json::json!([]);
            *run = serde_json::from_value(saved).unwrap();
        });
        // The fixture's lane never got a checkout, so nothing is asked of the hosting CLI.
        let nowhere = p.open_pull_request(42, 0, draft(), cx).unwrap_err();
        assert!(nowhere.contains("no worktree"), "{nowhere}");
        assert_eq!(p.delivery(42, 0), None);

        p.runtime_mut(42);
        p.finish_pull_request(42, 0, Err("gh: push the branch first".into()), cx);
        assert_eq!(
            p.delivery(42, 0),
            Some(&Delivery::Failed("gh: push the branch first".into()))
        );
        let url = "https://github.com/acme/app/pull/3";
        p.finish_pull_request(42, 0, Ok(url.into()), cx);
        assert_eq!(p.delivery(42, 0), Some(&Delivery::Opened(url.into())));
        assert_eq!(p.delivery(42, 1), None);

        p.delete_run(42, cx).unwrap();
        p.finish_pull_request(42, 0, Ok(url.into()), cx);
        assert_eq!(p.delivery(42, 0), None);
    });
}

/// The fixture run at `phase` after a review of `tree`, its lane assigned and bound to a
/// checkout Git never made.
fn reviewed(run: &Run, phase: Phase, tree: &str) -> Run {
    let mut run = run.clone();
    run.record_worktree(0, "/unused-pipeline-test-project-worktrees/missing".into());
    let mut saved = serde_json::to_value(&run).unwrap();
    saved["phase"] = serde_json::to_value(phase).unwrap();
    saved["outstanding"] = serde_json::json!([]);
    saved["review_revision"] = "reviewed".into();
    saved["review_trees"] = serde_json::json!([tree]);
    saved["lanes"][0]["assignment"] = "Add login".into();
    serde_json::from_value(saved).unwrap()
}

fn line_comment(tree: &str) -> hive_core::git::review_notes::Note {
    use hive_core::git::{diff::DiffSource, review_notes::Line};
    hive_core::git::review_notes::Note {
        path: "login.rs".into(),
        source: DiffSource::Compare(tree.into()),
        start: Line::Modified(3),
        end: Line::Modified(4),
        text: "Handle the expired session".into(),
    }
}

#[gpui::test]
fn acceptance_waits_until_queued_line_comments_are_sent_or_removed(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        let tree = "a".repeat(40);
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            *run = reviewed(run, Phase::Acceptance, &tree);
        });
        let note = line_comment(&tree);
        p.edit_comments(42, cx, |r| r.add_comment(0, tree.clone(), note.clone()))
            .unwrap();
        // A comment on a tree the review no longer shows is refused and nothing is saved.
        let stale = p.edit_comments(42, cx, |r| r.add_comment(0, "b".repeat(40), note.clone()));
        assert!(stale.unwrap_err().contains("no longer matches"));
        assert_eq!(p.run(42, cx).unwrap().comments().len(), 1);

        let unsent = p.accept(42, cx).unwrap_err();
        assert!(unsent.contains("line comments"), "{unsent}");
        assert!(!p.accepting(42));
        assert_eq!(p.run(42, cx).unwrap().phase(), Phase::Acceptance);

        let queued = (0, tree.clone(), note.clone());
        p.edit_comments(42, cx, |r| {
            r.remove_comment(&queued);
            Ok(())
        })
        .unwrap();
        p.accept(42, cx).unwrap();
        assert!(p.accepting(42));
    });
}

#[gpui::test]
async fn comments_for_an_unreadable_checkout_stay_queued_and_the_run_stays_at_acceptance(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    let tree = "a".repeat(40);
    let note = line_comment(&tree);
    let sending = pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        p.store.update(cx, |s, _| {
            let run = &mut s.state.pipelines.runs[0];
            *run = reviewed(run, Phase::Acceptance, &tree);
        });
        p.edit_comments(42, cx, |r| r.add_comment(0, tree.clone(), note.clone()))
            .unwrap();
        p.send_comments(42, 0, tree.clone(), vec![note.clone()], cx)
    });
    assert!(sending.await.is_err());
    pipelines.read_with(cx, |p, cx| {
        let run = p.run(42, cx).unwrap();
        assert_eq!(run.phase(), Phase::Acceptance);
        assert_eq!(run.comments(), [(0, tree.clone(), note.clone())]);
        assert!(run.pending().is_empty());
    });
    let missing = pipelines.update(cx, |p, cx| p.send_comments(7, 0, tree, vec![note], cx));
    assert!(missing.await.unwrap_err().contains("unavailable"));
}

#[gpui::test]
fn delivery_is_checked_only_after_acceptance_and_a_missing_checkout_is_not_an_error(
    cx: &mut TestAppContext,
) {
    use super::Delivery;
    let pipelines = recovering(cx);
    let tree = "a".repeat(40);
    pipelines.update(cx, |p, cx| {
        p.suspended.clear();
        let store = p.store.clone();
        let at = |phase, s: &mut Store| {
            let run = &mut s.state.pipelines.runs[0];
            *run = reviewed(run, phase, &tree);
        };
        store.update(cx, |s, _| at(Phase::Acceptance, s));
        p.refresh_delivery(42, cx);
        assert_eq!(p.delivery(42, 0), None);
        store.update(cx, |s, _| at(Phase::Accepted, s));
        p.refresh_delivery(42, cx);
        assert_eq!(p.delivery(42, 0), Some(&Delivery::Checking));
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        // Git has no such checkout, so the hosting CLI was never asked and nothing is recorded.
        assert!(
            matches!(p.delivery(42, 0), Some(Delivery::Unpublished(_))),
            "{:?}",
            p.delivery(42, 0)
        );
        assert_eq!(p.run(42, cx).unwrap().request_url(0), None);
        // A request the user starts while a check is running is not overwritten by its answer.
        p.refresh_delivery(42, cx);
        assert_eq!(p.delivery(42, 0), Some(&Delivery::Checking));
        p.runtime_mut(42).deliveries.insert(0, Delivery::Opening);
        p.refresh_delivery(42, cx);
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        assert_eq!(p.delivery(42, 0), Some(&Delivery::Opening));
    });
}

fn lane_on(name: &str, branch: &str) -> Lane {
    Lane {
        name: name.into(),
        project: PathBuf::from("/repo"),
        branch: branch.into(),
        ..Lane::default()
    }
}

#[test]
fn a_new_run_needs_a_valid_branch_for_every_lane() {
    let repo = Path::new("/repo-worktrees");
    let missing = [lane_on("api", "")];
    assert!(
        check_branches(missing.iter().map(|lane| (repo, lane)))
            .unwrap_err()
            .contains("api")
    );
    let invalid = [lane_on("api", "bad name")];
    assert!(check_branches(invalid.iter().map(|lane| (repo, lane))).is_err());
    let valid = [lane_on("api", "feature/login")];
    assert!(check_branches(valid.iter().map(|lane| (repo, lane))).is_ok());
}

#[test]
fn lanes_in_one_repository_share_its_branch_and_other_repositories_may_differ() {
    let (web, api) = (Path::new("/web-worktrees"), Path::new("/api-worktrees"));
    let lanes = [lane_on("ui", "feature"), lane_on("docs", "other")];
    assert!(
        check_branches([(web, &lanes[0]), (web, &lanes[1])])
            .unwrap_err()
            .contains("same branch")
    );
    assert!(check_branches([(web, &lanes[0]), (api, &lanes[1])]).is_ok());
}

/// A Git repository in its own temporary directory, removed with the test.
struct TempRepository(PathBuf);

impl TempRepository {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("hive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        let repository = Self(path.canonicalize().unwrap());
        hive_core::git::testing::init(&repository.0);
        repository
    }

    fn git(&self, args: &[&str]) {
        hive_core::git::testing::run(&self.0, args);
    }

    fn commit(&self, file: &str) {
        std::fs::write(self.0.join(file), file).unwrap();
        self.git(&["add", "--", file]);
        self.git(&["commit", "-m", file]);
    }

    fn key(&self, reference: &str) -> PreviewKey {
        PreviewKey {
            project: self.0.clone(),
            checkout: self.0.clone(),
            reference: reference.into(),
        }
    }
}

impl Drop for TempRepository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The commit and the local-changes flag of a base that has been read.
fn read_base(pipelines: &Pipelines, key: &PreviewKey) -> (String, bool) {
    match pipelines.preview(key) {
        Preview::Ready(preview) => (preview.commit.clone(), preview.dirty),
        other => panic!("the base is not read: {other:?}"),
    }
}

#[gpui::test]
fn reading_a_base_again_sees_where_its_branch_moved_and_what_changed_locally(
    cx: &mut TestAppContext,
) {
    let repository = TempRepository::new("pipeline-base-moved");
    repository.commit("first.txt");
    let key = repository.key("refs/heads/main");
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.store.update(cx, |s, _| {
            let project = ProjectRef::new(repository.0.clone(), Host::Local);
            s.state.projects.push(project);
        });
        p.refresh_preview(key.clone(), cx);
        assert!(matches!(p.preview(&key), Preview::Reading));
    });
    cx.run_until_parked();
    let (first, dirty) = pipelines.read_with(cx, |p, _| read_base(p, &key));
    assert!(!dirty);

    // What a terminal does while the form stays open.
    repository.commit("second.txt");
    std::fs::write(repository.0.join("untracked.txt"), "local").unwrap();
    pipelines.update(cx, |p, cx| {
        p.refresh_preview(key.clone(), cx);
        // The earlier answer stays on screen until the new one arrives.
        assert!(p.is_reading_preview(&key));
        assert_eq!(read_base(p, &key), (first.clone(), false));
    });
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        let (second, dirty) = read_base(p, &key);
        assert_ne!(second, first);
        assert!(dirty);
        assert!(!p.is_reading_preview(&key));
        // A run starts only from the commit read last.
        assert!(p.previews.resolved(&repository.0, &second).is_some());
        assert!(p.previews.resolved(&repository.0, &first).is_none());
    });
}

#[gpui::test]
fn a_base_that_cannot_be_read_fails_with_gits_answer_and_can_be_read_again(
    cx: &mut TestAppContext,
) {
    let repository = TempRepository::new("pipeline-base-unreadable");
    // No commit yet, so `main` resolves to nothing.
    let key = repository.key("refs/heads/main");
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        p.store.update(cx, |s, _| {
            let project = ProjectRef::new(repository.0.clone(), Host::Local);
            s.state.projects.push(project);
        });
        p.refresh_preview(key.clone(), cx);
    });
    cx.run_until_parked();
    pipelines.update(cx, |p, cx| {
        let Preview::Failed(failure) = p.preview(&key) else {
            panic!("an unborn branch was read: {:?}", p.preview(&key));
        };
        // Git's own line, without the command and the folder that the detail adds.
        assert!(failure.summary.starts_with("fatal:"), "{failure:?}");
        assert!(failure.detail.contains("rev-parse"), "{failure:?}");
        assert!(!p.is_reading_preview(&key));
        // The retry shows as a read in progress, not as the old failure.
        p.refresh_preview(key.clone(), cx);
        assert!(matches!(p.preview(&key), Preview::Reading));
    });
    repository.commit("first.txt");
    cx.run_until_parked();
    // The retry was started before the commit existed; whichever it saw, one more read is
    // conclusive.
    pipelines.update(cx, |p, cx| p.refresh_preview(key.clone(), cx));
    cx.run_until_parked();
    pipelines.read_with(cx, |p, _| {
        assert!(matches!(p.preview(&key), Preview::Ready(_)));
    });
}

#[gpui::test]
fn a_repository_removed_from_hive_fails_its_read_instead_of_staying_unread(
    cx: &mut TestAppContext,
) {
    let pipelines = recovering(cx);
    let key = PreviewKey {
        project: "/removed-from-hive".into(),
        checkout: "/removed-from-hive".into(),
        reference: "HEAD".into(),
    };
    pipelines.update(cx, |p, cx| {
        p.refresh_preview(key.clone(), cx);
        let Preview::Failed(failure) = p.preview(&key) else {
            panic!("the removed project is still being read");
        };
        let said = &failure.summary;
        assert!(said.contains("no longer a project in Hive"), "{said}");
        assert!(!p.is_reading_preview(&key));
    });
}
