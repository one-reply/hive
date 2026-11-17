use super::*;
use gpui_kit::component::input::RopeExt as _;

const FORMAT_DISABLED: &str = "Formatting is off: language tools are disabled in Settings → Files.";

impl Document {
    pub(crate) fn format(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading
            || self.path_busy
            || self.base.is_none()
            || !hive_core::formatting::supported(&self.path)
        {
            return;
        }
        // Prettier comes from `node_modules`, cargo and rustfmt follow `rust-toolchain.toml`:
        // formatting runs what the worktree chooses, exactly like a language server.
        if !self.runs_language_tools(cx) {
            self.format_error = Some(FORMAT_DISABLED.into());
            cx.notify();
            return;
        }
        let target = self.target.clone();
        let path = self.path.clone();
        let text = self.editor.read(cx).text().clone();
        let revision = self.revision;
        self.formatting = true;
        self.format_error = None;
        self._format = Some(cx.spawn_in(window, async move |this, cx| {
            let (work_target, work_path, work_text) = (target.clone(), path.clone(), text.clone());
            let result = crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                hive_core::formatting::format(&work_target.host, &work_target.path, &work_path, &work_text.to_string(), cancellation)
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.formatting = false;
                if !accepts(revision, this.revision, &target, &this.target, &path, &this.path)
                    || this.editor.read(cx).text() != &text || this.path_busy {
                    this.format_error = Some("Formatting was not applied because the document changed. Run Format again.".into());
                    cx.notify(); return;
                }
                match result {
                    Ok(formatted) if formatted != text => {
                        this.editor.update(cx, |editor, cx| {
                            let selected = editor.selected_range();
                            let start = editor.text().offset_to_position(selected.start);
                            let end = editor.text().offset_to_position(selected.end);
                            let reversed = editor.is_selection_reversed();
                            let scroll = editor.scroll_offset();
                            editor.replace_all(formatted, window, cx);
                            let range = editor.text().position_to_offset(&start)..editor.text().position_to_offset(&end);
                            editor.set_selected_range_with_direction(range, reversed, cx);
                            editor.set_scroll_offset(scroll, cx);
                        });
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(%error, "formatting failed");
                        this.format_error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn accepts(
    revision: u64,
    current: u64,
    target: &Target,
    current_target: &Target,
    path: &std::path::Path,
    current_path: &std::path::Path,
) -> bool {
    revision == current && target == current_target && path == current_path
}

#[cfg(test)]
mod tests {
    use super::accepts;
    use hive_core::{host::Host, projects::Target};
    use std::path::Path;
    #[test]
    fn edits_renames_and_host_changes_reject_formatter_output() {
        let target = Target {
            host: Host::Local,
            project: "/work".into(),
            path: "/work".into(),
        };
        let path = Path::new("a.rs");
        assert!(accepts(1, 1, &target, &target, path, path));
        assert!(!accepts(1, 2, &target, &target, path, path));
        assert!(!accepts(1, 1, &target, &target, path, Path::new("b.rs")));
        let remote = Target {
            host: Host::Ssh {
                target: "server".into(),
            },
            ..target.clone()
        };
        assert!(!accepts(1, 1, &target, &remote, path, path));
    }
}
