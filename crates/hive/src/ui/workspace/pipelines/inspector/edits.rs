//! Whether a lane's developer edits files in its worktree without asking.
use super::*;
use hive_core::agent::launch::supports_accept_edits;

impl TemplateEditor {
    pub(super) fn lane_edits(&self, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let block = section(cx).child(section_title(Lucide::FilePen, "File edits", cx));
        if !supports_accept_edits(form.agent) {
            return block.child(subtle(unsupported_note(form.agent.name()), cx));
        }
        let options = vec![
            (
                EditApproval::CliDefault,
                "Ask as the CLI is configured".to_owned(),
            ),
            (
                EditApproval::AcceptEdits,
                "Edit files in the worktree without asking".to_owned(),
            ),
        ];
        block
            .child(subtle(edits_summary(form.edits), cx))
            .child(setting(
                "Developer may",
                choice(
                    "lane-edits",
                    &form.edits,
                    options,
                    cx,
                    |s: &mut Self, edits, _, _| s.inspector.edits = edits,
                ),
                cx,
            ))
    }
}

/// What the chosen setting means for a run nobody is watching.
fn edits_summary(edits: EditApproval) -> &'static str {
    match edits {
        EditApproval::CliDefault => {
            "Hive passes no permission setting. If the CLI asks before each edit, the run waits for you every time."
        }
        EditApproval::AcceptEdits => {
            "File edits inside the worktree go through. Commands and anything outside the worktree still ask, as the CLI guards them."
        }
    }
}

fn unsupported_note(agent: &str) -> String {
    format!(
        "{agent} has no setting for file edits alone, only one that approves everything, which Hive never passes. Its own configuration decides."
    )
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{EditApproval, edits_summary, unsupported_note};

    #[test]
    fn each_setting_says_what_still_asks() {
        assert!(edits_summary(EditApproval::CliDefault).contains("waits for you"));
        assert!(edits_summary(EditApproval::AcceptEdits).contains("still ask"));
        assert!(unsupported_note("OpenCode").starts_with("OpenCode has no setting"));
    }
}
