//! Adding, reordering and removing stages: the required QA review is a fixed barrier, and lanes
//! and reviews only move among their own kind.
use super::*;
use gpui_kit::component::button::ButtonVariants as _;
use hive_core::pipeline::ReviewKind;

#[derive(Clone, Copy)]
pub(super) enum StageChoice {
    Lane,
    Review(ReviewKind),
}
impl StageChoice {
    /// Menu label, which is also the name the new stage starts with.
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
/// Which positions an optional review can occupy on its side of QA.
pub(super) fn review_positions(t: &Template, index: usize) -> std::ops::Range<usize> {
    let qa = t.qa_index();
    match index.cmp(&qa) {
        std::cmp::Ordering::Less => 0..qa,
        std::cmp::Ordering::Equal => qa..qa,
        std::cmp::Ordering::Greater => qa + 1..t.reviews.len(),
    }
}

pub(super) fn move_item(t: &mut Template, from: Item, to: Item) -> bool {
    match (from, to) {
        (Item::Lane(a), Item::Lane(b)) => reorder(&mut t.lanes, a, b),
        (Item::Review(a), Item::Review(b)) if review_positions(t, a).contains(&b) => {
            reorder(&mut t.reviews, a, b)
        }
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
    pub(super) fn add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let editor = cx.entity();
        Button::new("add-stage")
            .small()
            .cursor_pointer()
            .icon(Lucide::Plus)
            .dropdown_caret(true)
            .label("Add stage")
            .dropdown_menu(move |mut menu, window, _| {
                for stage in std::iter::once(StageChoice::Lane)
                    .chain(ReviewKind::ALL.map(StageChoice::Review))
                {
                    menu = menu.item(
                        PopupMenuItem::new(stage.label())
                            .icon(stage.role().icon())
                            .on_click(window.listener_for(&editor, move |editor, _, w, cx| {
                                editor.add(stage, w, cx);
                            })),
                    );
                }
                menu
            })
            .into_any_element()
    }
    pub(in crate::ui::workspace::pipelines) fn ordering(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (min, count, index, make): (usize, usize, usize, fn(usize) -> Item) =
            match self.selected {
                Item::Lane(i) if self.template.lanes.len() > 1 => {
                    (0, self.template.lanes.len(), i, Item::Lane)
                }
                Item::Review(i) => {
                    let positions = review_positions(&self.template, i);
                    if positions.is_empty() {
                        return None;
                    }
                    (positions.start, positions.end, i, Item::Review)
                }
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
                .ghost()
                .small()
                .cursor_pointer()
                .icon(Lucide::Ellipsis)
                .accessibility_label("Stage actions")
                .tooltip("Stage actions"),
            options,
            cx,
            move |s, dest, w, cx| match dest {
                Some(to) => s.move_to(make(index), to, w, cx),
                None => s.remove(w, cx),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Item, ReviewKind, RoleKind, StageChoice, Template, move_item, reorder};
    use hive_core::pipeline::Library;

    #[test]
    fn add_menu_roles_match_the_stages_on_the_canvas() {
        assert_eq!(StageChoice::Lane.role(), RoleKind::Build);
        let library = Library::default();
        let mut template = Template::default();
        for kind in ReviewKind::ALL {
            let i = template.add_review(library.review_step(kind));
            assert_eq!(
                StageChoice::Review(kind).role(),
                RoleKind::review(i, &template.reviews[i])
            );
        }
    }

    #[test]
    fn cannot_drag_the_qa_barrier_or_move_lanes_into_review() {
        let mut t = Template::default();
        assert!(!move_item(&mut t, Item::Review(1), Item::Review(0)));
        assert!(!move_item(&mut t, Item::Lane(0), Item::Review(1)));
        assert!(!move_item(&mut t, Item::Review(2), Item::Review(0)));
        t.reviews.push(t.reviews[2].clone());
        t.reviews[3].name = "Extra review".into();
        assert!(move_item(&mut t, Item::Review(3), Item::Review(2)));
        assert_eq!(t.reviews[1].name, "QA");
        assert_eq!(t.reviews[2].name, "Extra review");
        let mut items = vec!["api", "web", "mobile"];
        assert!(reorder(&mut items, 0, 2));
        assert_eq!(items, ["web", "mobile", "api"]);
    }
}
