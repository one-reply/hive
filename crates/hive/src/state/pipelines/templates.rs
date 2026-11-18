//! Template drafts outlive project switches and may be open in two projects at once.
use super::*;
use hive_core::pipeline::Library;

impl Pipelines {
    pub(crate) fn save_template(
        &mut self,
        original: Option<&Template>,
        draft: Template,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        draft.validate().map_err(|e| e.to_string())?;
        let library = &self.store.read(cx).state.pipelines;
        let slot = template_slot(&library.templates, original, &draft.name)?;
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
fn template_slot(
    saved: &[Template],
    original: Option<&Template>,
    name: &str,
) -> Result<Option<usize>, String> {
    let slot = original
        .map(|before| {
            saved.iter().position(|t| t == before).ok_or_else(|| {
                "This template changed in another project. Reopen it before saving.".to_owned()
            })
        })
        .transpose()?;
    if saved
        .iter()
        .enumerate()
        .any(|(i, t)| Some(i) != slot && t.name.eq_ignore_ascii_case(name))
    {
        return Err("Choose a unique template name.".into());
    }
    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::{Template, template_slot};
    #[test]
    fn prevents_silent_overwrite_after_changes_in_another_project() {
        let original = Template::default();
        let mut updated = original.clone();
        updated.planner.instruction.push_str(" Check both.");
        assert!(
            template_slot(
                std::slice::from_ref(&updated),
                Some(&original),
                &original.name
            )
            .is_err()
        );
        assert!(
            template_slot(
                std::slice::from_ref(&updated),
                None,
                &original.name.to_uppercase()
            )
            .is_err()
        );
        assert_eq!(
            template_slot(std::slice::from_ref(&updated), Some(&updated), "New name").unwrap(),
            Some(0)
        );
    }
}
