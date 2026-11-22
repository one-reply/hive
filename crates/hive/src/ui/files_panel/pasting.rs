//! Paste from Finder: ⌘V and the Paste menu items read the files copied in Finder (⌘C) from
//! the clipboard, and files dragged in from Finder drop into the folder under the pointer.
//! All of them run one `FileOperation::Paste`, which copies without replacing anything.
use super::*;
use crate::state::files::FileOperation;
use hive_core::projects::Target;
use std::any::Any;

const REMOTE_UNSUPPORTED: &str =
    "Pasting files from this Mac works only in local worktrees, not over SSH.";
const BUSY: &str = "Another file operation is running. Paste again when it finishes.";

/// The Paste menu item's label; over SSH it says why the item is disabled.
pub(super) fn menu_label(local: bool) -> &'static str {
    if local {
        "Paste"
    } else {
        "Paste (local worktrees only)"
    }
}

/// The folder a pasted or new entry goes into for a row: the row itself when it is a folder,
/// otherwise the folder holding it.
pub(super) fn folder_for(path: &Path, is_folder: bool) -> PathBuf {
    if is_folder {
        path.to_owned()
    } else {
        path.parent().unwrap_or(Path::new("")).to_owned()
    }
}

/// Whether a folder row or the root header for `directory` accepts what is being dragged:
/// a move of one entry of this worktree, or files from Finder.
pub(super) fn accepts_drop(value: &dyn Any, target: Option<&Target>, directory: &Path) -> bool {
    if let Some(drag) = value.downcast_ref::<DraggedFiles>() {
        return drag.can_move_to(target, directory);
    }
    accepts_finder_drop(value, target)
}

/// Whether files dragged from Finder may drop here. File rows and the empty tree take only
/// these: an internal move still needs a folder row. Over SSH the drop is accepted so the
/// panel can say why it cannot paste there.
pub(super) fn accepts_finder_drop(value: &dyn Any, target: Option<&Target>) -> bool {
    value.is::<ExternalPaths>() && target.is_some()
}

/// Why a paste into this worktree cannot start now, in the panel's words.
fn refusal(local: bool, busy: bool) -> Option<&'static str> {
    if !local {
        Some(REMOTE_UNSUPPORTED)
    } else if busy {
        Some(BUSY)
    } else {
        None
    }
}

/// The files and folders copied in Finder, or nothing when the clipboard holds anything else.
pub(super) fn copied_paths(cx: &App) -> Vec<PathBuf> {
    cx.read_from_clipboard()
        .into_iter()
        .flat_map(ClipboardItem::into_entries)
        .find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
            _ => None,
        })
        .unwrap_or_default()
}

impl FilesPanel {
    /// ⌘V in the tree: paste into the selected folder, beside the selected file, or into the root.
    pub(super) fn paste_into_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .tree
            .read(cx)
            .selected_entry()
            .filter(|entry| !entry.is_disabled())
            .map(|entry| folder_for(&tree::path(entry.item().id.as_ref()), entry.is_folder()))
            .unwrap_or_default();
        self.paste_clipboard(directory, window, cx);
    }

    /// Paste what Finder copied. Text on the clipboard, such as a copied path, is not a file
    /// to paste, so it is left alone.
    pub(super) fn paste_clipboard(
        &mut self,
        directory: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sources = copied_paths(cx);
        if !sources.is_empty() {
            self.paste(directory, sources, window, cx);
        }
    }

    pub(super) fn paste(
        &mut self,
        directory: PathBuf,
        sources: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_hover = None;
        self._drag_expand = None;
        let files = self.files.read(cx);
        let Some(target) = files.target() else {
            return;
        };
        if let Some(refusal) = refusal(target.host.is_local(), files.busy) {
            self.operation_error = Some(refusal.to_owned());
            cx.notify();
            return;
        }
        let operation = FileOperation::Paste {
            directory,
            sources: sources.into(),
        };
        self.files
            .update(cx, |files, cx| files.perform(operation, window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::{DraggedFiles, accepts_drop, accepts_finder_drop, folder_for, refusal};
    use gpui_kit::ExternalPaths;
    use hive_core::{host::Host, projects::Target};
    use std::path::{Path, PathBuf};

    fn target(host: Host) -> Target {
        Target {
            project: "/repo".into(),
            path: "/repo".into(),
            host,
        }
    }

    #[test]
    fn folder_for_keeps_a_folder_and_lifts_a_file_to_its_folder() {
        assert_eq!(folder_for(Path::new("src/ui"), true), Path::new("src/ui"));
        assert_eq!(
            folder_for(Path::new("src/main.rs"), false),
            Path::new("src")
        );
        assert_eq!(folder_for(Path::new("README.md"), false), Path::new(""));
    }

    #[test]
    fn finder_drops_need_a_worktree_and_internal_moves_keep_their_rules() {
        let local = target(Host::Local);
        let remote = target(Host::Ssh {
            target: "home".into(),
        });
        let finder = ExternalPaths::default();
        assert!(accepts_drop(&finder, Some(&local), Path::new("src")));
        assert!(accepts_drop(&finder, Some(&remote), Path::new("")));
        assert!(!accepts_drop(&finder, None, Path::new("src")));

        let moved = DraggedFiles {
            target: local.clone(),
            paths: std::sync::Arc::from([PathBuf::from("src")]),
        };
        assert!(accepts_drop(&moved, Some(&local), Path::new("docs")));
        assert!(!accepts_drop(&moved, Some(&local), Path::new("src/inner")));
        assert!(!accepts_drop(&"text", Some(&local), Path::new("src")));
        assert!(accepts_finder_drop(&finder, Some(&local)));
        assert!(!accepts_finder_drop(&finder, None));
        assert!(
            !accepts_finder_drop(&moved, Some(&local)),
            "a move never lands beside a file row"
        );
    }

    #[test]
    fn a_paste_over_ssh_or_during_another_operation_is_refused_with_the_reason() {
        assert_eq!(refusal(true, false), None);
        assert!(refusal(false, false).is_some_and(|reason| reason.contains("SSH")));
        assert!(refusal(true, true).is_some_and(|reason| reason.contains("Another file")));
    }
}
