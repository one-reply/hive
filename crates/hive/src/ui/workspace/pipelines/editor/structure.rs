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
    /// Move it into the gap before this position: among the lanes for a lane, among the
    /// rows of reviews for a review, which then stands on a row of its own.
    Move(usize),
    /// Put the review after the one at this position, to run together with it.
    Join(usize),
    Remove,
}

/// The menu of the selected stage, in order. Empty when the stage can neither move nor go.
fn stage_actions(t: &Template, selected: Item) -> Vec<(String, StageAction)> {
    let mut actions = Vec::new();
    let mut offer = |label: &str, action| actions.push((label.to_owned(), action));
    let removable = match selected {
        Item::Lane(i) => {
            if i > 0 {
                offer("Move earlier", StageAction::Move(i - 1));
            }
            if i + 1 < t.lanes.len() {
                offer("Move later", StageAction::Move(i + 2));
            }
            t.lanes.len() > 1
        }
        Item::Review(i) => {
            let rows = t.review_groups();
            let row = t.review_row(i);
            if rows[row].len() > 1 {
                // Leaving the group is one step; where it goes next is another.
                offer("Run on its own", StageAction::Move(row + 1));
            } else {
                if row > 0 {
                    offer("Move earlier", StageAction::Move(row - 1));
                    let previous = rows[row - 1].end - 1;
                    let label = format!("Run alongside {}", t.reviews[previous].name);
                    offer(&label, StageAction::Join(previous));
                }
                if row + 1 < rows.len() {
                    offer("Move later", StageAction::Move(row + 2));
                }
            }
            i != t.qa_index()
        }
        Item::Planner | Item::Development => return Vec::new(),
    };
    if removable {
        offer("Remove stage", StageAction::Remove);
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
            .map(|(label, action)| (label, false, action))
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
            move |s, action, w, cx| match (action, selected) {
                (StageAction::Move(before), _) => s.move_to(selected, before, w, cx),
                (StageAction::Join(beside), Item::Review(review)) => {
                    s.join(review, beside, true, w, cx)
                }
                (StageAction::Join(_), _) => {}
                (StageAction::Remove, _) => s.remove(w, cx),
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

    /// The menu of `item` as labels with what each does.
    fn menu(t: &Template, item: Item) -> Vec<(String, StageAction)> {
        stage_actions(t, item)
    }

    fn entry(label: &str, action: StageAction) -> (String, StageAction) {
        (label.to_owned(), action)
    }

    #[test]
    fn qa_moves_like_any_review_but_is_never_removed() {
        // Code review, QA, Security & performance, each on a row of its own.
        let t = Template::default();
        assert_eq!(
            menu(&t, Item::Review(1)),
            [
                entry("Move earlier", StageAction::Move(0)),
                entry("Run alongside Code review", StageAction::Join(0)),
                entry("Move later", StageAction::Move(3)),
            ]
        );
        assert_eq!(
            menu(&t, Item::Review(0)),
            [
                entry("Move later", StageAction::Move(2)),
                entry("Remove stage", StageAction::Remove),
            ]
        );
        assert_eq!(
            menu(&t, Item::Review(2)),
            [
                entry("Move earlier", StageAction::Move(1)),
                entry("Run alongside QA", StageAction::Join(1)),
                entry("Remove stage", StageAction::Remove),
            ]
        );
    }

    #[test]
    fn the_menu_moves_a_stage_to_where_it_says() {
        let mut t = Template::default();
        let (_, later) = menu(&t, Item::Review(1))[2];
        assert_eq!(later, StageAction::Move(3));
        assert_eq!(t.move_review(1, 3), Some(2));
        let (_, earlier) = menu(&t, Item::Review(2))[0];
        assert_eq!(earlier, StageAction::Move(1));
        assert_eq!(t.move_review(2, 1), Some(1));
        assert_eq!(t.qa_index(), 1);
    }

    #[test]
    fn a_review_joins_the_row_above_and_one_in_a_group_can_only_leave_it_first() {
        let mut t = Template::default();
        let (label, join) = menu(&t, Item::Review(2))[1].clone();
        assert_eq!(label, "Run alongside QA");
        let StageAction::Join(beside) = join else {
            panic!("{join:?}");
        };
        assert_eq!(t.join_review(2, beside, true), Some(2));
        assert_eq!(t.review_groups(), [0..1, 1..3]);
        // Both members of the group: one step takes either out, onto a row after it.
        for review in [1, 2] {
            let mut expected = vec![entry("Run on its own", StageAction::Move(2))];
            if review != t.qa_index() {
                expected.push(entry("Remove stage", StageAction::Remove));
            }
            assert_eq!(menu(&t, Item::Review(review)), expected);
        }
        assert_eq!(t.move_review(1, 2), Some(2));
        assert_eq!(t.review_groups(), [0..1, 1..2, 2..3]);
        assert_eq!(t.reviews[2].name, "QA");
        // A review joins the last review of the row above it.
        t.join_review(1, 0, true).unwrap();
        assert_eq!(
            menu(&t, Item::Review(2))[1],
            entry("Run alongside Security & performance", StageAction::Join(1))
        );
    }

    #[test]
    fn the_only_lane_and_the_fixed_stages_have_no_menu() {
        let mut t = Template::default();
        assert!(menu(&t, Item::Lane(0)).is_empty());
        assert!(menu(&t, Item::Planner).is_empty());
        assert!(menu(&t, Item::Development).is_empty());
        t.insert_lane(1, "API");
        assert_eq!(
            menu(&t, Item::Lane(0)),
            [
                entry("Move later", StageAction::Move(2)),
                entry("Remove stage", StageAction::Remove),
            ]
        );
    }
}
