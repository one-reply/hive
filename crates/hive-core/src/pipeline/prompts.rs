//! Reusable stage prompts and the links that keep template stages in step with them.
//!
//! A linked stage mirrors its prompt's text in `Step::instruction`, so everything that reads
//! a stage's instructions, including a run's frozen copy of its template, needs no lookup.
use super::{Error, Library, Template, recipe::unique_name};
use serde::{Deserialize, Serialize};

/// First id given to a prompt the user creates. Built-in prompts own the ids below it, so
/// deleting one never frees its id for an unrelated prompt.
pub(crate) const FIRST_USER_ID: u64 = 1000;

/// A named instruction text that template stages follow by id.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prompt {
    /// Stable identity: renaming a prompt keeps the stages linked to it.
    pub id: u64,
    /// Name shown in Settings and in the stage inspector.
    pub name: String,
    /// Instructions given to the stage's agent.
    pub text: String,
}

impl Prompt {
    /// Whether a stored prompt has the name and text every library entry needs.
    pub(crate) fn is_complete(&self) -> bool {
        !self.name.trim().is_empty() && !self.text.trim().is_empty()
    }
}

impl Library {
    /// Reusable prompts in display order.
    pub fn prompts(&self) -> &[Prompt] {
        &self.prompts
    }

    /// The prompt with this id, if the library still has it.
    pub fn prompt(&self, id: u64) -> Option<&Prompt> {
        self.prompts.iter().find(|p| p.id == id)
    }

    /// Creates a prompt, or replaces the one with `id`, and gives every template stage linked
    /// to it the new text. Returns the prompt's id. Runs keep the text they started with.
    pub fn save_prompt(&mut self, id: Option<u64>, name: &str, text: &str) -> Result<u64, Error> {
        let (name, text) = (name.trim(), text.trim());
        if name.is_empty() {
            return Err(Error::InvalidInput("Give the prompt a name."));
        }
        if text.is_empty() {
            return Err(Error::InvalidInput("Write the prompt's instructions."));
        }
        if self
            .prompts
            .iter()
            .any(|p| Some(p.id) != id && p.name.eq_ignore_ascii_case(name))
        {
            return Err(Error::InvalidInput("Choose a unique prompt name."));
        }
        let id = match id {
            Some(id) => {
                let prompt = self.prompt_mut(id)?;
                prompt.name = name.into();
                prompt.text = text.into();
                id
            }
            None => {
                let id = self.next_prompt_id()?;
                self.prompts.push(Prompt {
                    id,
                    name: name.into(),
                    text: text.into(),
                });
                id
            }
        };
        self.sync_prompts();
        Ok(id)
    }

    /// Adds a copy of a prompt under a free "… copy" name and returns the copy's id.
    pub fn duplicate_prompt(&mut self, id: u64) -> Result<u64, Error> {
        let source = self.prompt_mut(id)?.clone();
        let name = unique_name(
            &format!("{} copy", source.name),
            self.prompts.iter().map(|p| p.name.as_str()),
        );
        self.save_prompt(None, &name, &source.text)
    }

    /// Removes a prompt. Stages linked to it keep its text as their own instructions.
    pub fn delete_prompt(&mut self, id: u64) -> Result<(), Error> {
        self.prompt_mut(id)?;
        self.prompts.retain(|p| p.id != id);
        self.sync_prompts();
        Ok(())
    }

    /// Number of template stages linked to a prompt.
    pub fn prompt_uses(&self, id: u64) -> usize {
        self.templates
            .iter()
            .flat_map(Template::steps)
            .filter(|step| step.prompt == Some(id))
            .count()
    }

    /// Restores the link invariant in every template; runs are frozen and never touched.
    pub(crate) fn sync_prompts(&mut self) {
        for template in &mut self.templates {
            template.sync_prompts(&self.prompts);
        }
    }

    fn prompt_mut(&mut self, id: u64) -> Result<&mut Prompt, Error> {
        self.prompts
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(Error::InvalidInput(
                "This prompt was deleted. Reopen the prompt list.",
            ))
    }

    fn next_prompt_id(&self) -> Result<u64, Error> {
        let Some(highest) = self.prompts.iter().map(|p| p.id).max() else {
            return Ok(FIRST_USER_ID);
        };
        // Only a hand-edited file can hold the largest id.
        let next = highest
            .checked_add(1)
            .ok_or(Error::InvalidInput("The prompt library is full."))?;
        Ok(next.max(FIRST_USER_ID))
    }
}

