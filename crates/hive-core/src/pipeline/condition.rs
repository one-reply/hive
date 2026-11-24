//! When an optional review runs: only for a change that touches the paths it names.
//!
//! A condition is a list of `.gitignore`-style patterns, the syntax of
//! [`IncludePatterns`]. It is judged against the paths the reviewed trees change, so it sees
//! exactly what the reviewer would. A condition Hive cannot judge never skips its review.
use super::{Error, Template};
use crate::worktree::IncludePatterns;

/// The patterns in a condition's text: one per line, blank lines dropped. Fails on the first
/// pattern that cannot be read, such as a negation or a comment.
pub fn path_patterns(text: &str) -> Result<Vec<String>, Error> {
    let lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    lines
        .map(|line| {
            if is_readable(line) {
                Ok(line.to_owned())
            } else {
                Err(Error::InvalidCondition(line.to_owned()))
            }
        })
        .collect()
}

fn is_readable(pattern: &str) -> bool {
    !IncludePatterns::parse(pattern).is_empty()
}

/// Whether a review limited to `patterns` runs for a change of the repository-relative paths
/// `changed`. It also runs when a pattern cannot be read.
pub fn applies(patterns: &[String], changed: &[String]) -> bool {
    if !patterns.iter().all(|pattern| is_readable(pattern)) {
        return true;
    }
    let patterns = IncludePatterns::parse(&patterns.join("\n"));
    changed.iter().any(|path| patterns.covers_file(path))
}

impl Template {
    /// The paths the review at `index` is limited to. `None` for a review that always runs:
    /// one without a condition, and QA whatever its recipe says.
    pub fn review_condition(&self, index: usize) -> Option<&[String]> {
        let step = self.reviews.get(index)?;
        (index != self.qa_index() && !step.only_paths.is_empty()).then_some(&step.only_paths[..])
    }

    /// Whether any review of this recipe is limited to some paths.
    pub fn has_review_conditions(&self) -> bool {
        (0..self.reviews.len()).any(|index| self.review_condition(index).is_some())
    }

    /// Rejects a condition on QA and a pattern this build cannot read. Checked when a recipe
    /// is saved or launched, not when it is loaded: a saved recipe must keep loading.
    pub(super) fn validate_conditions(&self) -> Result<(), Error> {
        let qa = self.qa_index();
        if self
            .reviews
            .get(qa)
            .is_some_and(|step| !step.only_paths.is_empty())
        {
            return Err(Error::InvalidInput(
                "The QA review always runs. Remove its path condition.",
            ));
        }
        let unreadable = self
            .reviews
            .iter()
            .flat_map(|step| &step.only_paths)
            .find(|pattern| !is_readable(pattern));
        match unreadable {
            Some(pattern) => Err(Error::InvalidCondition(pattern.clone())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Template, applies, path_patterns};

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|path| (*path).to_owned()).collect()
    }

    #[test]
    fn a_folder_pattern_covers_files_at_any_depth_inside_it() {
        let changed = paths(&["crates/auth/src/token/refresh.rs", "README.md"]);
        for pattern in [
            "crates/auth",
            "crates/auth/",
            "crates/auth/**",
            "auth",
            "token/",
        ] {
            assert!(applies(&paths(&[pattern]), &changed), "{pattern}");
        }
        for pattern in ["crates/billing", "auth.rs", "src/token", "*.sql"] {
            assert!(!applies(&paths(&[pattern]), &changed), "{pattern}");
        }
    }

    #[test]
    fn a_file_pattern_matches_by_name_or_by_anchored_path() {
        let changed = paths(&["db/migrations/0042_users.sql", "src/lib.rs"]);
        assert!(applies(&paths(&["*.sql"]), &changed));
        assert!(applies(&paths(&["db/migrations/*.sql"]), &changed));
        assert!(applies(&paths(&["/src/lib.rs"]), &changed));
        assert!(!applies(&paths(&["migrations/*.sql"]), &changed));
        // Any one pattern is enough, and no change at all matches nothing.
        assert!(applies(&paths(&["*.proto", "*.sql"]), &changed));
        assert!(!applies(&paths(&["*.sql"]), &[]));
    }

    #[test]
    fn a_pattern_that_cannot_be_read_never_skips_the_review() {
        let changed = paths(&["docs/guide.md"]);
        for pattern in ["!docs", "# docs", "/"] {
            assert!(applies(&paths(&["*.sql", pattern]), &changed), "{pattern}");
        }
    }

    #[test]
    fn a_conditions_text_is_one_readable_pattern_per_line() {
        assert_eq!(
            path_patterns("  crates/auth/ \n\n*.sql\n   \n").unwrap(),
            ["crates/auth/", "*.sql"]
        );
        assert!(path_patterns(" \n").unwrap().is_empty());
        assert!(matches!(
            path_patterns("src/\n!src/generated\n"),
            Err(Error::InvalidCondition(pattern)) if pattern == "!src/generated"
        ));
    }

    #[test]
    fn qa_always_runs_and_only_readable_conditions_are_saved() {
        // Code review, QA, Security & performance.
        let mut recipe = Template::default();
        assert!(!recipe.has_review_conditions());
        recipe.reviews[2].only_paths = paths(&["crates/auth/"]);
        assert_eq!(
            recipe.review_condition(2),
            Some(&paths(&["crates/auth/"])[..])
        );
        assert_eq!(recipe.review_condition(0), None);
        assert_eq!(recipe.review_condition(9), None);
        assert!(recipe.has_review_conditions());
        assert!(recipe.validate_conditions().is_ok());

        recipe.reviews[2].only_paths.push("!vendor".into());
        assert!(matches!(
            recipe.validate_conditions(),
            Err(Error::InvalidCondition(pattern)) if pattern == "!vendor"
        ));
        // The recipe still loads: only saving and launching refuse it.
        assert!(recipe.validate_structure().is_ok());

        recipe.reviews[2].only_paths.pop();
        recipe.reviews[1].only_paths = paths(&["src/"]);
        assert_eq!(recipe.review_condition(1), None);
        assert!(matches!(
            recipe.validate_conditions(),
            Err(Error::InvalidInput(_))
        ));
    }
}
