//! Editing rules shared by every template editor: names stay unique, routes follow renames.
use super::{Error, LanePreset, Library, PlanApproval, Profile, ReviewRole, Step, Template};
use std::collections::HashSet;
use std::ops::Range;

/// Where the item at `from` lands when it is dropped into the gap before position `before`
/// (`0..=len`). `None` when it would stay where it is or a position is out of range.
fn destination(len: usize, from: usize, before: usize) -> Option<usize> {
    if from >= len || before > len {
        return None;
    }
    let to = if from < before { before - 1 } else { before };
    (to != from).then_some(to)
}

/// `base`, or `base N` with the first free number; names compare ignoring ASCII case.
pub fn unique_name<'a>(base: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let names: HashSet<_> = existing.map(str::to_ascii_lowercase).collect();
    let available = |name: &str| !names.contains(&name.to_ascii_lowercase());
    if available(base) {
        return base.to_owned();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| available(name))
        .expect("an unbounded sequence of numbered names has a free one")
}

impl Library {
    /// Position of `original` exactly as the editor loaded it; another project may have
    /// changed or deleted it since.
    pub fn template_index(&self, original: &Template) -> Result<usize, Error> {
        self.templates
            .iter()
            .position(|t| t == original)
            .ok_or(Error::InvalidInput(
                "This template was changed or deleted. Reopen it from the library.",
            ))
    }

    /// Where saving a draft named `name` goes: replacing `original`, or appended when new.
    /// Rejects a name another template already uses.
    pub fn template_slot(
        &self,
        original: Option<&Template>,
        name: &str,
    ) -> Result<Option<usize>, Error> {
        let slot = original.map(|t| self.template_index(t)).transpose()?;
        if self
            .templates
            .iter()
            .enumerate()
            .any(|(i, t)| Some(i) != slot && t.name.eq_ignore_ascii_case(name))
        {
            return Err(Error::InvalidInput("Choose a unique template name."));
        }
        Ok(slot)
    }

    /// Moves the template named `name` to the place of the one named `onto`; the templates
    /// between them shift by one towards the gap. Returns whether the order changed: an
    /// unknown name and a template moved onto itself change nothing.
    pub fn move_template(&mut self, name: &str, onto: &str) -> bool {
        let position = |wanted: &str| self.templates.iter().position(|t| t.name == wanted);
        let (Some(from), Some(to)) = (position(name), position(onto)) else {
            return false;
        };
        if from == to {
            return false;
        }
        let template = self.templates.remove(from);
        self.templates.insert(to, template);
        true
    }
}

impl Template {
    /// Position of the required QA stage. Saved recipes without a marker used the first review.
    pub fn qa_index(&self) -> usize {
        self.reviews
            .iter()
            .position(|s| s.review_role == ReviewRole::Qa)
            .unwrap_or(0)
    }

    /// Whether the planner's report releases development without the user. A recipe with a
    /// shared contract always asks: the contract is the user's to approve.
    pub fn auto_approves_plan(&self) -> bool {
        self.plan_approval == PlanApproval::Auto && !self.shared_contract
    }

    /// Flips who approves the plan. A recipe with a shared contract keeps asking.
    pub fn toggle_plan_approval(&mut self) {
        self.plan_approval = match self.plan_approval {
            PlanApproval::Ask if !self.shared_contract => PlanApproval::Auto,
            PlanApproval::Ask | PlanApproval::Auto => PlanApproval::Ask,
        };
    }

    /// Turns the shared contract on or off. Turning it on returns the plan to the user.
    pub fn set_shared_contract(&mut self, on: bool) {
        self.shared_contract = on;
        if on {
            self.plan_approval = PlanApproval::Ask;
        }
    }

