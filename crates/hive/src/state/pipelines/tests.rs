use super::{Pipelines, Store};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use hive_core::state::ProjectRef;
use hive_core::{
    host::Host,
    pipeline::{Lane, Phase, Report, Run, Template},
};
use std::path::PathBuf;

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
        edited.planner.instruction.push_str(" Check both lanes.");
        p.save_template(Some(&first), edited.clone(), cx).unwrap();
        let saved = &p.store.read(cx).state.pipelines.templates;
        assert!(saved.contains(&source) && saved.contains(&second) && saved.contains(&edited));
        assert!(p.duplicate_template(&first, cx).is_err());
    });
}

#[gpui::test]
fn deleting_a_template_preserves_runs_and_rejects_stale_confirmations(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let run = p.run(42, cx).unwrap().clone();
        let original = run.template().clone();
        let mut edited = original.clone();
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
        assert!(
            p.report(
                7,
                &ticket,
                Report::Developed {
                    summary: "unauthorized".into()
                },
                cx
            )
            .is_err()
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
fn cancelling_drops_everything_held_for_the_run(cx: &mut TestAppContext) {
    let pipelines = recovering(cx);
    pipelines.update(cx, |p, cx| {
        let worker = std::sync::Arc::new(hive_core::command::Cancellation::default());
        let runtime = p.runtime_mut(42);
        runtime.error = Some("Checkout failed".into());
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
