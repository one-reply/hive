use super::*;
use crate::state::pull_requests::{Status, can_connect};
use gpui_kit::component::Disableable as _;
use hive_core::git::pull_request::Provider;
pub(super) const HEIGHT: Pixels = px(24.);

fn request_labels(provider: Provider) -> (&'static str, &'static str, &'static str) {
    match provider {
        Provider::GitHub => ("#", "Open pull request", "No pull request"),
        Provider::GitLab => ("!", "Open merge request", "No merge request"),
    }
}

impl Sidebar {
    pub(super) fn pr_card(
        &self,
        project: &Project,
        worktree: &Worktree,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let head = format!("{:?}", worktree.head);
        let status = AppState::get(cx)
            .pull_requests
            .read(cx)
            .status(&project.host, &worktree.path, &head)
            .cloned();
        let hover = Hover::new(
            SharedString::from(format!("pr:{}", worktree.path.display())),
            window,
            cx,
        );
        let check_hover = Hover::new(
            SharedString::from(format!("ci:{}", worktree.path.display())),
            window,
            cx,
        );
        let row = h_flex()
            .h(HEIGHT)
            .gap_1()
            .items_center()
            .text_xs()
            .text_color(cx.theme().muted_foreground);
        let can_connect = status
            .as_ref()
            .and_then(|status| can_connect(&project.host, status))
            .is_some();
        match status {
            Some(Status::Ready(snapshot)) => {
                let (prefix, tooltip, empty) = request_labels(snapshot.provider());
                let Some(pr) = snapshot.request() else {
                    return row.child(empty).into_any_element();
                };
                let label = format!("{prefix}{} · {}", pr.number, pr.state.to_lowercase());
                let url = pr.web_url().map(str::to_owned);
                let check_url = pr.failed_check_url().map(str::to_owned);
                row.child(
                    Button::new("open-pr")
                        .cursor_pointer()
                        .xsmall()
                        .label(label)
                        .tooltip(tooltip)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            if let Some(url) = &url {
                                cx.open_url(url);
                            }
                        })
                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                )
                .child(
                    Button::new("open-check")
                        .cursor_pointer()
                        .xsmall()
                        .label(pr.checks())
                        .tooltip("Open failing CI run")
                        .disabled(check_url.is_none())
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            if let Some(url) = &check_url {
                                cx.open_url(url);
                            }
                        })
                        .smooth_hover(ButtonLook::Ghost, &check_hover, cx),
                )
                .into_any_element()
            }
            Some(Status::Unavailable { message: error, .. }) => {
                if !can_connect {
                    return row
                        .child(
                            div()
                                .id("pr-unavailable-detail")
                                .child("PR/MR unavailable")
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(error.clone())
                                        .build(window, cx)
                                }),
                        )
                        .into_any_element();
                }
                row.child(
                    Button::new("pr-unavailable")
                        .cursor_pointer()
                        .xsmall()
                        .label("PR/MR unavailable")
                        .tooltip(format!(
                            "{error}\nOpen Git settings to connect your account."
                        ))
                        .on_click(|_, _, cx| {
                            cx.stop_propagation();
                            crate::ui::settings::open_git(cx);
                        })
                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                )
                .into_any_element()
            }
            None => row.child("Loading PR/MR…").into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Provider, request_labels};

    #[test]
    fn github_and_gitlab_have_distinct_request_labels() {
        assert_eq!(
            request_labels(Provider::GitHub),
            ("#", "Open pull request", "No pull request")
        );
        assert_eq!(
            request_labels(Provider::GitLab),
            ("!", "Open merge request", "No merge request")
        );
    }
}
