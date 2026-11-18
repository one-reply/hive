//! Adding, reordering and removing stages: the first QA review is a fixed barrier, and lanes
//! and reviews only move among their own kind.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum StageChoice {
    Lane,
    Code,
    Security,
    Performance,
    Custom,
}
impl StageChoice {
    pub(super) fn recipe(self) -> (&'static str, &'static str) {
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
    pub(super) fn ordering(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

#[cfg(test)]
mod tests {
    use super::{Item, StageChoice, Template, move_item, reorder};
    #[test]
    fn every_added_review_has_a_runnable_default() {
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
