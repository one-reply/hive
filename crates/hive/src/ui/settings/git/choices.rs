use crate::state::ai_providers::{AiProviders, KeyStatus};
use crate::ui::agent_logo;
use gpui_kit::{
    component::{ActiveTheme as _, Icon, h_flex, searchable_list::SearchableListItem},
    *,
};
use hive_core::{
    commit_message::{Generator, cli},
    llm::Provider,
};

#[derive(Clone, IntoElement)]
pub(super) struct SourceItem {
    value: Generator,
    status: &'static str,
    available: bool,
}

impl SourceItem {
    fn name(&self) -> &'static str {
        match self.value {
            Generator::Cli { agent, .. } => agent.name(),
            Generator::Api { provider, .. } => provider.name(),
        }
    }

    fn kind(&self) -> &'static str {
        match self.value {
            Generator::Cli { .. } => "CLI",
            Generator::Api { .. } => "API",
        }
    }

    fn logo(&self) -> &'static [u8] {
        match self.value {
            Generator::Cli { agent, .. } => agent_logo::svg(agent),
            Generator::Api { provider, .. } => super::super::ai::logo(provider),
        }
    }
}

impl SearchableListItem for SourceItem {
    type Value = Generator;
    fn title(&self) -> SharedString {
        format!("{} {}{}", self.name(), self.kind(), self.status).into()
    }
    fn display_title(&self) -> Option<AnyElement> {
        Some(self.clone().into_any_element())
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.clone()
    }
    fn value(&self) -> &Generator {
        &self.value
    }
    fn disabled(&self) -> bool {
        !self.available
    }
}

impl RenderOnce for SourceItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(Icon::default().data(self.logo()).size_4().flex_none())
            .child(div().min_w_0().truncate().child(self.name()))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}{}", self.kind(), self.status)),
            )
    }
}

pub(super) fn sources(providers: &AiProviders, selected: Option<&Generator>) -> Vec<SourceItem> {
    let mut items = Vec::new();
    for agent in cli::AGENTS {
        let available = providers.cli_agents.contains(&agent);
        let saved =
            selected.filter(|s| matches!(s, Generator::Cli { agent: a, .. } if *a == agent));
        if available || saved.is_some() {
            items.push(SourceItem {
                value: saved.cloned().unwrap_or(Generator::Cli {
                    agent,
                    arguments: String::new(),
                }),
                status: if available { "" } else { " · unavailable" },
                available,
            });
        }
    }
    for provider in Provider::ALL {
        let status = providers.provider(provider).status;
        let available = status == KeyStatus::Configured;
        let saved =
            selected.filter(|s| matches!(s, Generator::Api { provider: p, .. } if *p == provider));
        if available || saved.is_some() {
            items.push(SourceItem {
                value: saved.cloned().unwrap_or(Generator::Api {
                    provider,
                    model: None,
                }),
                status: if available {
                    ""
                } else if status.is_busy() {
                    " · checking"
                } else {
                    " · unavailable"
                },
                available,
            });
        }
    }
    items
}
