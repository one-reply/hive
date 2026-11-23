//! A stage's instructions: its own text, or a prompt from the shared library that it follows.
use super::*;
use gpui_kit::component::{
    IndexPath,
    scroll::ScrollableElement as _,
    searchable_list::{SearchableListItem, SearchableVec},
    select::{Select, SelectEvent, SelectState},
};
use hive_core::pipeline::Prompt;

/// Picker entry for instructions that belong to the stage alone.
const CUSTOM: &str = "Custom instructions";
/// Lines of instructions the inspector shows; longer text scrolls inside its box.
const PREVIEW_LINES: f32 = 10.;
/// Set on the preview so its height limit is a whole number of lines.
const PREVIEW_LINE_HEIGHT: Rems = rems(1.4);

/// One picker entry: a library prompt, or `None` for the stage's own text.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Choice {
    id: Option<u64>,
    title: SharedString,
}
impl SearchableListItem for Choice {
    type Value = Option<u64>;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &Option<u64> {
        &self.id
    }
}

/// The stage's own text first, then the library in its order.
fn prompt_choices(prompts: &[Prompt]) -> Vec<Choice> {
    let custom = Choice {
        id: None,
        title: CUSTOM.into(),
    };
    let library = prompts.iter().map(|prompt| Choice {
        id: Some(prompt.id),
        title: prompt.name.clone().into(),
    });
    std::iter::once(custom).chain(library).collect()
}

type Picker = SelectState<SearchableVec<Choice>>;
pub(super) struct PromptPicker {
    select: Entity<Picker>,
    choices: Vec<Choice>,
    _selection: Subscription,
}
impl PromptPicker {
    pub(super) fn new(
        linked: Option<u64>,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) -> Self {
        let choices = prompt_choices(AppState::get(cx).store.read(cx).state.pipelines.prompts());
        let index = choices.iter().position(|c| c.id == linked).unwrap_or(0);
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(choices.clone()),
                Some(IndexPath::new(index)),
                w,
                cx,
            )
            .searchable(true)
        });
        let selection = cx.subscribe_in(
            &select,
            w,
            |editor, _, event: &SelectEvent<SearchableVec<Choice>>, w, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    editor.follow_prompt(*choice, w, cx);
                }
            },
        );
        Self {
            select,
            choices,
            _selection: selection,
        }
    }

    /// Shows the library as it is now with `linked` selected. The store announces every
    /// state change, so the list is rebuilt only when the prompts themselves changed.
    fn sync(
        &mut self,
        prompts: &[Prompt],
        linked: Option<u64>,
        w: &mut Window,
        cx: &mut Context<TemplateEditor>,
    ) {
        let choices = prompt_choices(prompts);
        let selected = self.select.read(cx).selected_value().copied();
        if choices == self.choices && selected == Some(linked) {
            return;
        }
        self.select.update(cx, |select, cx| {
            if choices != self.choices {
                select.set_items(SearchableVec::new(choices.clone()), w, cx);
            }
            select.set_selected_value(&linked, w, cx);
        });
        self.choices = choices;
    }

    fn render(&self) -> AnyElement {
        Select::new(&self.select)
            .small()
            .w_full()
            .accessibility_label("Prompt")
            .search_placeholder("Search prompts")
            .menu_width(rems(17.))
            .into_any_element()
    }
}

impl TemplateEditor {
    /// Links the selected stage to a library prompt, or with `None` hands its text back to
    /// the stage for editing.
    fn follow_prompt(&mut self, id: Option<u64>, w: &mut Window, cx: &mut Context<Self>) {
        let library = &AppState::get(cx).store.read(cx).state.pipelines;
        let text = id.and_then(|id| library.prompt(id)).map(|p| p.text.clone());
        let form = &mut self.inspector;
        match text {
            Some(text) => {
                form.prompt = id;
                form.edit_prompt = false;
                form.instruction
                    .update(cx, |area, cx| area.set_value(text, w, cx));
            }
            None => {
                form.prompt = None;
                form.edit_prompt = true;
                form.instruction.read(cx).focus_handle(cx).focus(w, cx);
            }
        }
        self.sync_prompts(w, cx);
        cx.notify();
    }

    /// Brings the draft and the open stage in line with the prompt library, which the
    /// Settings window can change while this editor is open.
    pub(in crate::ui::workspace::pipelines) fn sync_prompts(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompts = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .prompts()
            .to_vec();
        self.template.sync_prompts(&prompts);
        // A prompt changed in Settings is not an edit of this draft.
        self.opened.sync_prompts(&prompts);
        let form = &mut self.inspector;
        if let Some(id) = form.prompt {
            match prompts.iter().find(|p| p.id == id) {
                Some(prompt) if form.instruction.read(cx).value() != prompt.text.as_str() => {
                    let text = prompt.text.clone();
                    form.instruction
                        .update(cx, |area, cx| area.set_value(text, w, cx));
                }
                Some(_) => {}
                None => form.prompt = None,
            }
        }
        let linked = form.prompt;
        if let Some(picker) = &mut form.prompt_picker {
            picker.sync(&prompts, linked, w, cx);
        }
    }

