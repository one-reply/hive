//! Adding, moving and removing stages. Lanes and reviews move among their own kind; the
//! required QA review moves like any other but cannot be removed.
use super::drag::{Ghost, Load};
use super::nodes::{Detail, Face};
use super::*;
use gpui_kit::component::button::ButtonVariants as _;
use hive_core::pipeline::ReviewKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum StageChoice {
    Lane,
    Review(ReviewKind),
}
impl StageChoice {
    /// Every stage the palette offers, lanes first.
    fn all() -> impl Iterator<Item = Self> {
        std::iter::once(Self::Lane).chain(ReviewKind::ALL.map(Self::Review))
    }

    /// Palette label, which is also the name the new stage starts with.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Lane => "Development lane",
            Self::Review(kind) => kind.stage_name(),
        }
    }

    fn role(self) -> RoleKind {
        match self {
            Self::Lane => RoleKind::Build,
            Self::Review(ReviewKind::Code) => RoleKind::CodeReview,
            Self::Review(ReviewKind::Security) => RoleKind::Security,
            Self::Review(ReviewKind::Performance) => RoleKind::Performance,
            Self::Review(ReviewKind::Acceptance) => RoleKind::Review,
        }
    }
}

/// What the stage menu can do with the selected stage.
#[derive(Clone, Copy, Debug, PartialEq)]
enum StageAction {
    /// Move it into the gap before this position of its row.
    Move(usize),
    Remove,
}

/// The menu of the selected stage, in order. Empty when the stage can neither move nor go.
fn stage_actions(t: &Template, selected: Item) -> Vec<(&'static str, StageAction)> {
    let (index, count, removable) = match selected {
        Item::Lane(i) => (i, t.lanes.len(), t.lanes.len() > 1),
        Item::Review(i) => (i, t.reviews.len(), i != t.qa_index()),
        Item::Planner | Item::Development => return Vec::new(),
    };
    let mut actions = Vec::new();
    if index > 0 {
        actions.push(("Move earlier", StageAction::Move(index - 1)));
    }
    if index + 1 < count {
        actions.push(("Move later", StageAction::Move(index + 2)));
    }
    if removable {
        actions.push(("Remove stage", StageAction::Remove));
    }
    actions
}

impl TemplateEditor {
    /// New stages: a click adds one at its usual place, a drag puts it where it is dropped.
    pub(super) fn palette(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let editor = cx.entity_id();
        let mut palette = h_flex()
            .flex_none()
            .flex_wrap()
            .gap_1()
            .px_3()
            .py_1p5()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(subtle("Add stage", cx));
        for stage in StageChoice::all() {
            let role = stage.role();
            let dragged = Dragged {
                editor,
                load: Load::Add(stage),
                ghost: Ghost::Stage(
                    Face {
                        role,
                        glyph: role.icon(),
                        title: stage.label().into(),
                        detail: Detail::Note("New stage".into()),
                    },
                    None,
                ),
            };
            palette = palette.child(
                button_with_icon(
                    SharedString::from(format!("add-stage-{}", stage.label())),
                    stage.label(),
                    role.icon(),
                    ButtonLook::Outline,
                    w,
                    cx,
                    move |s, w, cx| s.add(stage, None, w, cx),
                )
                .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone())),
            );
        }
        palette.child(subtle("Click, or drag onto the canvas", cx))
    }

    pub(in crate::ui::workspace::pipelines) fn ordering(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let selected = self.selected;
        let actions = stage_actions(&self.template, selected);
        if actions.is_empty() {
            return None;
        }
        let options = actions
            .into_iter()
            .map(|(label, action)| (label.to_owned(), false, action))
            .collect();
        Some(options_menu(
            Button::new("selected-stage-actions")
                .ghost()
                .small()
                .cursor_pointer()
                .icon(Lucide::Ellipsis)
                .accessibility_label("Stage actions")
                .tooltip("Stage actions"),
            options,
            cx,
            move |s, action, w, cx| match action {
                StageAction::Move(before) => s.move_to(selected, before, w, cx),
                StageAction::Remove => s.remove(w, cx),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, ReviewKind, RoleKind, StageAction, StageChoice, Template, stage_actions};
    use hive_core::pipeline::Library;

    #[test]
    fn palette_roles_match_the_stages_on_the_canvas() {
        assert_eq!(StageChoice::Lane.role(), RoleKind::Build);
        let library = Library::default();
        let mut template = Template::default();
        for kind in ReviewKind::ALL {
            let i = template.add_review(library.review_step(kind));
            assert_eq!(
                StageChoice::Review(kind).role(),
                RoleKind::review(&template, i)
            );
        }
        assert_eq!(StageChoice::all().count(), 1 + ReviewKind::ALL.len());
    }

    #[test]
    fn qa_moves_like_any_review_but_is_never_removed() {
        // Code review, QA, Security & performance.
        let t = Template::default();
        let labels = |item| -> Vec<_> { stage_actions(&t, item).into_iter().collect() };
        assert_eq!(
            labels(Item::Review(1)),
            [
                ("Move earlier", StageAction::Move(0)),
                ("Move later", StageAction::Move(3)),
            ]
        );
        assert_eq!(
            labels(Item::Review(0)),
            [
                ("Move later", StageAction::Move(2)),
                ("Remove stage", StageAction::Remove),
            ]
        );
        assert_eq!(
            labels(Item::Review(2)),
            [
                ("Move earlier", StageAction::Move(1)),
                ("Remove stage", StageAction::Remove),
            ]
        );
    }

    #[test]
    fn the_menu_moves_a_stage_to_where_it_says() {
        let mut t = Template::default();
        let (_, later) = stage_actions(&t, Item::Review(1))[1];
        assert_eq!(later, StageAction::Move(3));
        assert_eq!(t.move_review(1, 3), Some(2));
        let (_, earlier) = stage_actions(&t, Item::Review(2))[0];
        assert_eq!(earlier, StageAction::Move(1));
        assert_eq!(t.move_review(2, 1), Some(1));
        assert_eq!(t.qa_index(), 1);
    }

    #[test]
    fn the_only_lane_and_the_fixed_stages_have_no_menu() {
        let mut t = Template::default();
        assert!(stage_actions(&t, Item::Lane(0)).is_empty());
        assert!(stage_actions(&t, Item::Planner).is_empty());
        assert!(stage_actions(&t, Item::Development).is_empty());
        t.insert_lane(1, "API");
        assert_eq!(
            stage_actions(&t, Item::Lane(0)),
            [
                ("Move later", StageAction::Move(2)),
                ("Remove stage", StageAction::Remove),
            ]
        );
    }
}
