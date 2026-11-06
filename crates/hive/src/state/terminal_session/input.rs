use super::{SessionEvent, TerminalSession};
use gpui_kit::{ClipboardEntry, ClipboardItem, Context};
use hive_core::terminal::paste;
use hive_core::{host::Host, projects::Target};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub(super) enum Input {
    Bytes(Vec<u8>),
    Text(String),
    Image(Vec<u8>),
}

impl TerminalSession {
    pub(crate) fn write_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.enqueue_input(Input::Bytes(bytes), cx);
    }

    pub(crate) fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = cx.read_from_clipboard().and_then(clipboard_input) {
            self.enqueue_input(input, cx);
        }
    }

    pub(crate) fn accepts_file_drop(&self, target: &Target) -> bool {
        same_worktree(target, self.worktree.as_deref()) && !self.exited && self.terminal().is_some()
    }

    pub(crate) fn insert_file_references(
        &mut self,
        target: &Target,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self.accepts_file_drop(target) {
            return Err("Drop the file into a running terminal in the same worktree.".into());
        }
        let references = paste::file_references(
            &target.path,
            paths,
            self.directory.as_deref().unwrap_or(&target.path),
        )
        .map_err(|error| error.to_string())?;
        self.enqueue_input(Input::Text(references), cx);
        Ok(())
    }

    pub(super) fn enqueue_input(&mut self, input: Input, cx: &mut Context<Self>) {
        if self.exited || self.terminal().is_none() {
            return;
        }
        if self
            .terminal()
            .is_some_and(|terminal| terminal.clear_selection())
        {
            cx.notify();
        }
        self.pending_input.push_back(input);
        self.flush_input(cx);
    }

    fn flush_input(&mut self, cx: &mut Context<Self>) {
        if self.paste_task.is_some() {
            return;
        }
        while let Some(input) = self.pending_input.pop_front() {
            let Some(terminal) = self.terminal().filter(|_| !self.exited) else {
                self.pending_input.clear();
                return;
            };
            let bytes = match input {
                Input::Bytes(bytes) => bytes,
                Input::Text(text) => paste::text_bytes(&text, terminal.mode()),
                Input::Image(bytes) => {
                    self.start_image_paste(bytes, cx);
                    return;
                }
            };
            terminal.scroll_to_bottom();
            terminal.write(bytes);
        }
    }

    fn start_image_paste(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let save = cx.background_executor().spawn(async move {
            let result = paste::save_image(&bytes).and_then(|path| paste::quoted_path(&path));
            if let Err(error) = &result {
                tracing::error!(%error, "could not paste clipboard image");
            }
            result
        });
        self.paste_task = Some(cx.spawn(async move |this, cx| {
            let result = save.await;
            // The weak entity keeps a late image from reaching a closed or different pane.
            let _ = this.update(cx, |this, cx| {
                this.paste_task = None;
                if this.exited || this.terminal().is_none() {
                    this.pending_input.clear();
                    return;
                }
                match result {
                    Ok(path) => this.pending_input.push_front(Input::Text(path)),
                    Err(error) => {
                        discard_submissions(&mut this.pending_input);
                        cx.emit(SessionEvent::PasteFailed(format!(
                            "{error}. Paste the image again before pressing Enter."
                        )));
                    }
                }
                this.flush_input(cx);
            });
        }));
    }
}

fn same_worktree(target: &Target, worktree: Option<&Path>) -> bool {
    target.host == Host::Local && worktree == Some(target.path.as_path())
}

fn clipboard_input(item: ClipboardItem) -> Option<Input> {
    if item
        .entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
    {
        item.into_entries().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(Input::Image(image.bytes)),
            _ => None,
        })
    } else {
        item.text().map(Input::Text)
    }
}

fn discard_submissions(pending: &mut VecDeque<Input>) {
    // Enter, Ctrl+J and Alt+Enter must not submit a prompt whose image failed to save.
    pending.retain(|input| !matches!(input, Input::Bytes(bytes) if matches!(bytes.as_slice(), b"\r" | b"\n" | b"\x1b\r")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Image, ImageFormat};

    #[test]
    fn file_drops_only_target_the_same_local_worktree() {
        let mut target = Target {
            project: "/repo".into(),
            path: "/repo-worktrees/feature".into(),
            host: Host::Local,
        };
        assert!(same_worktree(&target, Some(&target.path)));
        assert!(!same_worktree(&target, Some(&target.project)));
        assert!(!same_worktree(&target, Some(Path::new("/other"))));
        assert!(!same_worktree(&target, None));
        target.host = Host::Ssh {
            target: "remote".into(),
        };
        assert!(!same_worktree(&target, Some(&target.path)));
    }

    #[test]
    fn snapshots_image_bytes_and_preserves_text_clipboards() {
        let item = ClipboardItem::new_image(&Image::from_bytes(ImageFormat::Png, vec![1, 2, 3]));
        assert!(matches!(clipboard_input(item), Some(Input::Image(bytes)) if bytes == [1, 2, 3]));
        assert!(matches!(
            clipboard_input(ClipboardItem::new_string("one\ntwo".into())),
            Some(Input::Text(text)) if text == "one\ntwo"
        ));
    }

    #[test]
    fn failed_image_keeps_typing_and_other_pastes_but_removes_queued_submit_keys() {
        let mut pending = VecDeque::from([
            Input::Bytes(b"a".to_vec()),
            Input::Bytes(b"\r".to_vec()),
            Input::Text("pasted text".into()),
            Input::Image(vec![2]),
            Input::Bytes(b"\x1b[D".to_vec()),
            Input::Bytes(b"\x1b\r".to_vec()),
            Input::Bytes(b"\n".to_vec()),
        ]);
        discard_submissions(&mut pending);
        assert_eq!(pending.len(), 4);
        assert!(matches!(pending.pop_front(), Some(Input::Bytes(bytes)) if bytes == b"a"));
        assert!(matches!(pending.pop_front(), Some(Input::Text(text)) if text == "pasted text"));
        assert!(matches!(pending.pop_front(), Some(Input::Image(bytes)) if bytes == [2]));
        assert!(matches!(pending.pop_front(), Some(Input::Bytes(bytes)) if bytes == b"\x1b[D"));
    }
}
