use super::inspector::Inspector;
use super::map::{Layout, RoleKind};
use super::*;
use hive_core::pipeline::LanePreset;
use hive_core::pipeline::unique_name;
mod nodes;
mod structure;
use structure::{StageChoice, move_item};

/// Inspector width beside the canvas, in rems: wide enough for a mode's explanation to
/// wrap in a few lines.
const INSPECTOR_WIDTH: f32 = 20.;

#[derive(Copy, Clone, PartialEq, Eq)]
pub(super) enum Item {
    Planner,
    Development,
    Lane(usize),
    Review(usize),
}

#[derive(Clone)]
struct Dragged {
    editor: EntityId,
    item: Item,
    label: String,
}
impl Render for Dragged {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        card(cx).child(self.label.clone())
    }
}

pub(super) struct TemplateEditor {
    original: Option<Template>,
    /// The draft as the editor first showed it: what leaving without saving goes back to.
    pub(super) opened: Template,
    parent: WeakEntity<PipelinePanel>,
    pub(super) template: Template,
    name: Entity<InputState>,
    pub(super) selected: Item,
    pub(super) inspector: Inspector,
    error: Option<String>,
    renaming: bool,
    compact: bool,
    show_inspector: bool,
    _subscriptions: [Subscription; 2],
}

impl TemplateEditor {
    pub(super) fn new(
        original: Option<Template>,
        parent: WeakEntity<PipelinePanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut template = original.clone().unwrap_or_default();
        if original.is_none() {
            template.name = unique_name(
                "Custom workflow",
                AppState::get(cx)
                    .store
                    .read(cx)
                    .state
                    .pipelines
                    .templates
                    .iter()
                    .map(|t| t.name.as_str()),
            );
        }
        // A prompt edited or deleted since the recipe was saved must not show stale text.
        template.sync_prompts(AppState::get(cx).store.read(cx).state.pipelines.prompts());
        let name = input(&template.name, "Template name", window, cx);
        let inspector = Inspector::new(Item::Planner, &template, window, cx);
        let models = cx.observe_in(
            &AppState::get(cx).pipelines.clone(),
            window,
            |s, _, w, cx| {
                s.sync_models(w, cx);
                cx.notify()
            },
        );
        let prompts = cx.observe_in(&AppState::get(cx).store.clone(), window, |s, _, w, cx| {
            s.sync_prompts(w, cx);
            cx.notify()
        });
        let mut editor = Self {
            original,
            opened: template.clone(),
            parent,
            template,
            name,
            selected: Item::Planner,
            inspector,
            error: None,
            renaming: false,
            compact: false,
            show_inspector: false,
            _subscriptions: [models, prompts],
        };
        // Read back through the same fields as every later comparison, so a field that
        // tidies its text does not count as an edit.
        if let Ok(opened) = editor.draft(cx) {
            editor.opened = opened;
        }
        editor
    }
    /// The recipe as it would be saved now, with the fields still in the inspector.
    fn draft(&self, cx: &App) -> Result<Template, String> {
        let mut draft = self.template.clone();
        self.inspector.apply(self.selected, &mut draft, cx)?;
        draft.name = self.name.read(cx).value().trim().to_owned();
        Ok(draft)
    }
    /// Whether leaving the editor would drop something the user changed.
    pub(super) fn is_modified(&self, cx: &App) -> bool {
        differs_from(&self.opened, self.draft(cx))
    }
    fn apply(&mut self, cx: &App) -> bool {
        match self.inspector.apply(self.selected, &mut self.template, cx) {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }
    fn select(&mut self, item: Item, w: &mut Window, cx: &mut Context<Self>) {
        self.show_inspector = true;
        if item != self.selected && self.apply(cx) {
            self.load(item, w, cx);
        }
        cx.notify();
    }
    fn load(&mut self, item: Item, w: &mut Window, cx: &mut Context<Self>) {
        self.inspector = Inspector::new(item, &self.template, w, cx);
        self.selected = item;
    }
    fn move_to(&mut self, from: Item, to: Item, w: &mut Window, cx: &mut Context<Self>) {
        if self.apply(cx) && move_item(&mut self.template, from, to) {
            self.load(to, w, cx);
        }
        cx.notify();
    }
    fn add(&mut self, kind: StageChoice, w: &mut Window, cx: &mut Context<Self>) {
        if !self.apply(cx) {
            cx.notify();
            return;
        }
        let item = match kind {
            StageChoice::Lane => {
                let i = self.template.lanes.len();
                self.template.lanes.push(LanePreset {
                    name: unique_name(
                        kind.label(),
                        self.template.lanes.iter().map(|l| l.name.as_str()),
                    ),
                    profile: self.template.development.profile.clone(),
                    ..Default::default()
                });
                Item::Lane(i)
            }
            StageChoice::Review(review) => {
                let i = self.template.reviews.len();
                let library = &AppState::get(cx).store.read(cx).state.pipelines;
                let mut step = library.review_step(review);
                step.name = unique_name(
                    &step.name,
                    self.template.reviews.iter().map(|s| s.name.as_str()),
                );
                step.profile = self.template.planner.profile.clone();
                self.template.reviews.push(step);
                Item::Review(i)
            }
        };
        self.load(item, w, cx);
        self.show_inspector = true;
        cx.notify();
    }
    fn remove(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Item::Lane(i) => {
                if self.template.remove_lane(i).is_err() {
                    return;
                }
            }
            Item::Review(i) => {
                if self.template.remove_review(i).is_err() {
                    return;
                }
            }
            Item::Planner | Item::Development => return,
        }
        self.error = None;
        self.load(Item::Development, w, cx);
        cx.notify();
    }
    fn save(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.apply(cx) {
            cx.notify();
            return;
        }
        self.template.name = self.name.read(cx).value().trim().to_owned();
        let result = AppState::get(cx)
            .pipelines
            .clone()
            .update(cx, |pipelines, cx| {
                pipelines.save_template(self.original.as_ref(), self.template.clone(), cx)
            });
        match result {
            Ok(()) => {
                let _ = self.parent.update(cx, |p, cx| {
                    p.selected_template = Some(self.template.name.clone());
                    p.home(w, cx)
                });
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
    pub(super) fn role(&self, item: Item) -> RoleKind {
        match item {
            Item::Planner => RoleKind::Plan,
            Item::Development | Item::Lane(_) => RoleKind::Build,
            Item::Review(i) => RoleKind::review(i, &self.template.reviews[i]),
        }
    }
}

/// A draft the inspector cannot apply still holds what the user typed, so it is an edit too.
fn differs_from(opened: &Template, draft: Result<Template, String>) -> bool {
    draft.map_or(true, |draft| draft != *opened)
}

impl Render for TemplateEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = if self.renaming {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().flex_1().child(TextField::input(
                    Input::new(&self.name),
                    &self.name,
                    cx,
                )))
                .child(button(
                    "finish-template-name",
                    "Done",
                    ButtonLook::Ghost,
                    window,
                    cx,
                    |s, _, cx| {
                        s.renaming = false;
                        cx.notify()
                    },
                ))
        } else {
            let name = self.name.read(cx).value();
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_base()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(icon_action(
                    "rename-template",
                    "Rename template",
                    Lucide::Pencil,
                    window,
                    cx,
                    |s: &mut Self, w, cx| {
                        s.renaming = true;
                        s.name.read(cx).focus_handle(cx).focus(w, cx);
                        cx.notify()
                    },
                ))
        };
        let mut bar = h_flex()
            .w_full()
            .flex_none()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(title);
        if self.compact {
            bar = bar.child(icon_action(
                "toggle-stage-inspector",
                if self.show_inspector {
                    "Show canvas"
                } else {
                    "Show stage details"
                },
                if self.show_inspector {
                    Lucide::Workflow
                } else {
                    Lucide::SlidersHorizontal
                },
                window,
                cx,
                |s: &mut Self, _, cx| {
                    s.show_inspector = !s.show_inspector;
                    cx.notify()
                },
            ));
        }
        bar = bar
            .child(button(
                "shared-contract",
                if self.template.shared_contract {
                    "Shared contract: on"
                } else {
                    "Shared contract: off"
                },
                ButtonLook::Outline,
                window,
                cx,
                |v, _, cx| {
                    v.template.shared_contract = !v.template.shared_contract;
                    cx.notify();
                },
            ))
            .child(self.add_menu(cx))
            .child(button(
                "save-template",
                "Save",
                ButtonLook::Primary,
                window,
                cx,
                |s, w, cx| s.save(w, cx),
            ));
        let mut body = h_flex().flex_1().min_h_0().items_stretch();
        if !self.compact || !self.show_inspector {
            body = body.child(self.canvas(window, cx));
        }
        if !self.compact || self.show_inspector {
            let fields = self.inspector_fields(window, cx);
            body = body.child(
                div()
                    .h_full()
                    .min_w_0()
                    .when(self.compact, |el| el.flex_1())
                    .when(!self.compact, |el| {
                        el.w(rems(INSPECTOR_WIDTH))
                            .flex_none()
                            .border_l_1()
                            .border_color(cx.theme().border)
                    })
                    .child(scroll("pipeline-inspector", fields)),
            );
        }
        let compact = self.compact;
        let split_width = map::units(600.).to_pixels(window.rem_size());
        let view = cx.weak_entity();
        v_flex()
            .size_full()
            .min_h_0()
            .child(bar)
            .on_children_prepainted(move |bounds, _, cx| {
                if let Some(first) = bounds.first() {
                    let narrow = first.size.width < split_width;
                    if narrow != compact {
                        let view = view.clone();
                        cx.defer(move |cx| {
                            let _ = view.update(cx, |s, cx| {
                                if s.compact != narrow {
                                    s.compact = narrow;
                                    cx.notify();
                                }
                            });
                        });
                    }
                }
            })
            .when_some(self.error.clone(), |el, e| {
                el.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(e),
                )
            })
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::{Template, differs_from, unique_name};

    #[test]
    fn only_a_changed_or_unreadable_draft_counts_as_unsaved() {
        let opened = Template::default();
        assert!(!differs_from(&opened, Ok(opened.clone())));
        let mut renamed = opened.clone();
        renamed.name.push_str(" 2");
        assert!(differs_from(&opened, Ok(renamed)));
        assert!(differs_from(
            &opened,
            Err("Enter a name for this step or lane.".into())
        ));
    }

    #[test]
    fn new_drafts_get_a_free_numbered_name() {
        assert_eq!(
            unique_name(
                "Custom workflow",
                ["custom workflow", "Custom workflow 3"].into_iter()
            ),
            "Custom workflow 2"
        );
    }
}
