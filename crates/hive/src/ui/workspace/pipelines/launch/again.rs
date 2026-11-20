//! Starting a new run from an earlier one: the same task and repositories, a new branch.
use super::*;

impl LaunchForm {
    /// Fills the form in from `run`. Every lane still needs a branch: the earlier run's
    /// branch is open in its worktree, and Git does not open a branch twice.
    pub(in crate::ui::workspace::pipelines) fn repeat(
        &mut self,
        run: &Run,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.frozen = true;
        self.previous_recipe = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .templates
            .iter()
            .find(|t| t.name == run.template().name && *t != run.template())
            .map(|latest| (run.template().clone(), latest.clone()));
        self.direct = run.direct_start();
        if self.direct
            && let Some(lane) = run.lanes().first()
        {
            self.criteria
                .update(cx, |c, cx| c.set_value(&lane.criteria, window, cx));
        }
        self.task
            .update(cx, |task, cx| task.set_value(run.task(), window, cx));
        let registered = repositories(cx);
        let recipe_lanes = self.template.lanes.len();
        // A recipe lane the earlier run left out stays out; the user can bind it again.
        for binding in &mut self.bindings {
            if !run
                .lanes()
                .iter()
                .any(|lane| lane.name == binding.preset.name)
            {
                binding.bind(None, window, cx);
            }
        }
        for lane in run.lanes() {
            let known = registered.iter().any(|(path, _)| path == &lane.project);
            let index = match self
                .bindings
                .iter()
                .position(|binding| binding.preset.name == lane.name)
            {
                Some(index) => index,
                None => {
                    let preset = LanePreset {
                        name: lane.name.clone(),
                        profile: lane.profile.clone(),
                        edits: lane.edits,
                        ..Default::default()
                    };
                    self.bindings.push(Binding::new(preset, None, window, cx));
                    self.bindings.len() - 1
                }
            };
            // A project removed from Hive since then cannot be bound; a lane the recipe
            // does not have would only be an empty card.
            if known {
                self.bindings[index].restore(lane, window, cx);
            } else if index >= recipe_lanes {
                self.bindings.remove(index);
            }
        }
        for binding in &self.bindings {
            binding.preview(cx);
        }
        cx.notify();
    }
}