impl Template {
    /// Makes every linked stage mirror its prompt's current text. A stage whose prompt is gone
    /// keeps the text it has as its own instructions.
    pub fn sync_prompts(&mut self, prompts: &[Prompt]) {
        for step in self.steps_mut() {
            let Some(id) = step.prompt else {
                continue;
            };
            match prompts.iter().find(|p| p.id == id) {
                Some(prompt) if step.instruction != prompt.text => {
                    step.instruction.clone_from(&prompt.text);
                }
                Some(_) => {}
                None => step.prompt = None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Lane, Run};

    fn linked_planner(library: &Library) -> u64 {
        library.templates[0]
            .planner
            .prompt
            .expect("starter recipes link their stages")
    }

    #[test]
    fn saving_a_prompt_updates_every_linked_template_stage_and_no_run() {
        let mut library = Library::default();
        let id = linked_planner(&library);
        let mut copy = library.templates[0].clone();
        copy.name = "Second".into();
        library.templates.push(copy);
        let lane = Lane {
            name: library.templates[0].lanes[0].name.clone(),
            project: "/project".into(),
            worktree: "/project".into(),
            base: "HEAD".into(),
            ..Default::default()
        };
        let run = Run::new(
            1,
            "/owner".into(),
            "Task".into(),
            library.templates[0].clone(),
            vec![lane],
        )
        .unwrap();
        library.runs.push(run);
        let frozen = library.runs.clone();

        assert_eq!(
            library
                .save_prompt(Some(id), "Planner", " New text ")
                .unwrap(),
            id
        );

        assert_eq!(library.prompt(id).unwrap().text, "New text");
        assert_eq!(library.templates[0].planner.instruction, "New text");
        assert_eq!(
            library.templates.last().unwrap().planner.instruction,
            "New text"
        );
        assert_eq!(library.runs, frozen);
    }

    #[test]
    fn renaming_a_prompt_keeps_its_links() {
        let mut library = Library::default();
        let id = linked_planner(&library);
        let text = library.prompt(id).unwrap().text.clone();
        let uses = library.prompt_uses(id);
        assert!(uses > 0);
        library.save_prompt(Some(id), "Renamed", &text).unwrap();
        assert_eq!(library.prompt(id).unwrap().name, "Renamed");
        assert_eq!(library.prompt_uses(id), uses);
        assert_eq!(library.templates[0].planner.prompt, Some(id));
    }

    #[test]
    fn deleting_a_prompt_leaves_linked_stages_as_custom_text() {
        let mut library = Library::default();
        let id = linked_planner(&library);
        let text = library.templates[0].planner.instruction.clone();
        library.delete_prompt(id).unwrap();
        assert!(library.prompt(id).is_none());
        assert_eq!(library.templates[0].planner.prompt, None);
        assert_eq!(library.templates[0].planner.instruction, text);
        assert_eq!(library.prompt_uses(id), 0);
        assert!(library.delete_prompt(id).is_err());
    }

    #[test]
    fn rejects_blank_or_duplicate_names_without_changing_the_library() {
        let mut library = Library::default();
        let original = library.clone();
        let taken = library.prompts()[1].name.to_uppercase();
        let id = library.prompts()[0].id;
        assert!(library.save_prompt(None, "  ", "Text").is_err());
        assert!(library.save_prompt(None, "Name", " \n ").is_err());
        assert!(library.save_prompt(None, &taken, "Text").is_err());
        assert!(library.save_prompt(Some(id), &taken, "Text").is_err());
        assert!(library.save_prompt(Some(u64::MAX), "Gone", "Text").is_err());
        assert_eq!(library, original);
    }

    #[test]
    fn new_prompt_ids_never_reuse_a_built_in_id() {
        let mut library = Library::default();
        let built_in = library.prompts().iter().map(|p| p.id).max().unwrap();
        assert!(built_in < FIRST_USER_ID);
        library.prompts.clear();
        let first = library.save_prompt(None, "Mine", "Text").unwrap();
        assert_eq!(first, FIRST_USER_ID);
        let copy = library.duplicate_prompt(first).unwrap();
        assert_eq!(copy, FIRST_USER_ID + 1);
        assert_eq!(library.prompt(copy).unwrap().name, "Mine copy");
        assert_eq!(library.duplicate_prompt(first).unwrap(), FIRST_USER_ID + 2);
        assert_eq!(library.prompts().last().unwrap().name, "Mine copy 2");
    }

    #[test]
    fn a_library_holding_the_largest_id_reports_that_it_is_full() {
        let mut library = Library::default();
        library.prompts[0].id = u64::MAX;
        assert!(library.save_prompt(None, "Mine", "Text").is_err());
    }

    #[test]
    fn default_library_is_unchanged_by_sync() {
        let mut library = Library::default();
        let original = library.clone();
        library.sync_prompts();
        assert_eq!(library, original);
        assert!(
            library
                .templates
                .iter()
                .flat_map(Template::steps)
                .all(|step| step.prompt.is_some())
        );
    }
}
