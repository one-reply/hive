//! Provider connection controls; all process, download and authentication work belongs to state.
use crate::{
    app::AppState,
    state::git_hosting::{GitHosting, Status},
    ui::{
        hover::{ButtonLook, Hover, SmoothButton as _},
        text_field::TextField,
    },
};
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use hive_core::git::hosting::{Connection, Provider};

const LOGO_SIZE: Pixels = px(24.);
struct ServerInput {
    provider: Provider,
    input: Entity<InputState>,
    details_open: bool,
}
pub(super) struct Connections {
    hosting: Entity<GitHosting>,
    inputs: Vec<ServerInput>,
    client_id: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}
impl Connections {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hosting = AppState::get(cx).git_hosting.clone();
        let inputs: Vec<ServerInput> = Provider::ALL
            .into_iter()
            .map(|provider| {
                let server = hosting
                    .read(cx)
                    .account(provider)
                    .server
                    .as_str()
                    .to_owned();
                let input =
                    cx.new(|cx| InputState::new(window, cx).placeholder(provider.default_host()));
                input.update(cx, |input, cx| input.set_value(server.clone(), window, cx));
                ServerInput {
                    provider,
                    input,
                    details_open: is_custom_server(provider, &server),
                }
            })
            .collect();
        let mut subscriptions = vec![cx.observe(&hosting, |_, _, cx| cx.notify())];
        for row in &inputs {
            subscriptions.push(cx.subscribe(&row.input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        let client_id =
            cx.new(|cx| InputState::new(window, cx).placeholder("Use existing glab configuration"));
        hosting.update(cx, |hosting, cx| hosting.refresh(cx));
        Self {
            hosting,
            inputs,
            client_id,
            _subscriptions: subscriptions,
        }
    }
    fn connect(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let Some(row) = self.inputs.iter().find(|row| row.provider == provider) else {
            return;
        };
        let server = row.input.read(cx).value().to_string();
        let client_id = (provider == Provider::GitLab && is_custom_server(provider, &server))
            .then(|| self.client_id.read(cx).value().trim().to_owned())
            .filter(|id| !id.is_empty());
        self.hosting.update(cx, |hosting, cx| {
            hosting.connect(provider, &server, client_id, cx)
        });
    }
    fn check(&mut self, provider: Provider, cx: &mut Context<Self>) {
        let Some(row) = self.inputs.iter().find(|row| row.provider == provider) else {
            return;
        };
        let server = row.input.read(cx).value().to_string();
        self.hosting
            .update(cx, |hosting, cx| hosting.check(provider, &server, cx));
    }
    fn toggle_details(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if let Some(row) = self.inputs.iter_mut().find(|row| row.provider == provider) {
            row.details_open = !row.details_open;
            cx.notify();
        }
    }
    fn server_fields(
        &self,
        row: &ServerInput,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut fields = v_flex()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(div().text_sm().child("Server"))
            .child(TextField::input(
                Input::new(&row.input)
                    .disabled(busy)
                    .aria_label(format!("{} server", row.provider.name())),
                &row.input,
                cx,
            ));
        if row.provider == Provider::GitLab
            && is_custom_server(row.provider, row.input.read(cx).value().as_ref())
        {
            let help = Hover::new("gitlab-oauth-help", window, cx);
            fields = fields
                .child(div().pt_2().text_sm().child("OAuth Application ID"))
                .child(TextField::input(Input::new(&self.client_id).disabled(busy)
                    .aria_label("GitLab OAuth Application ID"), &self.client_id, cx))
                .child(div().text_xs().text_color(cx.theme().muted_foreground)
                    .child("Self-managed GitLab requires an OAuth application for browser sign-in."))
                .child(Button::new("gitlab-oauth-help").cursor_pointer().small()
                    .icon(Lucide::ExternalLink).label("Self-managed sign-in setup")
                    .on_click(|_, _, cx| cx.open_url("https://docs.gitlab.com/cli/authentication/#oauth-gitlab-self-managed-gitlab-dedicated"))
                    .smooth_hover(ButtonLook::Ghost, &help, cx));
        }
        fields
    }
    fn provider(&self, row: &ServerInput, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let provider = row.provider;
        let account = self.hosting.read(cx).account(provider);
        let status = visible_status(
            &account.status,
            account.server.as_str(),
            row.input.read(cx).value().as_ref(),
        );
        let busy = status.busy();
        let label = action_label(&status);
        let connected = matches!(
            &status,
            Status::Ready(Connection::Connected(_) | Connection::ConnectedWithWarning(_))
        );
        let message = match &status {
            Status::Checking => "Checking your connection…".into(),
            Status::Ready(Connection::MissingCli) => format!(
                "Install {} to connect. Homebrew is not required.",
                provider.cli()
            ),
            Status::Ready(Connection::SignedOut) => {
                "Sign in to see pull/merge requests and CI status.".into()
            }
            Status::Ready(Connection::Connected(user) | Connection::ConnectedWithWarning(user)) => {
                format!("Connected as {user} · {}", account.server.as_str())
            }
            Status::Installing => {
                "Preparing the official CLI. Downloads are verified before installation.".into()
            }
            Status::SigningIn(_) => "Finish signing in and approve access in your browser.".into(),
            Status::Failed(error) => error.clone(),
        };
        let action = Hover::new(("hosting-connect", provider as usize), window, cx).enabled(!busy);
        let refresh = Hover::new(("hosting-check", provider as usize), window, cx).enabled(!busy);
        let cancel = Hover::new(("hosting-cancel", provider as usize), window, cx);
        let mut controls = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new(("hosting-connect", provider as usize))
                    .cursor_pointer()
                    .small()
                    .label(label)
                    .tooltip(format!("Connect {} in your browser", provider.name()))
                    .loading(matches!(&status, Status::Checking | Status::Installing))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.connect(provider, cx)))
                    .smooth_hover(
                        if connected {
                            ButtonLook::Ghost
                        } else {
                            ButtonLook::Primary
                        },
                        &action,
                        cx,
                    ),
            )
            .child(
                Button::new(("hosting-check", provider as usize))
                    .cursor_pointer()
                    .small()
                    .icon(Lucide::RefreshCw)
                    .label("Check connection")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| this.check(provider, cx)))
                    .smooth_hover(ButtonLook::Ghost, &refresh, cx),
            );
        if busy {
            controls = controls.child(
                Button::new(("hosting-cancel", provider as usize))
                    .cursor_pointer()
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.hosting
                            .update(cx, |hosting, cx| hosting.cancel(provider, cx))
                    }))
                    .smooth_hover(ButtonLook::Ghost, &cancel, cx),
            );
        }
        let color = match &status {
            Status::Failed(_) => cx.theme().danger,
            Status::Ready(Connection::Connected(_) | Connection::ConnectedWithWarning(_)) => {
                cx.theme().success
            }
            _ => cx.theme().muted_foreground,
        };
        let details = Hover::new(("hosting-details", provider as usize), window, cx).enabled(!busy);
        let logo: &'static [u8] = match provider {
            Provider::GitHub => include_bytes!("../../../../assets/github.svg"),
            Provider::GitLab => include_bytes!("../../../../assets/gitlab.svg"),
        };
        v_flex()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(Icon::default().data(logo).size(LOGO_SIZE).flex_shrink_0())
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(provider.name()),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_start()
                                    .when(connected, |line| line.items_center())
                                    .text_sm()
                                    .text_color(color)
                                    .when(connected, |line| {
                                        line.child(
                                            Icon::new(Lucide::CircleCheck).size_4().flex_shrink_0(),
                                        )
                                    })
                                    .when(matches!(&status, Status::Failed(_)), |line| {
                                        line.child(
                                            Icon::new(Lucide::CircleAlert).size_4().flex_shrink_0(),
                                        )
                                    })
                                    .child(div().min_w_0().child(message)),
                            ),
                    ),
            )
            .when(
                matches!(&status, Status::Ready(Connection::ConnectedWithWarning(_))),
                |panel| {
                    panel.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(hive_core::git::hosting::STORAGE_WARNING),
                    )
                },
            )
            .when(row.details_open, |panel| {
                panel.child(self.server_fields(row, busy, window, cx))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .flex_wrap()
                    .child(
                        Button::new(("hosting-details", provider as usize))
                            .cursor_pointer()
                            .small()
                            .icon(if row.details_open {
                                Lucide::ChevronDown
                            } else {
                                Lucide::ChevronRight
                            })
                            .label("Server settings")
                            .disabled(busy)
                            .tooltip(format!("{} server settings", provider.name()))
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.toggle_details(provider, cx)
                                }),
                            )
                            .smooth_hover(ButtonLook::Ghost, &details, cx),
                    )
                    .child(controls),
            )
            .when_some(
                match status {
                    Status::SigningIn(prompt) => prompt,
                    _ => None,
                },
                |panel, prompt| {
                    let hover = Hover::new(("hosting-code", provider as usize), window, cx);
                    panel.child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("One-time code: {}", prompt.code)),
                            )
                            .child(
                                Button::new(("hosting-code", provider as usize))
                                    .cursor_pointer()
                                    .small()
                                    .icon(Lucide::ExternalLink)
                                    .label("Copy code & Open browser")
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            prompt.code.clone(),
                                        ));
                                        cx.open_url(&prompt.url);
                                    })
                                    .smooth_hover(ButtonLook::Primary, &hover, cx),
                            ),
                    )
                },
            )
    }
}
impl Render for Connections {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_4()
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Hosting accounts"))
            .child(div().text_sm().text_color(cx.theme().muted_foreground)
                .child("Connect your accounts to see pull requests, merge requests and CI status."))
            .children(self.inputs.iter().map(|row| self.provider(row,window,cx)).collect::<Vec<_>>())
            .child(div().text_xs().text_color(cx.theme().muted_foreground)
                .child("Existing CLI sign-ins are reused; credentials stay with the official CLI. These connections apply to this Mac. SSH projects use credentials on their remote host. Repository access still depends on your account permissions."))
    }
}

