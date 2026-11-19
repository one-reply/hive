//! Adding, reordering and removing stages: the first QA review is a fixed barrier, and lanes
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
}
pub(super) fn move_item(t: &mut Template, from: Item, to: Item) -> bool {
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
    pub(super) fn add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let options = std::iter::once(StageChoice::Lane)
            .chain(ReviewKind::ALL.map(StageChoice::Review))
            .map(|stage| (stage.label().to_owned(), false, stage))
            .collect();
        options_menu(
            Button::new("add-stage")
                .small()
                .cursor_pointer()
                .icon(Lucide::Plus)
                .dropdown_caret(true)
                .label("Add stage"),
            options,
            cx,
            |s, stage, w, cx| s.add(stage, w, cx),
        )
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
    use super::{Item, Template, move_item, reorder};
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
