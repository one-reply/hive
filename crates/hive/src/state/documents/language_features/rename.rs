//! A user-requested LSP rename is reviewed against exact buffer and disk snapshots.
//! Apply updates open buffers together; normal save/recovery owns all subsequent disk writes.
use super::*;
use crate::state::file_io;
use hive_core::{
    files::{self, Snapshot as Disk},
    language_service::{edits, workspace_edits},
};
use std::{collections::BTreeMap, sync::Arc};

const FILE_LIMIT: usize = 128;
const PREVIEW_BYTES: usize = 16 * 1024 * 1024;
const CHANGED_RANGES: usize = 10000;
const STALE: &str = "A source file changed. Recalculate the rename to review the new edits.";

pub(crate) struct RenamePreview {
    origin: Snapshot,
    files: Vec<Change>,
}

struct Change {
    path: PathBuf,
    disk: Disk,
    before: String,
    after: String,
    edits: Vec<TextEdit>,
}

impl RenamePreview {
    pub(crate) fn rows(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for file in &self.files {
            rows.push(file.path.display().to_string());
            let lines = position::Lines::new(&file.before);
            for edit in &file.edits {
                if let (Some(a), Some(b)) =
                    (lines.offset(edit.range.start), lines.offset(edit.range.end))
                {
                    rows.push(format!(
                        "  {}:{}   {} → {}",
                        edit.range.start.line + 1,
                        file.before[..a]
                            .rsplit('\n')
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .count()
                            + 1,
                        display_edit(&file.before[a..b]),
                        display_edit(&edit.new_text)
                    ));
                }
            }
        }
        rows
    }

