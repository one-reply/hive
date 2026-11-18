use super::inspector::Inspector;
use super::map::{Layout, RoleKind};
use super::*;
use hive_core::pipeline::{LanePreset, Step};
mod nodes;

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
    parent: WeakEntity<PipelinePanel>,
    pub(super) template: Template,
    name: Entity<InputState>,
    pub(super) selected: Item,
    pub(super) inspector: Inspector,
    error: Option<String>,
    renaming: bool,
    compact: bool,
    show_inspector: bool,
    _models: Subscription,
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
        Self {
            original,
            parent,
            template,
            name,
            selected: Item::Planner,
            inspector,
            error: None,
            renaming: false,
            compact: false,
            show_inspector: false,
            _models: models,
        }
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
        let item = if matches!(kind, StageChoice::Lane) {
            let i = self.template.lanes.len();
            self.template.lanes.push(LanePreset {
                name: unique_name(
                    "Development lane",
                    self.template.lanes.iter().map(|l| l.name.as_str()),
                ),
                profile: self.template.development.profile.clone(),
                command: String::new(),
            });
            Item::Lane(i)
        } else {
            let i = self.template.reviews.len();
            let (name, prompt) = kind.recipe();
            self.template.reviews.push(Step {
                name: unique_name(name, self.template.reviews.iter().map(|s| s.name.as_str())),
                instruction: prompt.into(),
                profile: self.template.planner.profile.clone(),
                limit: 3,
                return_lane: None,
            });
            Item::Review(i)
        };
        self.load(item, w, cx);
        self.show_inspector = true;
        cx.notify();
    }
    fn remove(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Item::Lane(i) if self.template.lanes.len() > 1 => {
                let removed = self.template.lanes.remove(i);
                for s in &mut self.template.reviews {
                    if s.return_lane.as_deref() == Some(&removed.name) {
                        s.return_lane = None;
                    }
                }
            }
            Item::Review(i) if i > 0 => {
                self.template.reviews.remove(i);
            }
            _ => return,
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

#[derive(Clone, Copy)]
enum StageChoice {
    Lane,
    Code,
    Security,
    Performance,
    Custom,
}
impl StageChoice {
    fn recipe(self) -> (&'static str, &'static str) {
        match self {
            Self::Lane => ("Development lane", ""),
            Self::Code => (
                "Code review",
                "Review the implementation for correctness, maintainability and missing regression coverage. Report actionable findings and their affected lanes.",
            ),
            Self::Security => (
                "Security review",
                "Review the changed code for vulnerabilities, broken authorization and accidental exposure of sensitive data. Report the affected lane and a defensive fix.",
            ),
            Self::Performance => (
                "Performance review",
                "Inspect changed paths for performance regressions and unnecessary resource use. Report evidence and the affected lane.",
            ),
            Self::Custom => (
                "Custom review",
                "Review the changes against the approved acceptance criteria. Identify the affected lane for each finding.",
            ),
        }
    }
}
fn unique_name<'a>(base: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let names: HashSet<_> = existing.map(str::to_ascii_lowercase).collect();
    let available = |name: &str| !names.contains(&name.to_ascii_lowercase());
    if available(base) {
        return base.to_owned();
    }
    let mut n = 2;
    loop {
        let name = format!("{base} {n}");
        if available(&name) {
            return name;
        }
        n += 1;
    }
}

fn move_item(t: &mut Template, from: Item, to: Item) -> bool {
    match (from, to) {
        (Item::Lane(a), Item::Lane(b)) => reorder(&mut t.lanes, a, b),
        (Item::Review(a), Item::Review(b)) if a > 0 && b > 0 => reorder(&mut t.reviews, a, b),
        _ => false,
    }
}
fn reorder<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}
impl TemplateEditor {
    fn add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = [
            StageChoice::Lane,
            StageChoice::Code,
            StageChoice::Security,
            StageChoice::Performance,
            StageChoice::Custom,
        ]
        .map(|stage| (stage.recipe().0.to_owned(), false, stage))
        .into();
        options_menu(
            Button::new("add-stage")
                .small()
                .cursor_pointer()
                .icon(Lucide::Plus)
                .label("Add stage"),
            options,
            cx,
            |s, stage, w, cx| s.add(stage, w, cx),
        )
    }
    fn ordering(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (min, count, index, make): (usize, usize, usize, fn(usize) -> Item) =
            match self.selected {
                Item::Lane(i) if self.template.lanes.len() > 1 => {
                    (0, self.template.lanes.len(), i, Item::Lane)
                }
                Item::Review(i) if i > 0 => (1, self.template.reviews.len(), i, Item::Review),
                _ => return None,
            };
        let mut options = Vec::new();
        if index > min {
            options.push(("Move earlier".to_owned(), false, Some(make(index - 1))));
        }
        if index + 1 < count {
            options.push(("Move later".to_owned(), false, Some(make(index + 1))));
        }
        options.push(("Remove stage".to_owned(), false, None));
        Some(options_menu(
            Button::new("selected-stage-actions")
                .small()
                .cursor_pointer()
                .label("Stage actions"),
            options,
            cx,
            move |s, dest, w, cx| match dest {
                Some(to) => s.move_to(make(index), to, w, cx),
                None => s.remove(w, cx),
            },
        ))
    }
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
        bar = bar.child(self.add_menu(cx)).child(button(
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
            let fields = self
                .inspector_fields(window, cx)
                .children(self.ordering(cx));
            body = body.child(
                div()
                    .h_full()
                    .min_w_0()
                    .when(self.compact, |el| el.flex_1())
                    .when(!self.compact, |el| {
                        el.w(rems(17.))
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
    use super::{Item, StageChoice, Template, move_item, reorder, unique_name};
    #[test]
    fn new_drafts_and_stages_have_distinct_runnable_defaults() {
        assert_eq!(
            unique_name(
                "Custom workflow",
                ["custom workflow", "Custom workflow 3"].into_iter()
            ),
            "Custom workflow 2"
        );
        for role in [
            StageChoice::Code,
            StageChoice::Security,
            StageChoice::Performance,
            StageChoice::Custom,
        ] {
            let (name, instruction) = role.recipe();
            assert!(!name.is_empty() && !instruction.is_empty());
        }
    }
    #[test]
    fn cannot_drag_the_qa_barrier_or_move_lanes_into_review() {
        let mut t = Template::default();
        assert!(!move_item(&mut t, Item::Review(1), Item::Review(0)));
        assert!(!move_item(&mut t, Item::Lane(0), Item::Review(1)));
        t.reviews.push(t.reviews[1].clone());
        t.reviews[2].name = "Code review".into();
        assert!(move_item(&mut t, Item::Review(2), Item::Review(1)));
        assert_eq!(t.reviews[0].name, "QA");
        assert_eq!(t.reviews[1].name, "Code review");
        let mut items = vec!["api", "web", "mobile"];
        assert!(reorder(&mut items, 0, 2));
        assert_eq!(items, ["web", "mobile", "api"]);
    }
}
