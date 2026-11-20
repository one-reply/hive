//! The unsubmitted form as a draft: saved once the user has changed it, restored on return.
use super::*;
use hive_core::pipeline::draft::LaunchDraft;
use std::time::Duration;

/// How long edits collect before they are written. Every save clones the whole saved state
/// and redraws its observers, which is too much to do on each keystroke.
const SAVE_DELAY: Duration = Duration::from_millis(400);

/// What a form's content means for the draft the store holds for its project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DraftChange {
    Keep,
    Save,
    Discard,
}

/// A form left as it opened holds nothing to resume, so it is not a draft; one the user
/// returned to that state stops being one.
fn draft_change(
    current: &LaunchDraft,
    pristine: &LaunchDraft,
    stored: Option<&LaunchDraft>,
) -> DraftChange {
    match stored {
        Some(_) if current == pristine => DraftChange::Discard,
        Some(stored) if stored == current => DraftChange::Keep,
        None if current == pristine => DraftChange::Keep,
        _ => DraftChange::Save,
    }
}

impl LaunchForm {
    pub(in crate::ui::workspace::pipelines) fn restore_draft(
        &mut self,
        saved: LaunchDraft,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.template = saved.template;
        self.previous_recipe = saved.original.and_then(|original| {
            let latest = AppState::get(cx)
                .store
                .read(cx)
                .state
                .pipelines
                .templates
                .iter()
                .find(|t| t.name == original.name && **t != original)?
                .clone();
            Some((original, latest))
        });
        self.direct = saved.direct;
        self.frozen = saved.frozen;
        self.task
            .update(cx, |input, cx| input.set_value(saved.task, w, cx));
        self.criteria
            .update(cx, |input, cx| input.set_value(saved.criteria, w, cx));
        self.bindings = saved
            .lanes
            .into_iter()
            .map(|lane| Binding::from_draft(lane, w, cx))
            .collect();
        cx.notify();
    }

    /// The form as it would be saved now.
    pub(super) fn snapshot(&self, cx: &App) -> LaunchDraft {
        LaunchDraft {
            owner: self.project.clone(),
            template: self.template.clone(),
            original: self
                .previous_recipe
                .as_ref()
                .map(|(saved, _)| saved.clone())
                .or_else(|| self.frozen.then(|| self.template.clone())),
            frozen: self.frozen,
            task: self.task.read(cx).value().to_string(),
            direct: self.direct,
            criteria: self.criteria.read(cx).value().to_string(),
            lanes: self.bindings.iter().map(|b| b.draft(cx)).collect(),
        }
    }

    /// Whether the user has changed anything since the form opened.
    pub(in crate::ui::workspace::pipelines) fn is_modified(&self, cx: &App) -> bool {
        self.snapshot(cx) != self.pristine
    }

    pub(in crate::ui::workspace::pipelines) fn template(&self) -> &Template {
        &self.template
    }

    /// Stops saving: the form was submitted, discarded or replaced.
    pub(in crate::ui::workspace::pipelines) fn abandon(&mut self) {
        self.keep_draft = false;
        self.draft_save = None;
    }

    /// Writes the draft once edits pause. A save already waiting covers this change too.
    // ponytail: edits made within SAVE_DELAY of quitting are not saved; flush from the
    // app's quit hook if drafts must survive that.
    pub(super) fn schedule_draft_save(&mut self, cx: &mut Context<Self>) {
        if !self.keep_draft || self.draft_save.is_some() {
            return;
        }
        self.draft_save = Some(cx.spawn(async move |form, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = form.update(cx, |form, cx| {
                form.draft_save = None;
                form.save_draft(cx);
            });
        }));
    }

    fn save_draft(&self, cx: &mut Context<Self>) {
        if !self.keep_draft {
            return;
        }
        let draft = self.snapshot(cx);
        let store = AppState::get(cx).store.clone();
        let change = draft_change(
            &draft,
            &self.pristine,
            store
                .read(cx)
                .state
                .pipelines
                .drafts
                .iter()
                .find(|d| d.owner == self.project),
        );
        if change == DraftChange::Keep {
            return;
        }
        store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |lib| {
                lib.drafts.retain(|d| d.owner != self.project);
                if change == DraftChange::Save {
                    lib.drafts.push(draft);
                }
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{DraftChange, LaunchDraft, Template, draft_change};

    fn draft(task: &str) -> LaunchDraft {
        LaunchDraft {
            owner: "/web".into(),
            template: Template::default(),
            original: None,
            frozen: false,
            task: task.into(),
            direct: false,
            criteria: String::new(),
            lanes: vec![],
        }
    }

    #[test]
    fn an_untouched_form_is_never_saved_and_an_edited_one_is_saved_once() {
        let (pristine, edited) = (draft(""), draft("Fix login"));
        assert_eq!(draft_change(&pristine, &pristine, None), DraftChange::Keep);
        assert_eq!(draft_change(&edited, &pristine, None), DraftChange::Save);
        assert_eq!(
            draft_change(&edited, &pristine, Some(&edited)),
            DraftChange::Keep
        );
        assert_eq!(
            draft_change(&draft("Fix login now"), &pristine, Some(&edited)),
            DraftChange::Save
        );
    }

    #[test]
    fn a_form_returned_to_how_it_opened_drops_its_saved_draft() {
        let (pristine, edited) = (draft(""), draft("Fix login"));
        assert_eq!(
            draft_change(&pristine, &pristine, Some(&edited)),
            DraftChange::Discard
        );
        // An empty draft saved by a build that saved every opened form.
        assert_eq!(
            draft_change(&pristine, &pristine, Some(&pristine)),
            DraftChange::Discard
        );
    }
}
