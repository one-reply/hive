//! Template drafts outlive project switches and may be open in two projects at once.
use super::*;
use hive_core::pipeline::{Library, unique_name};

impl Pipelines {
    pub(crate) fn duplicate_template(
        &mut self,
        source: &Template,
        cx: &mut Context<Self>,
    ) -> Result<Template, String> {
        let library = &self.store.read(cx).state.pipelines;
        library.template_index(source)?;
        let mut copy = source.clone();
        copy.name = unique_name(
            &format!("{} copy", source.name),
            library.templates.iter().map(|t| t.name.as_str()),
        );
        self.save_template(None, copy.clone(), cx)?;
        Ok(copy)
    }

    pub(crate) fn delete_template(
        &mut self,
        original: &Template,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let index = self
            .store
            .read(cx)
            .state
            .pipelines
            .template_index(original)?;
        self.store.update(cx, |store, cx| {
            store.edit_pipelines(cx, |library| {
                library.templates.remove(index);
            });
        });
        cx.notify();
        Ok(())
    }

    pub(crate) fn save_template(
        &mut self,
        original: Option<&Template>,
        draft: Template,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        draft.validate().map_err(|e| e.to_string())?;
        let library = &self.store.read(cx).state.pipelines;
        // The prompt library may have changed while the draft was open: a linked stage takes
        // the current text, and the loaded copy is brought to what the library now holds.
        let mut draft = draft;
        draft.sync_prompts(library.prompts());
        let original = original.map(|loaded| {
            let mut current = loaded.clone();
            current.sync_prompts(library.prompts());
            current
        });
        let slot = library.template_slot(original.as_ref(), &draft.name)?;
        self.store.update(cx, |s, cx| {
            s.edit_pipelines(cx, |library: &mut Library| match slot {
                Some(index) => library.templates[index] = draft,
                None => library.templates.push(draft),
            })
        });
        cx.notify();
        Ok(())
    }
}
