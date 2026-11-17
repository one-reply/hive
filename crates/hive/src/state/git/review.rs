//! On-demand branch comparisons and paged commit history.
use super::*;

/// What the review sections show for one worktree: the user's choices, not the Git data
/// behind them, which is read again when the worktree is selected.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ReviewView {
    section: Section,
    base: Option<String>,
    selected_commit: Option<Commit>,
}

/// The base to compare with once the branch list is known: the chosen one while its branch
/// still exists, otherwise the repository's default.
pub(super) fn resolve_base(
    chosen: Option<String>,
    default: Option<String>,
    branches: &[Branch],
) -> Option<String> {
    chosen
        .filter(|base| branches.iter().any(|branch| branch.reference == *base))
        .or(default)
}

impl Git {
    /// Parks the review choices of `previous` and shows the ones `target` had; a worktree
    /// seen for the first time starts in Changes.
    pub(super) fn follow_review(&mut self, previous: Option<&Target>) {
        let shown = ReviewView {
            section: self.section,
            base: self.base.take(),
            selected_commit: self.selected_commit.take(),
        };
        let view = self
            .review_views
            .switch(previous, self.target.as_ref(), shown);
        self.section = view.section;
        self.base = view.base;
        self.selected_commit = view.selected_commit;
    }

    pub fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self._review = None;
        self.review_loading = false;
        self.query_generation += 1;
        self.section = section;
        self.selected_commit = None;
        self.files.clear();
        self.load_review(false, cx);
        cx.notify();
    }

    pub fn set_base(&mut self, base: String, cx: &mut Context<Self>) {
        self.files.clear();
        self.base = Some(base);
        self.section = Section::Compare;
        self.selected_commit = None;
        self.load_review(false, cx);
        cx.notify();
    }

    /// Show the files of `commit` in History, from the list or from elsewhere (inline blame).
    pub fn select_commit(&mut self, commit: Commit, cx: &mut Context<Self>) {
        self.files.clear();
        self.section = Section::History;
        self.selected_commit = Some(commit);
        self.load_review(false, cx);
        cx.notify();
    }

    pub fn load_review(&mut self, more: bool, cx: &mut Context<Self>) {
        self._review = None;
        self.review_loading = false;
        self.query_generation += 1;
        let Some(target) = self.target.clone() else {
            return;
        };
        let Some(head) = self.status.as_ref().and_then(|s| s.oid.clone()) else {
            self.commits.clear();
            self.files.clear();
            return;
        };
        if self.section == Section::Changes {
            return;
        }
        let generation = self.query_generation;
        let section = self.section;
        let base = self.base.clone();
        let commit = self.selected_commit.as_ref().map(|c| c.oid.clone());
        let skip = if more { self.commits.len() } else { 0 };
        self.review_loading = true;
        self._review = Some(cx.spawn(async move |this, cx| {
            let t = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if section == Section::Compare {
                        let files = base
                            .map(|b| git::history::compare_files(&t.host, &t.path, &b))
                            .transpose()?
                            .unwrap_or_default();
                        Ok::<_, git::GitError>((None, files))
                    } else if let Some(commit) = commit {
                        Ok((None, git::history::commit_files(&t.host, &t.path, &commit)?))
                    } else {
                        Ok((
                            Some(git::history::read(&t.host, &t.path, &head, skip)?),
                            vec![],
                        ))
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&target) || generation != this.query_generation {
                    return;
                }
                this.review_loading = false;
                match result {
                    Ok((commits, files)) => {
                        this.files = files;
                        if let Some(commits) = commits {
                            this.history_more = commits.len() == 50;
                            if !more {
                                this.commits.clear();
                            }
                            this.commits.extend(commits);
                        }
                    }
                    Err(error) => {
                        this.files.clear();
                        this.report(&target, error, cx);
                    }
                }
                cx.notify();
            });
        }));
    }
}

impl Git {
    pub(crate) fn add_note(
        &mut self,
        target: Target,
        note: git::review_notes::Note,
        cx: &mut Context<Self>,
    ) {
        if !note.text.trim().is_empty() {
            self.review_notes.entry(target).or_default().push(note);
            cx.notify();
        }
    }

    pub(crate) fn notes(&self, target: &Target) -> &[git::review_notes::Note] {
        self.review_notes.get(target).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn clear_notes(&mut self, target: &Target, cx: &mut Context<Self>) {
        self.review_notes.remove(target);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_base;
    use hive_core::git::branches::Branch;

    fn branch(reference: &str) -> Branch {
        Branch {
            reference: reference.into(),
            name: reference.rsplit('/').next().unwrap_or(reference).into(),
            remote: reference.starts_with("refs/remotes/"),
            worktree: None,
        }
    }

    #[test]
    fn a_chosen_base_is_kept_while_its_branch_exists() {
        let branches = [branch("refs/heads/release"), branch("refs/heads/main")];
        assert_eq!(
            resolve_base(
                Some("refs/heads/release".into()),
                Some("refs/heads/main".into()),
                &branches
            )
            .as_deref(),
            Some("refs/heads/release")
        );
    }

    #[test]
    fn a_base_whose_branch_is_gone_gives_way_to_the_default() {
        let branches = [branch("refs/heads/main")];
        assert_eq!(
            resolve_base(
                Some("refs/heads/release".into()),
                Some("refs/heads/main".into()),
                &branches
            )
            .as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(
            resolve_base(Some("refs/heads/release".into()), None, &[]),
            None
        );
    }

    #[test]
    fn no_choice_takes_the_default() {
        let branches = [branch("refs/heads/main")];
        assert_eq!(
            resolve_base(None, Some("refs/heads/main".into()), &branches).as_deref(),
            Some("refs/heads/main")
        );
    }
}
