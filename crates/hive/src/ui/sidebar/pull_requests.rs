use super::*;
use crate::state::pull_requests::Status;
use gpui_kit::component::Disableable as _;
pub(super) const HEIGHT: Pixels = px(24.);

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
        match status {
            Some(Status::Ready(Some(pr))) => {
                let label = format!("#{} · {}", pr.number, pr.state.to_lowercase());
                let url = pr.web_url().map(str::to_owned);
                let check_url = pr.failed_check_url().map(str::to_owned);
                row.child(
                    Button::new("open-pr")
                        .cursor_pointer()
                        .xsmall()
                        .label(label)
                        .tooltip("Open pull request")
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
                        .tooltip("Open the first failing check")
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
            Some(Status::Ready(None)) => row.child("No pull request").into_any_element(),
            Some(Status::Unavailable(error)) => row
                .child(
                    Button::new("pr-unavailable")
                        .cursor_pointer()
                        .xsmall()
                        .label("PR unavailable")
                        .tooltip(error)
                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                )
                .into_any_element(),
            None => row.child("Loading PR…").into_any_element(),
        }
    }
}
