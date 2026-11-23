//! How much a stage's CLI may do without asking.
use super::*;

impl TemplateEditor {
    pub(super) fn mode_setting(&self, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let mode = form.mode.unwrap_or(role_mode(self.selected));
        let options = pipeline_modes(form.agent)
            .into_iter()
            .map(|mode| (mode, mode_label(mode).to_owned()))
            .collect();
        setting(
            "Mode",
            choice(
                "stage-mode",
                &mode,
                options,
                cx,
                |s: &mut Self, mode, _, _| s.inspector.mode = Some(mode),
            ),
            cx,
        )
        .child(subtle(mode_summary(mode), cx))
    }
}

pub(in crate::ui::workspace::pipelines) fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::CliDefault => "CLI default",
        PermissionMode::Plan => "Plan (read-only)",
        PermissionMode::AcceptEdits => "Accept edits",
        PermissionMode::Auto => "Auto",
        PermissionMode::Bypass => "Bypass permissions",
    }
}

/// What the chosen mode means for a run nobody is watching.
fn mode_summary(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::CliDefault => {
            "Hive passes no permission setting. If the CLI asks before each action, the run waits for you every time."
        }
        PermissionMode::Plan => "Reads and plans without changing files.",
        PermissionMode::AcceptEdits => {
            "File edits inside the worktree go through. Commands and anything outside the worktree still ask, as the CLI guards them."
        }
        PermissionMode::Auto => {
            "The CLI's own automatic review approves or refuses each action; the run waits only for what it refuses to decide."
        }
        PermissionMode::Bypass => {
            "Warning: every command, edit and network call runs without asking and outside the sandbox. Use only on a disposable machine."
        }
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{PermissionMode, mode_label, mode_summary};

    #[test]
    fn each_mode_says_what_still_asks_and_bypass_warns() {
        assert!(mode_summary(PermissionMode::CliDefault).contains("waits for you"));
        assert!(mode_summary(PermissionMode::Plan).contains("without changing files"));
        assert!(mode_summary(PermissionMode::AcceptEdits).contains("still ask"));
        assert!(mode_summary(PermissionMode::Bypass).starts_with("Warning"));
        for mode in PermissionMode::ALL {
            assert!(!mode_label(mode).is_empty() && !mode_summary(mode).is_empty());
        }
    }
}
