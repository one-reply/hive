use super::*;
use hive_core::pipeline::draft::LaunchDraft;

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

    pub(super) fn save_draft(&self, cx: &mut Context<Self>) {
        if !self.keep_draft {
            return;
        }
        let draft = LaunchDraft {
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
        };
        let store = AppState::get(cx).store.clone();
        if store
            .read(cx)
            .state
            .pipelines
            .drafts
            .iter()
            .find(|d| d.owner == self.project)
            == Some(&draft)
        {
            return;
        }
        store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |lib| {
                if let Some(old) = lib.drafts.iter_mut().find(|d| d.owner == self.project) {
                    *old = draft;
                } else {
                    lib.drafts.push(draft);
                }
            })
        });
    }
}
