//! Adding, reordering and removing stages: the first QA review is a fixed barrier, and lanes
//! and reviews only move among their own kind.
use super::*;
use gpui_kit::component::button::ButtonVariants as _;

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
                "Review the changed code for correctness and maintainability: logic errors, unhandled edge cases and error paths, broken callers of changed functions, logic that duplicates code already in the repository, and missing or weak tests. Report only actionable defects, each with its location, the problem and the expected fix; skip style that the formatter or linter already enforces.",
            ),
            Self::Security => (
                "Security review",
                "Review the changed code and the paths it touches for vulnerabilities: untrusted input reaching shell commands, queries, file paths or HTML; missing authentication or authorization checks; secrets in code, logs or errors; unsafe deserialization; risky new dependencies. Report each issue with its location, how it can be exploited and a defensive fix; skip theoretical concerns and code the change does not touch.",
            ),
            Self::Performance => (
                "Performance review",
                "Inspect the changed paths for performance regressions: extra work on hot paths, I/O or queries inside loops, unbounded memory or result sizes, blocking calls on async or UI threads, missing limits or pagination. Report each issue with its location, its expected cost at realistic input sizes and a fix; do not flag micro-optimizations without evidence.",
            ),
            Self::Custom => (
                "Custom review",
                "Check the changes against every approved acceptance criterion in each worktree, using the code and tests rather than the developers' summaries. Report each unmet criterion with its evidence and the expected behavior.",
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