    /// Marks QA explicitly. A recipe saved before roles has QA as its unmarked first review,
    /// which stops being true once another review can stand before it.
    fn pin_qa(&mut self) {
        let qa = self.qa_index();
        if let Some(step) = self.reviews.get_mut(qa)
            && step.review_role == ReviewRole::Unspecified
        {
            step.review_role = ReviewRole::Qa;
        }
    }

    /// The reviews that run together with the one at `index`, itself included: a review and
    /// the ones joined to it by [`Step::with_previous`]. Empty past the last review.
    pub fn review_group(&self, index: usize) -> Range<usize> {
        let count = self.reviews.len();
        if index >= count {
            return count..count;
        }
        let joined = |review: usize| review > 0 && self.reviews[review].with_previous;
        let start = (0..=index).rev().find(|&review| !joined(review));
        let end = (index + 1..count).find(|&review| !joined(review));
        start.unwrap_or(0)..end.unwrap_or(count)
    }

    /// The groups of reviews in the order they run; a review on its own is a group of one.
    pub fn review_groups(&self) -> Vec<Range<usize>> {
        let mut groups = Vec::new();
        let mut next = 0;
        while next < self.reviews.len() {
            let group = self.review_group(next);
            next = group.end;
            groups.push(group);
        }
        groups
    }

    /// Which group the review at `index` runs in, counted from the first.
    pub fn review_row(&self, index: usize) -> usize {
        let groups = self.review_groups();
        let row = groups.iter().position(|group| group.contains(&index));
        row.unwrap_or(groups.len())
    }

    /// The reviews as rows of their positions; the reviews of a row run together.
    fn review_rows(&self) -> Vec<Vec<usize>> {
        self.review_groups()
            .into_iter()
            .map(Iterator::collect)
            .collect()
    }

    /// Rearranges the reviews into `rows` of their current positions and returns where the
    /// review at `followed` lands. `None`, and nothing touched, when that is how they stand.
    fn arrange_reviews(&mut self, rows: Vec<Vec<usize>>, followed: usize) -> Option<usize> {
        if rows == self.review_rows() {
            return None;
        }
        self.pin_qa();
        let mut steps: Vec<_> = std::mem::take(&mut self.reviews)
            .into_iter()
            .map(Some)
            .collect();
        let mut landed = None;
        for row in rows {
            for (place, from) in row.into_iter().enumerate() {
                let Some(mut step) = steps.get_mut(from).and_then(Option::take) else {
                    continue;
                };
                step.with_previous = place > 0;
                if from == followed {
                    landed = Some(self.reviews.len());
                }
                self.reviews.push(step);
            }
        }
        // Every review is in `rows`; one that somehow is not must still not be lost.
        self.reviews.extend(steps.into_iter().flatten());
        landed
    }

    /// Adds a review at its usual place, on a row of its own: code review before the row QA
    /// runs in, any other review last.
    pub fn add_review(&mut self, step: Step) -> usize {
        let row = if step.review_role == ReviewRole::Code {
            self.review_row(self.qa_index())
        } else {
            self.review_groups().len()
        };
        self.insert_review(row, step)
    }

    /// Inserts a review on a row of its own before the row at position `row` (after the last
    /// when out of range), under a free name, and returns its position. Code review starts
    /// with QA's CLI settings and the read-only reviewer mode; other reviews keep the
    /// editor's convention of using the planner profile.
    pub fn insert_review(&mut self, row: usize, mut step: Step) -> usize {
        step.name = unique_name(&step.name, self.reviews.iter().map(|s| s.name.as_str()));
        self.pin_qa();
        step.profile = match self.reviews.get(self.qa_index()) {
            Some(qa) if step.review_role == ReviewRole::Code => Profile {
                mode: None,
                ..qa.profile.clone()
            },
            _ => self.planner.profile.clone(),
        };
        step.with_previous = false;
        let groups = self.review_groups();
        let index = groups
            .get(row)
            .map_or(self.reviews.len(), |group| group.start);
        self.reviews.insert(index, step);
        index
    }

