use super::*;
use crate::state::pull_requests::{Status, can_connect};
use gpui_kit::component::Disableable as _;
use hive_core::git::pull_request::{Provider, PullRequest};
pub(super) const HEIGHT: Pixels = px(24.);
/// gpui-component's disabled button text: the muted color at this strength.
const DISABLED_TEXT_OPACITY: f32 = 0.5;

/// The request number's prefix, its link's tooltip and the text shown without a request.
struct RequestLabels {
    prefix: &'static str,
    tooltip: &'static str,
    empty: &'static str,
}

fn request_labels(provider: Provider) -> RequestLabels {
    let (prefix, tooltip, empty) = match provider {
        Provider::GitHub => ("#", "Open pull request", "No pull request"),
        Provider::GitLab => ("!", "Open merge request", "No merge request"),
    };
    RequestLabels {
        prefix,
        tooltip,
        empty,
    }
}

impl Sidebar {
    /// The PR/MR row of a worktree card. Its controls act only on the `selected` card: on
    /// another card a click anywhere selects that card first.
    pub(super) fn pr_card(
        &self,
        project: &Project,
        worktree: &Worktree,
        selected: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let status = AppState::get(cx)
            .pull_requests
            .read(cx)
            .status(&project.host, &worktree.path, &worktree.head)
            .cloned();
        let can_connect = status
            .as_ref()
            .and_then(|status| can_connect(&project.host, status))
            .is_some();
        match status {
            Some(Status::Ready(snapshot)) => {
                let labels = request_labels(snapshot.provider());
                match snapshot.request() {
                    Some(pr) => request_row(pr, &labels, &worktree.path, selected, window, cx)
                        .into_any_element(),
                    None => row(cx).child(labels.empty).into_any_element(),
                }
            }
            Some(Status::Unavailable { message: error, .. }) => {
                if !(can_connect && selected) {
                    return row(cx)
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
                let hover = request_hover(&worktree.path, window, cx);
                row(cx)
                    .child(
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
            None => row(cx).child("Loading PR/MR…").into_any_element(),
        }
    }
}

fn row(cx: &App) -> Div {
    h_flex()
        .h(HEIGHT)
        .gap_1()
        .items_center()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
}

fn request_hover(worktree: &Path, window: &mut Window, cx: &mut App) -> Hover {
    Hover::new(
        SharedString::from(format!("pr:{}", worktree.display())),
        window,
        cx,
    )
}

/// A request's number and checks. On the selected card they open the request and its
/// failing CI run; on another card they are text, so a click on them selects the card as a
/// click anywhere else on it does. A button there would take the click for itself, and a
/// disabled one would swallow it.
fn request_row(
    pr: &PullRequest,
    labels: &RequestLabels,
    worktree: &Path,
    selected: bool,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let label = format!(
        "{}{} · {}",
        labels.prefix,
        pr.number,
        pr.state.to_lowercase()
    );
    let url = pr.web_url().map(str::to_owned);
    let check_url = pr.failed_check_url().map(str::to_owned);
    if !selected {
        let nothing_to_open = cx.theme().muted_foreground.opacity(DISABLED_TEXT_OPACITY);
        return row(cx)
            .child(passive_label(label).debug_selector(|| "pr-request".into()))
            .child(
                passive_label(pr.checks())
                    // Dimmed like the disabled button it becomes on the selected card, so
                    // selection does not swap which of the two labels stands out.
                    .when(check_url.is_none(), |label| {
                        label.text_color(nothing_to_open)
                    })
                    .debug_selector(|| "pr-checks".into()),
            );
    }
    let hover = request_hover(worktree, window, cx);
    let check_hover = Hover::new(
        SharedString::from(format!("ci:{}", worktree.display())),
        window,
        cx,
    );
    row(cx)
        .child(
            Button::new("open-pr")
                .cursor_pointer()
                .xsmall()
                .label(label)
                .tooltip(labels.tooltip)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    if let Some(url) = &url {
                        cx.open_url(url);
                    }
                })
                .smooth_hover(ButtonLook::Ghost, &hover, cx)
                .debug_selector(|| "pr-request".into()),
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
                .smooth_hover(ButtonLook::Ghost, &check_hover, cx)
                .debug_selector(|| "pr-checks".into()),
        )
}

/// Text in the box of an extra-small button, so it stays in place when the card is
/// selected and the text becomes a button.
fn passive_label(text: impl Into<SharedString>) -> Div {
    div().h_5().px_1().flex().items_center().child(text.into())
}

#[cfg(test)]
mod tests {
    use super::{Provider, PullRequest, request_labels, request_row};
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, Modifiers, Render, TestAppContext, Window, div, px};
    use std::path::Path;

    const REQUEST_URL: &str = "https://github.com/one-reply/hive/pull/121";
    const CHECK_URL: &str = "https://github.com/one-reply/hive/actions/runs/7";

    #[test]
    fn github_and_gitlab_have_distinct_request_labels() {
        for (provider, expected) in [
            (
                Provider::GitHub,
                ("#", "Open pull request", "No pull request"),
            ),
            (
                Provider::GitLab,
                ("!", "Open merge request", "No merge request"),
            ),
        ] {
            let labels = request_labels(provider);
            assert_eq!((labels.prefix, labels.tooltip, labels.empty), expected);
        }
    }

    /// A worktree card that, like the sidebar's, selects itself on a click.
    struct Card {
        request: PullRequest,
        selected: bool,
        selections: usize,
    }

    impl Render for Card {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let labels = request_labels(Provider::GitHub);
            let row = request_row(
                &self.request,
                &labels,
                Path::new("/project"),
                self.selected,
                window,
                cx,
            );
            div()
                .id("card")
                .w(px(300.))
                .h(px(100.))
                .child(row)
                .on_click(cx.listener(|card, _, _, cx| {
                    card.selections += 1;
                    card.selected = true;
                    cx.notify();
                }))
        }
    }

    #[gpui::test]
    fn request_links_open_only_once_their_card_is_selected(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let request: PullRequest = serde_json::from_value(serde_json::json!({
            "number": 121,
            "url": REQUEST_URL,
            "state": "MERGED",
            "statusCheckRollup": [{ "conclusion": "FAILURE", "detailsUrl": CHECK_URL }],
        }))
        .unwrap();
        let (card, cx) = cx.add_window_view(|_, _| Card {
            request,
            selected: false,
            selections: 0,
        });
        for (selector, url) in [("pr-request", REQUEST_URL), ("pr-checks", CHECK_URL)] {
            card.update(cx, |card, cx| {
                card.selected = false;
                card.selections = 0;
                cx.notify();
            });
            // The first click only selects the card; the second opens the link.
            for (selections, opened) in [(1, cx.opened_url()), (1, Some(url.to_owned()))] {
                cx.update(|window, cx| {
                    window.activate_window();
                    window.draw(cx).clear(cx);
                });
                let bounds = cx.debug_bounds(selector).unwrap();
                cx.simulate_click(bounds.center(), Modifiers::default());
                cx.run_until_parked();
                cx.read(|cx| {
                    assert_eq!(card.read(cx).selections, selections, "{selector}");
                    assert!(card.read(cx).selected, "{selector}");
                });
                assert_eq!(cx.opened_url(), opened, "{selector}");
            }
        }
    }
}
