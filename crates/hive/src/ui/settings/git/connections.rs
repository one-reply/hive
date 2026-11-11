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
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use hive_core::git::hosting::{Connection, Provider};

const SERVER_WIDTH: Pixels = px(180.);
struct ServerInput {
    provider: Provider,
    input: Entity<InputState>,
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
                input.update(cx, |input, cx| input.set_value(server, window, cx));
                ServerInput { provider, input }
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
        let client_id = (provider == Provider::GitLab && server.trim() != "gitlab.com")
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
    fn provider(&self, row: &ServerInput, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let provider = row.provider;
        let account = self.hosting.read(cx).account(provider);
        let status = account.status.clone();
        let busy = status.busy();
        let label = action_label(&status);
        let message = match &status {
            Status::Checking => "Checking your connection…".into(),
            Status::Ready(Connection::MissingCli) => format!(
                "Install {} to connect. Homebrew is not required.",
                provider.cli()
            ),
            Status::Ready(Connection::SignedOut) => {
                "Sign in to see pull/merge requests and CI status.".into()
            }
            Status::Ready(Connection::Connected(user)) => {
                format!("Connected as {user} on {}", account.server.as_str())
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
                    .smooth_hover(ButtonLook::Primary, &action, cx),
            )
            .child(
                Button::new(("hosting-check", provider as usize))
                    .cursor_pointer()
                    .small()
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
            Status::Ready(Connection::Connected(_)) => cx.theme().success,
            _ => cx.theme().muted_foreground,
        };
        v_flex()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(provider.name()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .child("Server")
                    .child(
                        div().flex_1().min_w(SERVER_WIDTH).child(TextField::input(
                            Input::new(&row.input)
                                .disabled(busy)
                                .aria_label(format!("{} server", provider.name())),
                            &row.input,
                            cx,
                        )),
                    ),
            )
            .when(provider == Provider::GitLab && row.input.read(cx).value().trim() != "gitlab.com", |panel| {
                let help = Hover::new("gitlab-oauth-help", window, cx);
                panel.child(v_flex().gap_2()
                    .child(div().text_sm().child("OAuth Application ID (self-managed)"))
                    .child(TextField::input(Input::new(&self.client_id).disabled(busy).aria_label("GitLab OAuth Application ID"), &self.client_id, cx))
                    .child(Button::new("gitlab-oauth-help").cursor_pointer().small().label("Self-managed sign-in setup")
                        .on_click(|_, _, cx| cx.open_url("https://docs.gitlab.com/cli/authentication/#oauth-gitlab-self-managed-gitlab-dedicated"))
                        .smooth_hover(ButtonLook::Ghost, &help, cx)))
            })
            .child(div().text_sm().text_color(color).child(message))
            .child(controls)
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
                .child("Connect GitHub or GitLab to see pull/merge requests and CI. Existing CLI sign-ins are reused; credentials are managed by the official CLI."))
            .children(self.inputs.iter().map(|row| self.provider(row,window,cx)).collect::<Vec<_>>())
            .child(div().text_xs().text_color(cx.theme().muted_foreground)
                .child("These connections apply to this Mac. SSH projects use credentials on their remote host. Repository access still depends on your account permissions."))
    }
}

fn action_label(status: &Status) -> &'static str {
    match status {
        Status::Ready(Connection::MissingCli) => "Install & Connect",
        Status::Ready(Connection::Connected(_)) => "Sign in again",
        Status::Installing => "Preparing…",
        Status::SigningIn(_) => "Waiting for sign-in…",
        Status::Checking => "Checking…",
        _ => "Connect",
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, Status, action_label};
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
