//! Starting the run. Every repository is read again first, so the run starts from what Git
//! holds now and not from what its card read when it was last edited.
use super::*;
use hive_core::pipeline::draft::LaunchDraft;
use readiness::read_failure;

/// A start the user asked for that waits for the repositories to be read again.
pub(super) struct PendingStart {
    /// The form as the user confirmed it.
    form: LaunchDraft,
    /// What the card of each bound repository showed, in the order of the bindings.
    shown: Vec<Shown>,
}

/// What the form showed of one repository when the user pressed Start, as far as it decides
/// what the run does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Shown {
    /// The warning that the checkout has local changes the run will not include.
    pub(super) local_changes: bool,
    /// The command that must pass before acceptance, empty for agent review only.
    pub(super) check: String,
}

/// Something a fresh read brought that the user has not seen: the run would no longer be the
/// one they confirmed. A base commit that moved is not one, the run starts from the new commit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Change {
    LocalChanges,
    Check(String),
}

impl Change {
    fn message(&self, repository: &str) -> String {
        match self {
            Self::LocalChanges => format!(
                "{repository} now has local changes, and the run uses committed files only. \
                 Start again to go on without them."
            ),
            Self::Check(command) if command.is_empty() => format!(
                "The check suggested for {repository} is gone. Start again to rely on agent \
                 review, or choose a command."
            ),
            Self::Check(command) => format!(
                "The check suggested for {repository} is now “{command}”. Start again to use it."
            ),
        }
    }
}

fn unseen_change(seen: &Shown, fresh: &Shown) -> Option<Change> {
    if fresh.local_changes && !seen.local_changes {
        return Some(Change::LocalChanges);
    }
    (fresh.check != seen.check).then(|| Change::Check(fresh.check.clone()))
}

/// Whether ⌘⏎ starts the run. An open branch menu keeps the key: there it confirms the name
/// being typed, and a start would run on the branch chosen before.
pub(super) fn starts_run(secondary: bool, blocked: bool, branch_menu_open: bool) -> bool {
    secondary && !blocked && !branch_menu_open
}

impl LaunchForm {
    /// The first thing that keeps the run from starting, `None` once it can.
    pub(super) fn blocked_by(&self, cx: &App) -> Option<Blocker> {
        let lanes: Vec<_> = self.bindings.iter().filter_map(|b| b.state(cx)).collect();
        let has_task = !self.task.read(cx).value().trim().is_empty();
        let missing_criteria = self.is_direct() && self.criteria.read(cx).value().trim().is_empty();
        blocker(has_task, &lanes, missing_criteria)
    }

    /// What to tell the user about `blocker`. A failed read is named with its repository and
    /// what Git answered, so the reason says what to fix.
    pub(super) fn blocker_message(&self, blocker: Blocker, cx: &App) -> SharedString {
        if blocker != Blocker::Failed {
            return blocker.message().into();
        }
        let failed = self
            .bindings
            .iter()
            .find(|binding| binding.state(cx) == Some(LaneState::Failed));
        match failed.map(|binding| (binding, binding.base(cx))) {
            Some((binding, Preview::Failed(failure))) => {
                read_failure(&self.repository_name(binding, cx), failure).into()
            }
            _ => blocker.message().into(),
        }
    }

    fn repository_name(&self, binding: &Binding, cx: &App) -> String {
        let Some(project) = &binding.project else {
            return binding.preset.name.clone();
        };
        repositories(cx)
            .into_iter()
            .find(|(path, _)| path == project)
            .map_or_else(|| path_name(project), |(_, name)| name)
    }

    pub(super) fn is_starting(&self) -> bool {
        self.starting.is_some()
    }

    /// Reads every repository again; `resume_start` goes on once the answers are in.
    pub(super) fn request_start(&mut self, cx: &mut Context<Self>) {
        if self.starting.is_some() || self.blocked_by(cx).is_some() {
            return;
        }
        self.stop_dictation(cx);
        let shown = self
            .bindings
            .iter()
            .filter_map(|binding| Some(binding.shown(binding.ready(cx)?, cx)))
            .collect();
        self.starting = Some(PendingStart {
            form: self.snapshot(cx),
            shown,
        });
        self.error = None;
        for binding in &self.bindings {
            binding.read(cx);
        }
        cx.notify();
    }

