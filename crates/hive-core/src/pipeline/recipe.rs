//! Editing rules shared by every template editor: names stay unique, routes follow renames.
use super::{Error, Library, Profile, Template};
use std::collections::HashSet;

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
}

impl Template {
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

    /// Removes a QA review other than the first, which every template keeps.
    pub fn remove_review(&mut self, index: usize) -> Result<(), Error> {
        if index == 0 || index >= self.reviews.len() {
            return Err(Error::InvalidInput("A template needs a QA review."));
        }
        self.reviews.remove(index);
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
        assert!(t.remove_review(0).is_err());
        assert!(t.validate_structure().is_ok());
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