    /// Moves a review, QA included, onto a row of its own before the row at position `row`
    /// (`0..=rows`) and returns its new position. A review taken out of a group this way
    /// leaves the others together. `None` when nothing changes or a position is out of range.
    pub fn move_review(&mut self, from: usize, row: usize) -> Option<usize> {
        let mut rows = self.review_rows();
        if from >= self.reviews.len() || row > rows.len() {
            return None;
        }
        let own = self.review_row(from);
        rows[own].retain(|review| *review != from);
        let emptied = rows[own].is_empty();
        if emptied {
            rows.remove(own);
        }
        let row = if emptied && own < row { row - 1 } else { row };
        rows.insert(row, vec![from]);
        self.arrange_reviews(rows, from)
    }

    /// Puts a review beside the one at `beside`, after it or before it, so that the two run
    /// together, and returns its new position. `None` when it already stands there.
    pub fn join_review(&mut self, from: usize, beside: usize, after: bool) -> Option<usize> {
        if from == beside || from.max(beside) >= self.reviews.len() {
            return None;
        }
        let mut rows = self.review_rows();
        rows.iter_mut()
            .for_each(|row| row.retain(|review| *review != from));
        rows.retain(|row| !row.is_empty());
        let row = rows.iter_mut().find(|row| row.contains(&beside))?;
        let place = row.iter().position(|review| *review == beside)?;
        row.insert(place + usize::from(after), from);
        self.arrange_reviews(rows, from)
    }

    /// Moves a lane into the gap before position `before` (`0..=lanes.len()`) and returns its
    /// new position, or `None` when nothing moved. Routes name their lane, so they follow it.
    pub fn move_lane(&mut self, from: usize, before: usize) -> Option<usize> {
        let to = destination(self.lanes.len(), from, before)?;
        let lane = self.lanes.remove(from);
        self.lanes.insert(to, lane);
        Some(to)
    }

    /// Inserts a development lane into the gap before position `before` (the end when out of
    /// range), named `name` or its first free numbered form, with the shared developer profile.
    pub fn insert_lane(&mut self, before: usize, name: &str) -> usize {
        let lane = LanePreset {
            name: unique_name(name, self.lanes.iter().map(|l| l.name.as_str())),
            profile: self.development.profile.clone(),
            ..LanePreset::default()
        };
        let index = before.min(self.lanes.len());
        self.lanes.insert(index, lane);
        index
    }

    /// Renames a lane and every QA route that returns work to it.
    pub fn rename_lane(&mut self, index: usize, name: String) -> Result<(), Error> {
        if self
            .lanes
            .iter()
            .enumerate()
            .any(|(ix, lane)| ix != index && lane.name == name)
        {
            return Err(Error::InvalidInput("Lane names must be unique."));
        }
        let lane = self
            .lanes
            .get_mut(index)
            .ok_or(Error::InvalidInput("This lane no longer exists."))?;
        for step in &mut self.reviews {
            if step.return_lane.as_ref() == Some(&lane.name) {
                step.return_lane = Some(name.clone());
            }
        }
        lane.name = name;
        Ok(())
    }

    /// Removes a lane, keeping at least one, and drops QA routes that pointed at it.
    pub fn remove_lane(&mut self, index: usize) -> Result<(), Error> {
        if self.lanes.len() <= 1 || index >= self.lanes.len() {
            return Err(Error::InvalidInput("A template needs a development lane."));
        }
        let removed = self.lanes.remove(index);
        for step in &mut self.reviews {
            if step.return_lane.as_deref() == Some(&removed.name) {
                step.return_lane = None;
            }
        }
        Ok(())
    }

