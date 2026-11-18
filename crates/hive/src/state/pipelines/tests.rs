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
            p.run(42, cx).unwrap().lanes[0]
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
        assert_eq!(p.run(42, cx).unwrap().phase, Phase::Cancelled);
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
                run.task = "wrong".into();
                Ok(run.approve()?)
            })
            .is_err()
        );
        assert_eq!(p.run(42, cx).unwrap().task, "Task");
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
            run.phase = Phase::Acceptance;
            run.review_revision = Some("reviewed".into());
        };
        store.update(cx, |s, _| ready(s));
        p.accept_jobs.insert(42, 1);
        p.generations.insert(42, 1);
        p.finish_acceptance(42, 0, Ok("stale callback".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase, Phase::Acceptance);
        assert!(p.accepting(42));
        p.finish_acceptance(42, 1, Ok("manual edit".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase, Phase::Review(0));
        assert!(p.run(42, cx).unwrap().review_revision.is_none());
        store.update(cx, |s, _| ready(s));
        p.generations.insert(42, 3);
        p.accept_jobs.insert(42, 3);
        p.finish_acceptance(42, 3, Ok("reviewed".into()), cx);
        assert_eq!(p.run(42, cx).unwrap().phase, Phase::Accepted);
    });
}