    /// Runs whenever a read finishes. Starts the run the user confirmed, or says what the
    /// fresh read changed and leaves the form, which now shows it, for another look.
    pub(super) fn resume_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.starting.is_none() || self.bindings.iter().any(|b| b.is_reading(cx)) {
            return;
        }
        let Some(pending) = self.starting.take() else {
            return;
        };
        cx.notify();
        match self.recheck(&pending, cx) {
            Ok(()) => self.submit(window, cx),
            Err(reason) => self.error = Some(reason),
        }
    }

    fn recheck(&self, pending: &PendingStart, cx: &App) -> Result<(), String> {
        if self.snapshot(cx) != pending.form {
            return Err(
                "The form changed while the repositories were read again. Start again.".into(),
            );
        }
        if let Some(blocker) = self.blocked_by(cx) {
            return Err(self.blocker_message(blocker, cx).into());
        }
        let bound = self.bindings.iter().filter(|b| b.project.is_some());
        for (binding, seen) in bound.zip(&pending.shown) {
            let Some(preview) = binding.ready(cx) else {
                continue;
            };
            if let Some(change) = unseen_change(seen, &binding.shown(preview, cx)) {
                return Err(change.message(&self.repository_name(binding, cx)));
            }
        }
        Ok(())
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut lanes = Vec::new();
        for b in &self.bindings {
            let Some(project) = &b.project else { continue };
            let Some(preview) = b.ready(cx) else {
                return;
            };
            lanes.push(Lane {
                name: b.preset.name.clone(),
                project: project.clone(),
                base: preview.commit.clone(),
                branch: b.branch_name().unwrap_or_default().to_owned(),
                base_branch: b.base_branch(),
                command: b.command(preview, cx),
                setup: b.setup_command(cx),
                profile: b.preset.profile.clone(),
                ..Default::default()
            });
        }
        let task = self.task.read(cx).value().trim().to_owned();
        let result = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
            let options = crate::state::pipelines::LaunchOptions {
                frozen: self.frozen,
                regression: self.regression && self.can_start_direct(),
                direct_criteria: (self.direct && self.can_start_direct())
                    .then(|| self.criteria.read(cx).value().to_string()),
            };
            p.start(
                self.project.clone(),
                task,
                self.template.clone(),
                lanes,
                options,
                cx,
            )
        });
        match result {
            Ok(id) => {
                self.abandon();
                let _ = self
                    .parent
                    .update(cx, |panel, cx| panel.run_started(id, window, cx));
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Change, Shown, starts_run, unseen_change};

    fn shown(local_changes: bool, check: &str) -> Shown {
        Shown {
            local_changes,
            check: check.into(),
        }
    }

    #[test]
    fn a_fresh_read_stops_the_start_only_for_what_the_user_has_not_seen() {
        let seen = shown(false, "cargo test");
        assert_eq!(unseen_change(&seen, &seen), None);
        assert_eq!(
            unseen_change(&seen, &shown(true, "cargo test")),
            Some(Change::LocalChanges)
        );
        // Changes that went away, or a warning already on screen, do not stop the run.
        let warned = shown(true, "cargo test");
        assert_eq!(unseen_change(&warned, &warned), None);
        assert_eq!(unseen_change(&warned, &seen), None);
        assert_eq!(
            unseen_change(&seen, &shown(false, "npm test")),
            Some(Change::Check("npm test".into()))
        );
        assert_eq!(
            unseen_change(&seen, &shown(false, "")),
            Some(Change::Check(String::new()))
        );
        for change in [Change::LocalChanges, Change::Check("npm test".into())] {
            assert!(change.message("api").contains("api"), "{change:?}");
        }
    }

    #[test]
    fn secondary_enter_starts_an_unblocked_run_but_not_from_an_open_branch_menu() {
        assert!(starts_run(true, false, false));
        // The name being typed in the menu would be dropped for the branch chosen before.
        assert!(!starts_run(true, false, true));
        assert!(!starts_run(true, true, false));
        assert!(!starts_run(false, false, false));
    }
}