fn is_custom_server(provider: Provider, input: &str) -> bool {
    !input.trim().eq_ignore_ascii_case(provider.default_host())
}

fn visible_status(status: &Status, server: &str, input: &str) -> Status {
    if matches!(
        status,
        Status::Ready(Connection::Connected(_) | Connection::ConnectedWithWarning(_))
    ) && !input.trim().eq_ignore_ascii_case(server)
    {
        Status::Ready(Connection::SignedOut)
    } else {
        status.clone()
    }
}

fn action_label(status: &Status) -> &'static str {
    match status {
        Status::Ready(Connection::MissingCli) => "Install & Connect",
        Status::Ready(Connection::Connected(_) | Connection::ConnectedWithWarning(_)) => {
            "Sign in again"
        }
        Status::Installing => "Preparing…",
        Status::SigningIn(_) => "Waiting for sign-in…",
        Status::Checking => "Checking…",
        _ => "Connect",
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Provider, Status, action_label, is_custom_server, visible_status};
    #[test]
    fn public_servers_hide_details_and_custom_servers_keep_them_discoverable() {
        assert!(!is_custom_server(Provider::GitLab, " GitLab.COM "));
        assert!(is_custom_server(Provider::GitLab, "code.example"));
        assert!(!is_custom_server(Provider::GitHub, "github.com"));
    }
    #[test]
    fn editing_server_does_not_claim_the_new_host_is_connected() {
        let connected = Status::Ready(Connection::Connected("person".into()));
        assert_eq!(
            visible_status(&connected, "gitlab.com", " GitLab.COM "),
            connected
        );
        assert_eq!(
            visible_status(&connected, "gitlab.com", "code.example"),
            Status::Ready(Connection::SignedOut)
        );
        let failed = Status::Failed("check hostname".into());
        assert_eq!(visible_status(&failed, "gitlab.com", "bad/host"), failed);
    }
    #[test]
    fn missing_cli_offers_install_and_failed_auth_never_looks_connected() {
        assert_eq!(
            action_label(&Status::Ready(Connection::MissingCli)),
            "Install & Connect"
        );
        assert_eq!(action_label(&Status::Failed("network".into())), "Connect");
        assert_eq!(
            action_label(&Status::Ready(Connection::SignedOut)),
            "Connect"
        );
        assert_eq!(
            action_label(&Status::Ready(Connection::Connected("user".into()))),
            "Sign in again"
        );
        for status in [
            Status::Installing,
            Status::Checking,
            Status::SigningIn(None),
        ] {
            assert!(status.busy());
        }
    }
}
