//! After acceptance: taking each reviewed branch to the Git panel and to a pull request.
use super::*;
use crate::state::pipelines::Delivery;
use hive_core::git::pull_request::Draft;

impl RunView {
    /// One row per checkout with accepted work: where its changes are and what to do with them.
    pub(super) fn delivery(
        &self,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if run.phase() != Phase::Accepted {
            return None;
        }
        let mut rows = v_flex().gap_2().pt_2();
        let mut any = false;
        for ix in run.delivery_lanes() {
            any = true;
            rows = rows.child(self.delivery_row(run, ix, w, cx));
        }
        any.then_some(rows)
    }

    fn delivery_row(&self, run: &Run, ix: usize, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let lane = &run.lanes()[ix];
        let state = AppState::get(cx)
            .pipelines
            .read(cx)
            .delivery(run.id(), ix)
            .cloned();
        let mut row = h_flex().gap_2().flex_wrap().child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().truncate().child(lane_label(lane)))
                .child(subtle(lane.worktree.display().to_string(), cx).truncate()),
        );
        let app = AppState::get(cx);
        let branch = hive_core::pipeline::workspace::branch(run.id(), lane);
        let head = format!("{:?}", hive_core::projects::Head::Branch(branch));
        let cached = match app
            .pull_requests
            .read(cx)
            .status(&Host::Local, &lane.worktree, &head)
        {
            Some(crate::state::pull_requests::Status::Ready(snapshot)) => {
                snapshot.request().cloned()
            }
            _ => None,
        };
        let pr = match &state {
            Some(Delivery::Existing(pr)) => Some(
                cached
                    .filter(|p| p.web_url() == pr.web_url())
                    .unwrap_or_else(|| pr.clone()),
            ),
            Some(Delivery::Opened(url)) => cached.filter(|p| p.web_url() == Some(url)),
            _ => None,
        };
        if let Some(pr) = pr
            && let Some(url) = pr.web_url().map(str::to_owned)
        {
            row = row
                .child(subtle(format!("{} · {}", pr.state, pr.checks()), cx))
                .child(button(
                    ("view-existing-request", ix),
                    "View PR",
                    ButtonLook::Primary,
                    w,
                    cx,
                    move |_, _, cx| cx.open_url(&url),
                ));
            if let Some(url) = pr.failed_check_url().map(str::to_owned) {
                row = row.child(button(
                    ("failed-ci", ix),
                    "Open failing check",
                    ButtonLook::Outline,
                    w,
                    cx,
                    move |_, _, cx| cx.open_url(&url),
                ));
            }
            return row;
        }
        let (project, worktree) = (lane.project.clone(), lane.worktree.clone());
        let open_worktree = button(
            ("open-run-worktree", ix),
            "Open worktree",
            ButtonLook::Outline,
            w,
            cx,
            move |_, w, cx| {
                // The Git panel of the selected worktree takes over: commit, push, history.
                AppState::get(cx)
                    .projects
                    .clone()
                    .update(cx, |projects, cx| {
                        projects.select(&Host::Local, &project, Some(&worktree), cx)
                    });
                w.dispatch_action(Box::new(TogglePipelines), cx);
            },
        );
        let refresh = button(
            ("refresh-delivery", ix),
            "Refresh",
            ButtonLook::Ghost,
            w,
            cx,
            |v, _, cx| {
                AppState::get(cx)
                    .pipelines
                    .clone()
                    .update(cx, |p, cx| p.refresh_delivery(v.id, cx))
            },
        );
        match state {
            Some(Delivery::Opening) => row.child(subtle("Opening pull request…", cx)),
            Some(Delivery::Opened(url)) => {
                row.child(subtle("Check status unavailable", cx))
                    .child(button(
                        ("view-run-pull-request", ix),
                        "View pull request",
                        ButtonLook::Primary,
                        w,
                        cx,
                        move |_, _, cx| cx.open_url(&url),
                    ))
            }
            None | Some(Delivery::Checking | Delivery::Existing(_)) => {
                row.child(subtle("Checking publication…", cx))
            }
            // A step the user still owes is not an error and stays muted.
            Some(Delivery::Unpublished(step)) => row
                .child(open_worktree)
                .child(refresh)
                .child(delivery_message(ix, step, cx.theme().muted_foreground)),
            Some(Delivery::Failed(error)) => row
                .child(open_worktree)
                .child(refresh)
                .child(delivery_message(ix, error, cx.theme().danger)),
            Some(Delivery::Ready) => row.child(button(
                ("create-run-pull-request", ix),
                "Create pull request…",
                ButtonLook::Outline,
                w,
                cx,
                move |v, w, cx| v.draft_pull_request(ix, w, cx),
            )),
        }
    }

    /// Shows what would be opened, editable, before anything leaves the machine.
    fn draft_pull_request(&self, ix: usize, w: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_ref() else { return };
        let Some((title, body)) = run.pull_request_text(ix) else {
            return;
        };
        let id = self.id;
        let branch = run.lanes()[ix].branch.clone();
        let title = input(&title, "Title", w, cx);
        let body = area(&body, "Description", w, cx);
        let base = input(
            &run.lanes()[ix].base_branch,
            "The repository's default branch",
            w,
            cx,
        );
        let view = cx.weak_entity();
        w.open_alert_dialog(cx, move |dialog, _, cx| {
            let (title, body, base) = (title.clone(), body.clone(), base.clone());
            let view = view.clone();
            dialog
                .title("Create pull request")
                .child(subtle(pull_request_hint(&branch), cx))
                .child(field("Title", &title, cx))
                .child(paragraph("Description", &body, cx))
                .child(field("Merge into", &base, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Create pull request")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let draft = Draft {
                                title: title.read(cx).value().to_string(),
                                body: body.read(cx).value().to_string(),
                                base: base.read(cx).value().trim().to_owned(),
                            };
                            if draft.title.trim().is_empty() {
                                return false;
                            }
                            let result = AppState::get(cx)
                                .pipelines
                                .clone()
                                .update(cx, |p, cx| p.open_pull_request(id, ix, draft, cx));
                            let _ = view.update(cx, |this, cx| {
                                this.error = result.err();
                                cx.notify();
                            });
                            true
                        }),
                )
        });
    }
}