    /// Removes an optional review, keeping the required QA stage.
    pub fn remove_review(&mut self, index: usize) -> Result<(), Error> {
        if index == self.qa_index() || index >= self.reviews.len() {
            return Err(Error::InvalidInput("A template needs a QA review."));
        }
        let removed = self.reviews.remove(index);
        // The review that started a group hands the start to the next one; left joined, the
        // rest of the group would run with whatever stands before it.
        if !removed.with_previous
            && let Some(next) = self.reviews.get_mut(index)
        {
            next.with_previous = false;
        }
        Ok(())
    }

    /// Sets the development profile; lanes that used the previous default follow it, lanes
    /// with their own profile keep it.
    pub fn set_development_profile(&mut self, profile: Profile) {
        for lane in &mut self.lanes {
            if lane.profile == self.development.profile {
                lane.profile = profile.clone();
            }
        }
        self.development.profile = profile;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_names_skip_taken_ones_ignoring_ascii_case() {
        let taken = ["Plan", "plan 2"];
        assert_eq!(unique_name("Build", taken.into_iter()), "Build");
        assert_eq!(unique_name("PLAN", taken.into_iter()), "PLAN 3");
    }

    fn named(names: &[&str]) -> Library {
        Library {
            templates: names
                .iter()
                .map(|name| Template {
                    name: (*name).into(),
                    ..Template::default()
                })
                .collect(),
            ..Library::default()
        }
    }

    fn names(library: &Library) -> Vec<&str> {
        library.templates.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn a_moved_template_takes_the_place_of_the_one_it_is_moved_onto() {
        let mut library = named(&["A", "B", "C", "D"]);
        assert!(library.move_template("A", "C"));
        assert_eq!(names(&library), ["B", "C", "A", "D"]);
        assert!(library.move_template("D", "B"));
        assert_eq!(names(&library), ["D", "B", "C", "A"]);
        assert!(library.move_template("D", "A"));
        assert_eq!(names(&library), ["B", "C", "A", "D"]);
    }

    #[test]
    fn a_move_onto_itself_or_with_an_unknown_name_changes_nothing() {
        let mut library = named(&["A", "B"]);
        let before = library.clone();
        assert!(!library.move_template("A", "A"));
        assert!(!library.move_template("A", "Gone"));
        assert!(!library.move_template("Gone", "A"));
        assert!(!named(&[]).move_template("A", "B"));
        assert_eq!(library, before);
    }

    #[test]
    fn prevents_silent_overwrite_after_changes_in_another_project() {
        let original = Template::default();
        let mut updated = original.clone();
        updated.planner.instruction.push_str(" Check both.");
        let library = Library {
            templates: vec![updated.clone()],
            ..Library::default()
        };
        assert!(
            library
                .template_slot(Some(&original), &original.name)
                .is_err()
        );
        assert!(
            library
                .template_slot(None, &original.name.to_uppercase())
                .is_err()
        );
        assert_eq!(
            library.template_slot(Some(&updated), "New name").unwrap(),
            Some(0)
        );
    }

    #[test]
    fn lane_renames_and_removals_keep_qa_routes_consistent() {
        let mut t = Template::default();
        let mut api = t.lanes[0].clone();
        api.name = "API".into();
        t.lanes.push(api);
        t.reviews[0].return_lane = Some("API".into());
        assert!(t.rename_lane(1, t.lanes[0].name.clone()).is_err());
        t.rename_lane(1, "Server".into()).unwrap();
        assert_eq!(t.reviews[0].return_lane.as_deref(), Some("Server"));
        t.remove_lane(1).unwrap();
        assert_eq!(t.reviews[0].return_lane, None);
        assert!(t.remove_lane(0).is_err());
        assert!(t.remove_review(t.qa_index()).is_err());
        t.remove_review(0).unwrap();
        assert_eq!(t.qa_index(), 0);
        assert!(t.validate_structure().is_ok());
    }

    fn reviews(t: &Template) -> Vec<&str> {
        t.reviews.iter().map(|s| s.name.as_str()).collect()
    }

    /// QA then one optional review, as recipes were saved before stages carried a role.
    fn unmarked() -> Template {
        let mut t = Template::default();
        t.remove_review(0).unwrap();
        t.reviews[0].review_role = ReviewRole::Unspecified;
        t
    }

    #[test]
    fn a_review_moves_across_qa_and_qa_itself_moves() {
        let mut t = Template::default();
        assert_eq!(t.move_review(2, 0), Some(0));
        assert_eq!(reviews(&t), ["Security & performance", "Code review", "QA"]);
        assert_eq!(t.qa_index(), 2);
        assert_eq!(t.move_review(2, 0), Some(0));
        assert_eq!(reviews(&t), ["QA", "Security & performance", "Code review"]);
        assert_eq!(t.qa_index(), 0);
        assert_eq!(t.move_review(0, 3), Some(2));
        assert_eq!(reviews(&t), ["Security & performance", "Code review", "QA"]);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn a_review_moved_ahead_of_an_unmarked_qa_leaves_it_the_qa_stage() {
        let mut t = unmarked();
        assert_eq!(t.move_review(1, 0), Some(0));
        assert_eq!(reviews(&t), ["Security & performance", "QA"]);
        assert_eq!(t.reviews[1].review_role, ReviewRole::Qa);
        assert_eq!(t.qa_index(), 1);
        assert!(t.remove_review(1).is_err());
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn a_move_into_a_gap_beside_the_item_or_out_of_range_changes_nothing() {
        let mut t = unmarked();
        let before = t.clone();
        for (from, gap) in [(0, 0), (0, 1), (1, 1), (1, 2), (2, 0), (0, 3)] {
            assert_eq!(t.move_review(from, gap), None, "{from} before {gap}");
        }
        assert_eq!(t.move_lane(0, 0), None);
        assert_eq!(t.move_lane(0, 1), None);
        assert_eq!(t.move_lane(1, 0), None);
        // Not even the QA marker: a drag that moved nothing is not an edit.
        assert_eq!(t, before);
    }

    #[test]
    fn a_review_is_inserted_into_the_chosen_gap_under_a_free_name() {
        let mut t = Template::default();
        assert_eq!(
            t.insert_review(0, Step::new("QA", "Check the release notes.")),
            0
        );
        assert_eq!(
            reviews(&t),
            ["QA 2", "Code review", "QA", "Security & performance"]
        );
        assert_eq!(t.qa_index(), 2);
        assert_eq!(t.reviews[0].profile, t.planner.profile);
        assert_eq!(t.insert_review(99, Step::new("Last", "Read it again.")), 4);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn code_review_inserted_anywhere_starts_with_the_qa_profile_read_only() {
        let mut t = unmarked();
        t.reviews[0].profile.model = "qa-model".into();
        let mut code = Step::new("Code review", "Read the diff.");
        code.review_role = ReviewRole::Code;
        assert_eq!(t.insert_review(2, code), 2);
        assert_eq!(t.reviews[2].profile.model, "qa-model");
        assert_eq!(t.reviews[2].profile.mode, None);
        assert_eq!(t.qa_index(), 0);
        assert_eq!(t.reviews[0].review_role, ReviewRole::Qa);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn a_recipe_has_one_qa_wherever_it_stands() {
        let mut t = Template::default();
        t.move_review(2, 0);
        assert!(t.validate_structure().is_ok());
        t.reviews[0].review_role = ReviewRole::Qa;
        assert!(t.validate_structure().is_err());
        // Without a marker the first review is QA, and code review cannot be it.
        let mut unmarked = Template::default();
        unmarked.reviews[1].review_role = ReviewRole::Unspecified;
        assert!(unmarked.validate_structure().is_err());
    }

    /// The reviews by name, one inner list per group that runs together.
    fn rows(t: &Template) -> Vec<Vec<&str>> {
        let groups = t.review_groups().into_iter();
        groups
            .map(|group| group.map(|i| t.reviews[i].name.as_str()).collect())
            .collect()
    }

    #[test]
    fn reviews_put_beside_one_another_form_a_group_that_runs_together() {
        // Code review, QA, Security & performance, each on its own.
        let mut t = Template::default();
        assert_eq!(t.review_groups(), [0..1, 1..2, 2..3]);
        assert_eq!(t.join_review(2, 1, true), Some(2));
        assert_eq!(
            rows(&t),
            [vec!["Code review"], vec!["QA", "Security & performance"]]
        );
        assert_eq!((t.review_group(1), t.review_group(2)), (1..3, 1..3));
        assert_eq!((t.review_row(0), t.review_row(2)), (0, 1));
        assert!(t.review_group(3).is_empty());
        // Before the other one, and a third joins from the row above.
        assert_eq!(t.join_review(2, 1, false), Some(1));
        assert_eq!(t.join_review(0, 2, true), Some(2));
        assert_eq!(
            rows(&t),
            [vec!["Security & performance", "QA", "Code review"]]
        );
        assert_eq!(t.qa_index(), 1);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn a_review_taken_out_of_a_group_leaves_the_others_together() {
        let mut t = Template::default();
        t.join_review(1, 0, true).unwrap();
        t.join_review(2, 1, true).unwrap();
        assert_eq!(
            rows(&t),
            [vec!["Code review", "QA", "Security & performance"]]
        );
        // The first of the group moves to a row below it: the rest still run together.
        assert_eq!(t.move_review(0, 1), Some(2));
        assert_eq!(
            rows(&t),
            [vec!["QA", "Security & performance"], vec!["Code review"]]
        );
        // And one from the middle of a group, to a row above it.
        t.join_review(2, 1, false).unwrap();
        assert_eq!(
            rows(&t),
            [vec!["QA", "Code review", "Security & performance"]]
        );
        assert_eq!(t.move_review(1, 0), Some(0));
        assert_eq!(
            rows(&t),
            [vec!["Code review"], vec!["QA", "Security & performance"]]
        );
    }

    #[test]
    fn removing_the_review_that_starts_a_group_does_not_join_the_rest_to_the_row_above() {
        let mut t = Template::default();
        // Code review, then Security & performance together with QA.
        t.join_review(1, 2, true).unwrap();
        assert_eq!(
            rows(&t),
            [vec!["Code review"], vec!["Security & performance", "QA"]]
        );
        t.remove_review(1).unwrap();
        assert_eq!(rows(&t), [vec!["Code review"], vec!["QA"]]);
        // A member that did not start its group goes without touching the others.
        let mut t = Template::default();
        t.join_review(2, 1, true).unwrap();
        t.join_review(0, 1, false).unwrap();
        t.remove_review(2).unwrap();
        assert_eq!(rows(&t), [vec!["Code review", "QA"]]);
    }

    #[test]
    fn a_review_put_where_it_already_stands_is_not_an_edit() {
        let mut t = unmarked();
        t.join_review(1, 0, true).unwrap();
        let before = t.clone();
        assert_eq!(t.join_review(1, 0, true), None);
        assert_eq!(t.join_review(0, 1, false), None);
        assert_eq!(t.join_review(0, 0, true), None);
        assert_eq!(t.join_review(0, 9, true), None);
        assert_eq!(t.move_review(1, 9), None);
        assert_eq!(t, before);
    }

    #[test]
    fn a_new_review_stands_on_a_row_of_its_own_and_code_review_before_qas_group() {
        let mut t = Template::default();
        t.remove_review(0).unwrap();
        t.join_review(1, 0, true).unwrap();
        assert_eq!(rows(&t), [vec!["QA", "Security & performance"]]);
        let mut code = Step::new("Code review", "Read the diff.");
        code.review_role = ReviewRole::Code;
        code.with_previous = true;
        assert_eq!(t.add_review(code), 0);
        assert_eq!(t.add_review(Step::new("Docs", "Read the docs.")), 3);
        assert_eq!(t.insert_review(1, Step::new("Licenses", "Check them.")), 1);
        assert_eq!(
            rows(&t),
            [
                vec!["Code review"],
                vec!["Licenses"],
                vec!["QA", "Security & performance"],
                vec!["Docs"],
            ]
        );
    }

    #[test]
    fn the_first_review_runs_first_whatever_its_flag_says() {
        let mut t = Template::default();
        t.reviews[0].with_previous = true;
        assert_eq!(t.review_groups(), [0..1, 1..2, 2..3]);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn a_recipe_saved_before_groups_reads_as_one_review_at_a_time() {
        let mut saved = serde_json::to_value(Template::default()).unwrap();
        assert!(saved["reviews"][1].get("with_previous").is_none());
        saved["reviews"][2]["with_previous"] = true.into();
        let grouped: Template = serde_json::from_value(saved).unwrap();
        assert_eq!(grouped.review_groups(), [0..1, 1..3]);
    }

    #[test]
    fn lanes_move_and_insert_by_gap_and_routes_follow_their_lane() {
        let mut t = Template::default();
        assert_eq!(t.insert_lane(0, "Development"), 0);
        t.reviews[0].return_lane = Some("Development".into());
        assert_eq!(t.move_lane(0, 2), Some(1));
        let lanes: Vec<_> = t.lanes.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(lanes, ["Development", "Development 2"]);
        assert_eq!(t.lanes[1].profile, t.development.profile);
        assert_eq!(t.reviews[0].return_lane.as_deref(), Some("Development"));
        assert_eq!(t.insert_lane(99, "API"), 2);
        assert!(t.validate_structure().is_ok());
    }

    #[test]
    fn only_a_recipe_without_a_shared_contract_approves_its_plan_automatically() {
        let mut t = Template::default();
        assert!(!t.auto_approves_plan());
        t.toggle_plan_approval();
        assert!(t.auto_approves_plan());
        // Turning the contract on returns the plan to the user, and it stays with them.
        t.set_shared_contract(true);
        assert_eq!(t.plan_approval, PlanApproval::Ask);
        t.toggle_plan_approval();
        assert_eq!(t.plan_approval, PlanApproval::Ask);
        t.set_shared_contract(false);
        assert!(!t.auto_approves_plan());
        t.toggle_plan_approval();
        t.toggle_plan_approval();
        assert_eq!(t.plan_approval, PlanApproval::Ask);
        // A file edited by hand can hold both; the contract wins.
        t.plan_approval = PlanApproval::Auto;
        t.shared_contract = true;
        assert!(!t.auto_approves_plan());
    }

    #[test]
    fn an_unknown_plan_approval_asks_and_asking_is_not_written() {
        let mut saved = serde_json::to_value(Template::default()).unwrap();
        assert!(saved.get("plan_approval").is_none());
        saved["plan_approval"] = "after_a_delay".into();
        let read: Template = serde_json::from_value(saved).unwrap();
        assert_eq!(read.plan_approval, PlanApproval::Ask);

        let mut auto = Template::default();
        auto.toggle_plan_approval();
        let saved = serde_json::to_value(&auto).unwrap();
        assert_eq!(saved["plan_approval"], "auto");
        assert_eq!(serde_json::from_value::<Template>(saved).unwrap(), auto);
    }

    #[test]
    fn changing_development_profile_preserves_independent_lane_model() {
        let mut t = Template::default();
        let mut custom = t.lanes[0].clone();
        custom.name = "API".into();
        custom.profile.model = "custom".into();
        t.lanes.push(custom.clone());
        let mut profile = t.development.profile.clone();
        profile.model = "new-default".into();
        t.set_development_profile(profile.clone());
        assert_eq!(t.lanes[0].profile, profile);
        assert_eq!(t.lanes[1], custom);
        assert_eq!(t.development.profile, profile);
    }
}
