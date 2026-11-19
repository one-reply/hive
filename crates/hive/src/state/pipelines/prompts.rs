//! Edits to the shared prompt library; `hive_core` keeps linked template stages in step.
use super::*;
use hive_core::pipeline::{Error, Library};

impl Pipelines {
    /// Creates a prompt, or replaces the one with `id`; returns its id.
    pub(crate) fn save_prompt(
        &mut self,
        id: Option<u64>,
        name: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        self.edit_prompts(cx, |library| library.save_prompt(id, name, text))
    }

    /// Adds a copy of a prompt and returns the copy's id.
    pub(crate) fn duplicate_prompt(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Result<u64, String> {
        self.edit_prompts(cx, |library| library.duplicate_prompt(id))
    }

    /// Removes a prompt; stages that followed it keep its text as their own.
    pub(crate) fn delete_prompt(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        self.edit_prompts(cx, |library| library.delete_prompt(id))
    }

    fn edit_prompts<T>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Library) -> Result<T, Error>,
    ) -> Result<T, String> {
        let result = self
            .store
            .update(cx, |store, cx| store.try_edit_pipelines(cx, edit))?;
        cx.notify();
        Ok(result)
    }
}
