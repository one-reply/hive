//! Searchable provider catalogs keep custom and saved IDs available during discovery.
use super::*;
use gpui_kit::component::{
    IndexPath,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
enum Model {
    Default,
    Named(String),
    Custom,
}
impl SearchableListItem for Model {
    type Value = Self;
    fn title(&self) -> SharedString {
        match self {
            Self::Default => "CLI default".into(),
            Self::Named(s) => s.clone().into(),
            Self::Custom => "Custom model ID…".into(),
        }
    }
    fn value(&self) -> &Self {
        self
    }
}
type Picker = SelectState<SearchableVec<Model>>;
pub(super) struct ModelPicker {
    select: Entity<Picker>,
    catalog_ready: bool,
    _selection: Subscription,
}
impl ModelPicker {
    pub(super) fn new(
        agent: AgentKind,
        id: &str,
        t: &Template,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) -> Self {
        let (options, ready) = choices(agent, id, t, cx);
        let index = options
            .iter()
            .position(|option| matches!(option,Model::Named(value) if value==id))
            .unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(options),
                Some(IndexPath::new(index)),
                w,
                cx,
            )
            .searchable(true)
        });
        let selection = cx.subscribe_in(
            &select,
            w,
            |editor, _, event: &SelectEvent<SearchableVec<Model>>, w, cx| {
                let SelectEvent::Confirm(Some(model)) = event else {
                    return;
                };
                editor.inspector.custom_model = matches!(model, Model::Custom);
                match model {
                    Model::Named(id) => editor
                        .inspector
                        .model
                        .update(cx, |s, cx| s.set_value(id.clone(), w, cx)),
                    Model::Default => editor
                        .inspector
                        .model
                        .update(cx, |s, cx| s.set_value("", w, cx)),
                    Model::Custom => editor
                        .inspector
                        .model
                        .read(cx)
                        .focus_handle(cx)
                        .focus(w, cx),
                }
                cx.notify();
            },
        );
        Self {
            select,
            catalog_ready: ready,
            _selection: selection,
        }
    }
    pub(super) fn sync(
        &mut self,
        agent: AgentKind,
        id: &str,
        t: &Template,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) {
        let ready = AppState::get(cx)
            .pipelines
            .read(cx)
            .models
            .options
            .contains_key(&agent);
        if self.catalog_ready != ready {
            let (options, ready) = choices(agent, id, t, cx);
            self.select
                .update(cx, |s, cx| s.set_items(SearchableVec::new(options), w, cx));
            self.catalog_ready = ready;
        }
    }
    pub(super) fn render(&self) -> AnyElement {
        Select::new(&self.select)
            .small()
            .w_full()
            .accessibility_label("Model")
            .search_placeholder("Search models")
            .menu_width(rems(17.))
            .into_any_element()
    }
}
fn choices(agent: AgentKind, id: &str, t: &Template, cx: &App) -> (Vec<Model>, bool) {
    let app = AppState::get(cx);
    let catalog = app.pipelines.read(cx).models.options.get(&agent);
    let templates = app
        .store
        .read(cx)
        .state
        .pipelines
        .templates
        .iter()
        .chain(std::iter::once(t));
    let ids = catalog
        .and_then(|r| r.as_ref().ok())
        .map(Vec::as_slice)
        .unwrap_or_default();
    (model_choices(agent, id, templates, ids), catalog.is_some())
}
fn model_choices<'a>(
    agent: AgentKind,
    id: &str,
    templates: impl Iterator<Item = &'a Template>,
    catalog: &[String],
) -> Vec<Model> {
    let mut models = BTreeSet::new();
    for template in templates {
        for profile in [&template.planner.profile, &template.development.profile]
            .into_iter()
            .chain(template.lanes.iter().map(|l| &l.profile))
            .chain(template.reviews.iter().map(|r| &r.profile))
        {
            if profile.agent == agent && !profile.model.is_empty() {
                models.insert(profile.model.clone());
            }
        }
    }
    models.extend(catalog.iter().cloned());
    if !id.is_empty() {
        models.insert(id.into());
    }
    std::iter::once(Model::Default)
        .chain(models.into_iter().map(Model::Named))
        .chain(std::iter::once(Model::Custom))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{AgentKind, Model, Template, model_choices};

    #[test]
    fn model_picker_combines_saved_and_discovered_ids_for_only_the_chosen_cli() {
        let mut t = Template::default();
        t.development.profile.agent = AgentKind::Codex;
        t.development.profile.model = "remembered".into();
        t.planner.profile.agent = AgentKind::ClaudeCode;
        t.planner.profile.model = "other-provider".into();
        let catalog = ["installed".to_owned(), "remembered".to_owned()];
        let choices = model_choices(AgentKind::Codex, "manual", std::iter::once(&t), &catalog);
        assert_eq!(
            choices,
            vec![
                Model::Default,
                Model::Named("installed".into()),
                Model::Named("manual".into()),
                Model::Named("remembered".into()),
                Model::Custom
            ]
        );
        assert_eq!(
            model_choices(AgentKind::Codex, "", std::iter::empty(), &[]),
            vec![Model::Default, Model::Custom]
        );
    }
}