/// A lane as the delivery row names it: its role and the branch holding its work.
/// Why the row has no request yet, on its own line and selectable: a hosting CLI's error is
/// something to copy.
fn delivery_message(ix: usize, text: String, color: Hsla) -> Div {
    div()
        .w_full()
        .text_sm()
        .text_color(color)
        .child(SelectableText::new(("delivery-message", ix), text))
}

fn lane_label(lane: &hive_core::pipeline::Lane) -> String {
    if lane.branch.is_empty() {
        lane.name.clone()
    } else {
        format!(
            "{}  ·  {} → {}",
            lane.name,
            lane.branch,
            if lane.base_branch.is_empty() {
                "default branch"
            } else {
                &lane.base_branch
            }
        )
    }
}

/// What has to be true on the remote before the request can be opened: Hive does not push.
fn pull_request_hint(branch: &str) -> String {
    let branch = if branch.is_empty() {
        "The branch".to_owned()
    } else {
        format!("Branch {branch}")
    };
    format!(
        "{branch} must already be committed and pushed. Hive opens the request with your gh or glab sign-in and does not push for you."
    )
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{lane_label, pull_request_hint};
    use hive_core::pipeline::Lane;

    #[test]
    fn a_delivery_row_names_the_lane_and_its_branch() {
        let mut lane = Lane {
            name: "API".into(),
            ..Default::default()
        };
        assert_eq!(lane_label(&lane), "API");
        lane.branch = "feature/login".into();
        assert_eq!(lane_label(&lane), "API  ·  feature/login → default branch");
        assert!(pull_request_hint("feature/login").starts_with("Branch feature/login must"));
        assert!(pull_request_hint("").starts_with("The branch must"));
    }
}
