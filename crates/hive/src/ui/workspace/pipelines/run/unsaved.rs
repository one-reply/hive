//! Text typed on the run screen and not sent yet: leaving the screen would drop it.
use super::*;

/// What leaving would lose. An edited plan outranks a typed note: it is the larger loss,
/// and the one the question names.
fn unsent(plan_edited: bool, feedback: Option<&str>) -> Option<Unsaved> {
    if plan_edited {
        return Some(Unsaved::Plan);
    }
    feedback
        .filter(|note| !note.trim().is_empty())
        .map(|_| Unsaved::Feedback)
}

impl RunView {
    pub(in crate::ui::workspace::pipelines) fn id(&self) -> u64 {
        self.id
    }

    /// The plan editor counts only once it differs from the plan the run holds, and the
    /// feedback box only while it is open with text in it.
    pub(in crate::ui::workspace::pipelines) fn unsaved(&self, cx: &App) -> Option<Unsaved> {
        let plan_edited = self.editing_plan
            && self.run.as_deref().is_some_and(|run| {
                let typed: Vec<_> = self
                    .plan
                    .iter()
                    .map(|(task, criteria)| (task.read(cx).value(), criteria.read(cx).value()))
                    .collect();
                let saved = run
                    .lanes()
                    .iter()
                    .map(|lane| (lane.assignment.as_str(), lane.criteria.as_str()));
                !saved.eq(typed.iter().map(|(task, criteria)| (&**task, &**criteria)))
                    || run.shared_contract() != &*self.contract.read(cx).value()
            });
        let feedback = self.giving_feedback.then(|| self.note.read(cx).value());
        unsent(plan_edited, feedback.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::{Unsaved, unsent};

    #[test]
    fn only_an_edited_plan_or_typed_feedback_would_be_lost() {
        assert_eq!(unsent(false, None), None);
        // The feedback box is open but nothing was typed into it.
        assert_eq!(unsent(false, Some(" \n")), None);
        assert_eq!(
            unsent(false, Some("Handle the expired session")),
            Some(Unsaved::Feedback)
        );
        assert_eq!(unsent(true, None), Some(Unsaved::Plan));
        assert_eq!(unsent(true, Some("Also this")), Some(Unsaved::Plan));
    }
}
