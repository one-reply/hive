//! What the user sends back from acceptance: line comments for one lane or a request for
//! the whole task. Inputs are validated before mutation.
use super::*;
use crate::git::review_notes;
use std::fmt::Write as _;

impl Run {
    /// Submit the selected draft notes as one repair, retaining later notes and other recipients.
    pub fn submit_comments(
        &mut self,
        lane: usize,
        tree: &str,
        batch: &[review_notes::Note],
    ) -> Result<(), Error> {
        if batch.is_empty()
            || batch.iter().any(|n| {
                !self
                    .comments
                    .iter()
                    .any(|(i, t, note)| *i == lane && t == tree && note == n)
            })
        {
            return Err(Error::InvalidInput(
                "The comment batch changed. Review the remaining notes.",
            ));
        }
        self.request_lane_changes(lane, tree, &review_notes::prompt(batch))?;
        for note in batch {
            self.remove_comment(&(lane, tree.to_owned(), note.clone()));
        }
        Ok(())
    }

    /// Return a batch of line comments to the developer of the selected reviewed checkout.
    pub fn request_lane_changes(
        &mut self,
        lane: usize,
        tree: &str,
        note: &str,
    ) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("The result is no longer awaiting review"));
        }
        if self.lanes.get(lane).is_none_or(|l| l.assignment.is_empty())
            || self.review_trees.get(lane).is_none_or(|t| t != tree)
            || note.trim().is_empty()
        {
            return Err(Error::InvalidInput(
                "The comment position is stale. Review the current diff.",
            ));
        }
        self.feedback_trees.clone_from(&self.review_trees);
        self.feedback = format!(
            "User requested changes to {} (reviewed tree {tree}):\n{note}",
            self.lanes[lane].name
        );
        self.history.push(self.feedback.clone());
        self.briefs = vec![(lane, self.feedback.clone())];
        self.develop([lane]);
        self.continue_conversations();
        Ok(())
    }

    /// Return a final result to its active development lanes for another review cycle.
    pub fn request_changes(&mut self, note: &str) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("There is no result awaiting acceptance"));
        }
        self.feedback_trees.clone_from(&self.review_trees);
        self.feedback = if note.trim().is_empty() {
            "User requested another pass. Compare the implementation with the approved assignment and address any remaining gaps.".into()
        } else {
            format!("User requested changes: {note}")
        };
        // Line comments on the reviewed trees belong to this request: the repair replaces
        // those trees, and a comment left behind could never be sent afterwards.
        for (lane, binding) in self.lanes.iter().enumerate() {
            let batch: Vec<_> = self
                .comments
                .iter()
                .filter(|(to, tree, _)| *to == lane && self.review_trees.get(lane) == Some(tree))
                .map(|(_, _, note)| note.clone())
                .collect();
            if !batch.is_empty() {
                let _ = write!(
                    self.feedback,
                    "\nLine comments for {}:\n{}",
                    binding.name,
                    review_notes::prompt(&batch)
                );
            }
        }
        let reviewed = &self.review_trees;
        self.comments
            .retain(|(lane, tree, _)| reviewed.get(*lane) != Some(tree));
        self.history.push(self.feedback.clone());
        // The user read these notes at acceptance and may be referring to them.
        if !self.notes.is_empty() {
            self.feedback.push_str(
                "\nReviewer notes that did not block acceptance; address the ones this request covers:",
            );
            for (review, finding) in &self.notes {
                let _ = write!(
                    self.feedback,
                    "\n- {} · {}: {}",
                    self.template.reviews[*review].name,
                    self.lanes[finding.lane].name,
                    finding.description
                );
            }
        }
        self.briefs.clear();
        let active: Vec<_> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.assignment.trim().is_empty())
            .map(|(i, _)| i)
            .collect();
        self.develop(active);
        self.continue_conversations();
        Ok(())
    }
}
