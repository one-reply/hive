//! On-demand branch comparisons and paged commit history.
use super::*;

impl Git {
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

    pub fn select_commit(&mut self, oid: String, cx: &mut Context<Self>) {
        self.files.clear();
        self.selected_commit = Some(oid);
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
        let commit = self.selected_commit.clone();
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