    pub(super) fn instructions(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let lane = matches!(self.selected, Item::Lane(_));
        let mut title = section_title(
            if lane {
                Lucide::SquareTerminal
            } else {
                Lucide::MessageSquare
            },
            if lane {
                "Required QA command"
            } else {
                "Instructions"
            },
            cx,
        );
        if form.prompt.is_none() {
            title = title.child(icon_action(
                "edit-stage-prompt",
                if form.edit_prompt {
                    "Finish editing"
                } else {
                    "Edit"
                },
                if form.edit_prompt {
                    Lucide::Check
                } else {
                    Lucide::Pencil
                },
                w,
                cx,
                |s: &mut Self, w, cx| {
                    s.inspector.edit_prompt = !s.inspector.edit_prompt;
                    if s.inspector.edit_prompt {
                        s.inspector
                            .instruction
                            .read(cx)
                            .focus_handle(cx)
                            .focus(w, cx);
                    }
                    cx.notify()
                },
            ));
        }
        let mut prompt_box = section(cx).child(title);
        if let Some(picker) = &form.prompt_picker {
            prompt_box = prompt_box.child(picker.render());
        }
        match form.prompt {
            Some(id) => prompt_box.children(self.library_prompt(id, w, cx)),
            None if form.edit_prompt => prompt_box.child(TextField::textarea(
                Textarea::new(&form.instruction),
                &form.instruction,
                cx,
            )),
            None => prompt_box.child(self.own_text_preview(lane, w, cx)),
        }
    }

    /// A followed prompt is read-only here: its text belongs to the library.
    fn library_prompt(&self, id: u64, w: &mut Window, cx: &mut Context<Self>) -> [AnyElement; 3] {
        let text = self.inspector.instruction.read(cx).value();
        let preview = div()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(preview_text(self.selected, text));
        let note = subtle(
            "From the prompt library. Editing it in Settings updates every stage that follows it.",
            cx,
        );
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child(button(
                "edit-library-prompt",
                "Edit in Settings",
                ButtonLook::Outline,
                w,
                cx,
                move |_, _, cx| crate::ui::settings::open_prompt(id, cx),
            ))
            .child(button(
                "customize-stage-prompt",
                "Customize",
                ButtonLook::Ghost,
                w,
                cx,
                |s: &mut Self, w, cx| s.follow_prompt(None, w, cx),
            ));
        [
            preview.into_any_element(),
            note.into_any_element(),
            actions.into_any_element(),
        ]
    }

    fn own_text_preview(&self, lane: bool, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let prompt = self.inspector.instruction.read(cx).value();
        let empty = prompt.is_empty();
        let hover = Hover::new("stage-prompt-preview", w, cx);
        // The text itself opens the editor: the pencil is easy to miss in a narrow panel.
        let preview = div()
            .id("stage-prompt-preview")
            .role(Role::Button)
            .aria_label(if lane {
                "Edit required QA command"
            } else {
                "Edit instructions"
            })
            .tab_index(0)
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .text_sm()
            .when(empty, |el| el.italic())
            .text_color(if empty {
                cx.theme().muted_foreground
            } else {
                cx.theme().foreground
            })
            .child(preview_text(
                self.selected,
                match (empty, lane) {
                    (false, _) => prompt,
                    (true, true) => "No command required".into(),
                    (true, false) => "No instructions".into(),
                },
            ));
        on_activate(preview, cx, |s: &mut Self, w, cx| {
            s.inspector.edit_prompt = true;
            s.inspector
                .instruction
                .read(cx)
                .focus_handle(cx)
                .focus(w, cx);
            cx.notify()
        })
        .hover_bg(&hover, cx.theme().transparent, cx.theme().muted)
        .into_any_element()
    }
}

/// Instructions capped at [`PREVIEW_LINES`], scrolling past that. Each stage keeps its own
/// scroll position, so opening another stage does not start halfway down its text.
fn preview_text(item: Item, text: SharedString) -> impl IntoElement {
    let id: ElementId = match item {
        Item::Planner => "plan-instructions".into(),
        Item::Development => "development-instructions".into(),
        Item::Lane(index) => ("lane-command", index).into(),
        Item::Review(index) => ("review-instructions", index).into(),
    };
    div()
        .max_h(PREVIEW_LINE_HEIGHT * PREVIEW_LINES)
        .line_height(PREVIEW_LINE_HEIGHT)
        .child(text)
        .overflow_y_scrollbar()
        .id(id)
}

#[cfg(test)]
mod tests {
    use super::{CUSTOM, Prompt, prompt_choices};

    #[test]
    fn prompt_choices_list_the_stages_own_text_first() {
        let prompts = ["Planner", "Reviewer"].map(|name| Prompt {
            id: 1000 + name.len() as u64,
            name: name.into(),
            text: "Text".into(),
        });
        let choices = prompt_choices(&prompts);
        let titles: Vec<_> = choices.iter().map(|c| c.title.to_string()).collect();
        assert_eq!(titles, [CUSTOM, "Planner", "Reviewer"]);
        let ids: Vec<_> = choices.iter().map(|c| c.id).collect();
        assert_eq!(ids, [None, Some(1007), Some(1008)]);
        assert_eq!(prompt_choices(&[]).len(), 1);
    }
}
