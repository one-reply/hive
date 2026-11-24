//! When an optional review runs: on every pass, or only when the change touches given paths.
use super::*;

impl TemplateEditor {
    /// Whether the review at `index` is limited to some paths, the open form's unsaved text
    /// included. QA never is: it always runs.
    pub(in super::super) fn is_conditional(&self, index: usize, cx: &App) -> bool {
        if index == self.template.qa_index() {
            return false;
        }
        if self.selected == Item::Review(index) {
            return !self.inspector.paths.read(cx).value().trim().is_empty();
        }
        self.template.review_condition(index).is_some()
    }

    /// The paths the selected review is limited to; nothing for QA and for other stages.
    pub(super) fn review_condition(&self, cx: &mut Context<Self>) -> Option<Div> {
        let Item::Review(index) = self.selected else {
            return None;
        };
        if index == self.template.qa_index() {
            return None;
        }
        Some(
            section(cx)
                .child(section_title(Lucide::Funnel, "Runs when", cx))
                .child(subtle(
                    condition_summary(self.is_conditional(index, cx)),
                    cx,
                ))
                .child(paragraph(
                    "Changed paths, one pattern per line",
                    &self.inspector.paths,
                    cx,
                )),
        )
    }
}

/// What the paths field means for the review, in words.
fn condition_summary(limited: bool) -> &'static str {
    if limited {
        "This review runs only when the change touches one of these paths. Otherwise Hive skips it and says so on the run's timeline."
    } else {
        "This review runs on every pass. List folders, file names or globs to run it only when the change touches one of them."
    }
}

#[cfg(test)]
mod tests {
    use super::condition_summary;

    #[test]
    fn the_summary_says_whether_the_review_can_be_skipped() {
        assert!(condition_summary(false).starts_with("This review runs on every pass."));
        assert!(condition_summary(true).contains("Otherwise Hive skips it"));
    }
}