    pub(crate) fn file_count(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn apply(
        self: &Arc<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<(), String>> {
        let preview = self.clone();
        let reader = self.clone();
        let work = file_io::run(cx.background_executor().clone(), move |cancel| {
            verify_disks(&reader.origin.target, &reader.files, cancel)
        });
        window.spawn(cx, async move |cx| {
            work.await?;
            cx.update(|window, cx| preview.apply_buffers(window, cx))
                .map_err(|e| e.to_string())?
        })
    }

    fn apply_buffers(&self, window: &mut Window, cx: &mut App) -> Result<(), String> {
        if !self.origin.current(cx) {
            return Err(STALE.into());
        }
        let registry = crate::app::AppState::get(cx).documents.clone();
        apply_reviewed(
            &self.origin.target,
            &self.origin.path,
            &registry,
            &self.files,
            window,
            cx,
        )
    }
}

fn verify_disks(
    target: &Target,
    changes: &[Change],
    cancel: &hive_core::command::Cancellation,
) -> Result<(), String> {
    let access = files::Access::new(&target.host, cancel);
    for change in changes {
        let disk = access
            .read(&target.path, &change.path)
            .map_err(|e| e.to_string())?;
        if disk != change.disk {
            return Err(STALE.into());
        }
    }
    Ok(())
}

fn apply_reviewed(
    target: &Target,
    origin: &Path,
    registry: &Entity<Documents>,
    changes: &[Change],
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let mut documents = Vec::new();
    for change in changes {
        let reg = registry.read(cx);
        if reg.mutating_paths.iter().any(|(t, p)| {
            t.host == target.host && t.path == target.path && change.path.starts_with(p)
        }) {
            return Err(STALE.into());
        }
        let open = reg
            .in_path(target, &change.path, cx)
            .into_iter()
            .find(|d| d.read(cx).path == change.path);
        if let Some(doc) = &open {
            let d = doc.read(cx);
            if d.path_busy
                || d.saving
                || d.save_blocker().is_some()
                || d.base.as_ref() != Some(&change.disk)
                || d.text(cx) != change.before
            {
                return Err(STALE.into());
            }
        } else if change.disk.text() != change.before
            || reg.recovered.iter().any(|r| {
                r.draft.host == target.host
                    && r.draft.root == target.path
                    && r.draft.path == change.path
            })
        {
            return Err(STALE.into());
        }
        documents.push(open);
    }
    let mut tabs = Vec::new();
    for (change, open) in changes.iter().zip(documents) {
        let request = OpenFile {
            target: target.clone(),
            path: change.path.clone(),
            position: None,
            preview: false,
            placement: Default::default(),
        };
        let doc = open.unwrap_or_else(|| {
            let doc = registry.update(cx, |reg, cx| reg.get(&request, window, cx));
            doc.update(cx, |d, cx| {
                d.install_reviewed(change.disk.clone(), window, cx)
            });
            doc
        });
        doc.update(cx, |d, cx| {
            d.pin(cx);
            d.replace_preserving_view(change.after.clone(), window, cx);
        });
        tabs.push((request, doc));
    }
    tabs.sort_by_key(|(tab, _)| tab.path == origin);
    registry.update(cx, |_, cx| cx.emit(DocumentEvent::OpenReviewed(tabs)));
    Ok(())
}

fn display_edit(text: &str) -> String {
    if text.is_empty() {
        return "∅".into();
    }
    text.replace('\n', "↵")
}

fn relative_uri(root: &Path, uri: &Uri) -> Option<PathBuf> {
    let path = position::path(uri)?;
    files::location::relative(root, &path).map(Path::to_owned)
}

struct OpenVersion {
    text: Rope,
    disk: Disk,
    version: Option<i32>,
}

/// A server can only address the unsaved text of buffers synchronized with that server.
fn rename_source(
    disk: &Disk,
    open: Option<&OpenVersion>,
    expected: Option<i32>,
) -> Result<String, String> {
    if expected.is_some() && expected != open.and_then(|source| source.version) {
        return Err(STALE.into());
    }
    let Some(source) = open else {
        return Ok(disk.text());
    };
    if source.disk != *disk {
        return Err(STALE.into());
    }
    let text = source.text.to_string();
    if source.version.is_none() && !disk.matches(&text) {
        return Err("Rename includes unsaved changes unknown to this language server. Save the file and recalculate.".into());
    }
    Ok(text)
}

impl Document {
    pub(crate) fn rename(
        document: &Entity<Self>,
        name: String,
        cx: &mut App,
    ) -> Task<Result<Arc<RenamePreview>, String>> {
        let d = document.read(cx);
        let editor = d.editor.read(cx);
        let (text, offset) = (editor.text().clone(), editor.cursor());
        let Some((origin, client, source)) = Snapshot::capture(
            &document.downgrade(),
            &text,
            offset,
            |c| {
                c.rename_provider
                    .as_ref()
                    .is_some_and(|p| !matches!(p, OneOf::Left(false)))
            },
            cx,
        ) else {
            return Task::ready(Err(
                "Rename is unavailable for this document. Check the language tools.".into(),
            ));
        };
        let target = origin.target.clone();
        let mut open = BTreeMap::new();
        let registry = crate::app::AppState::get(cx).documents.clone();
        for doc in registry.read(cx).in_path(&target, Path::new(""), cx) {
            let d = doc.read(cx);
            if let Some(disk) = d.base.clone() {
                let text = d.editor.read(cx).text().clone();
                let session = d
                    .language
                    .clone()
                    .filter(|s| s.entity_id() == origin.session);
                let path = d.path.clone();
                if session.is_some()
                    && doc
                        .update(cx, |d, cx| d.language_client(&text, cx))
                        .is_none()
                {
                    return Task::ready(Err(
                        "Language tools are synchronizing. Try Rename again.".into()
                    ));
                }
                let version = position::uri(&target.path.join(&path))
                    .and_then(|u| session.and_then(|s| s.read(cx).document_version(&u)));
                open.insert(
                    path,
                    OpenVersion {
                        text,
                        disk,
                        version,
                    },
                );
            }
        }
        let request = client.request::<request::Rename>(RenameParams {
            text_document_position: source,
            new_name: name,
            work_done_progress_params: Default::default(),
        });
        cx.spawn(async move |cx| {
            let edit = request
                .map_err(|e| e.to_string())?
                .receive()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("The language server returned no rename edits.")?;
            let docs = workspace_edits::documents(edit)?;
            let work = file_io::run(cx.background_executor().clone(), move |cancel| {
                if docs.len() > FILE_LIMIT {
                    return Err("Rename exceeds the file preview limit.".into());
                }
                let mut paths = BTreeMap::new();
                let mut size = 0;
                let mut count = 0;
                let access = files::Access::new(&target.host, cancel);
                for doc in docs {
                    let path = relative_uri(&target.path, &doc.uri)
                        .ok_or("The rename includes a path outside this worktree.")?;
                    let disk = access
                        .read(&target.path, &path)
                        .map_err(|e| e.to_string())?;
                    count += doc.edits.len();
                    if count > CHANGED_RANGES {
                        return Err("Rename exceeds the preview limit.".into());
                    }
                    let before = rename_source(&disk, open.get(&path), doc.version)?;
                    let after = edits::apply(&before, &doc.edits)
                        .ok_or("The server returned invalid or overlapping edits.")?;
                    size += before.len() + after.len();
                    if size > PREVIEW_BYTES {
                        return Err("Rename exceeds the preview limit.".into());
                    }
                    if before != after
                        && paths
                            .insert(
                                path.clone(),
                                Change {
                                    path,
                                    disk,
                                    before,
                                    after,
                                    edits: doc.edits,
                                },
                            )
                            .is_some()
                    {
                        return Err("The server repeated a document in this rename.".into());
                    }
                }
                if paths.is_empty() {
                    return Err("No occurrences to rename.".into());
                }
                Ok::<_, String>(paths.into_values().collect::<Vec<_>>())
            });
            let files = work.await?;
            if !cx.update(|cx| origin.current(cx)) {
                return Err(STALE.into());
            }
            Ok(Arc::new(RenamePreview { origin, files }))
        })
    }
}

#[cfg(test)]
mod tests;
