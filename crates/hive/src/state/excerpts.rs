use crate::{
    app::AppState,
    state::documents::{Document, DocumentEvent, OpenFile},
};
use gpui_kit::{
    component::input::{Position, Rope, RopeExt as _},
    *,
};
use hive_core::{
    files::{
        self,
        excerpts::{self, File, Hit},
    },
    projects::Target,
};
use std::{collections::HashMap, ops::Range, path::Path};

mod query;
pub(crate) use query::Query;
const CACHE_FILES: usize = 32;
const READERS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub file: usize,
    pub line: Option<u32>,
}

pub(crate) struct Excerpts {
    pub target: Target,
    query: Query,
    pub files: Vec<File>,
    pub rows: Vec<Row>,
    pub loading: bool,
    pub stale: bool,
    pub truncated: bool,
    pub error: Option<String>,
    cache: HashMap<usize, Result<Rope, String>>,
    visible: Vec<usize>,
    line_counts: HashMap<usize, u32>,
    reads: HashMap<usize, Task<()>>,
    _query: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Excerpts {
    pub(crate) fn new(query: Query, target: Target, cx: &mut Context<Self>) -> Self {
        let documents = AppState::get(cx).documents.clone();
        let git = AppState::get(cx).git.clone();
        let mut subscriptions = vec![
            cx.subscribe(&documents, |this, _, event, cx| {
                let target = match event {
                    DocumentEvent::Changed(t)
                    | DocumentEvent::Saved(t)
                    | DocumentEvent::Deleted(t, _) => t,
                    _ => return,
                };
                if *target == this.target && !this.stale {
                    this.stale = true;
                    cx.notify();
                }
            }),
            cx.observe(&git, |this, git, cx| {
                // A snapshot never silently replaces the code the user is reviewing.
                if git.read(cx).target.as_ref() == Some(&this.target)
                    && !this.loading
                    && !this.stale
                {
                    this.stale = true;
                    cx.notify();
                }
            }),
        ];
        if matches!(query, Query::Problems) {
            for session in documents.read(cx).problem_sessions(&target) {
                subscriptions.push(cx.observe(&session, |this, _, cx| {
                    if !this.loading && !this.stale {
                        this.stale = true;
                        cx.notify();
                    }
                }));
            }
        }
        let mut this = Self {
            target,
            query,
            files: Vec::new(),
            rows: Vec::new(),
            loading: false,
            stale: false,
            truncated: false,
            error: None,
            cache: HashMap::new(),
            visible: Vec::new(),
            line_counts: HashMap::new(),
            reads: HashMap::new(),
            _query: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reads.clear();
        self.cache.clear();
        self.line_counts.clear();
        self.visible.clear();
        self.files.clear();
        self.rows.clear();
        self.error = None;
        self.stale = false;
        self.loading = true;
        let query = self.query.run(&self.target, cx);
        self._query = Some(cx.spawn(async move |this, cx| {
            let result = query.await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((hits, limited)) => {
                        let (files, truncated) = excerpts::group(hits);
                        this.files = files;
                        this.truncated = truncated || limited;
                        this.rebuild_rows();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "excerpt query failed");
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        self.rows = self
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, source)| {
                let lines = self.line_counts.get(&file).copied();
                file_rows(source, file, lines)
            })
            .collect();
    }

    pub(crate) fn load_visible(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.visible = self
            .rows
            .get(range)
            .unwrap_or_default()
            .iter()
            .map(|row| row.file)
            .collect();
        self.visible.dedup();
        self.start_reads(cx);
    }

    fn start_reads(&mut self, cx: &mut Context<Self>) {
        for index in self.visible.clone() {
            if self.reads.len() >= READERS {
                break;
            }
            if self.cache.contains_key(&index) || self.reads.contains_key(&index) {
                continue;
            }
            let path = self.files[index].path.clone();
            let host = self.target.host.clone();
            let buffer = if matches!(self.query, Query::Content(_)) {
                None
            } else {
                AppState::get(cx)
                    .documents
                    .read(cx)
                    .buffer(&host, &path, cx)
            };
            let task = cx.spawn(async move |this, cx| {
                let result = super::file_io::run(cx.background_executor().clone(), move |cancel| {
                    let text = if let Some(text) = buffer {
                        text
                    } else {
                        let root = path
                            .parent()
                            .ok_or_else(|| "Invalid source path".to_owned())?;
                        let name = path
                            .file_name()
                            .ok_or_else(|| "Invalid source path".to_owned())?;
                        files::Access::new(&host, cancel)
                            .read(root, Path::new(name))
                            .map_err(|e| e.to_string())?
                            .text()
                    };
                    Ok(Rope::from(text))
                })
                .await;
                let _ = this.update(cx, |this, cx| {
                    this.reads.remove(&index);
                    if let Err(error) = &result {
                        tracing::warn!(%error, "excerpt source read failed");
                    }
                    if this.cache.len() >= CACHE_FILES
                        && let Some(old) = this
                            .cache
                            .keys()
                            .find(|i| !this.visible.contains(i))
                            .copied()
                    {
                        this.cache.remove(&old);
                    }
                    if let Ok(text) = &result {
                        this.line_counts.insert(index, text.lines_len() as u32);
                    }
                    this.cache.insert(index, result);
                    this.rebuild_rows();
                    this.start_reads(cx);
                    cx.notify();
                });
            });
            self.reads.insert(index, task);
        }
    }

    pub(crate) fn line(&self, row: Row) -> (String, String, bool) {
        let source = &self.files[row.file];
        let Some(line) = row.line else {
            let path = source
                .path
                .strip_prefix(&self.target.path)
                .unwrap_or(&source.path);
            let error = self
                .cache
                .get(&row.file)
                .and_then(|r| r.as_ref().err())
                .cloned()
                .unwrap_or_default();
            return (path.display().to_string(), error, false);
        };
        let text = match self.cache.get(&row.file) {
            Some(Ok(text)) if (line as usize) < text.lines_len() => {
                text.slice_line(line as usize).to_string()
            }
            Some(Err(_)) => "Source unavailable".into(),
            _ => "Loading…".into(),
        };
        let matched = source.hits.iter().any(|hit| hit.line == line);
        let message = source
            .hits
            .iter()
            .filter(|hit| hit.line == line && !hit.message.is_empty())
            .map(|hit| hit.message.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        (
            text.trim_end_matches(['\r', '\n'])
                .chars()
                .take(2_000)
                .collect(),
            message,
            matched,
        )
    }

    pub(crate) fn open(&self, row: Row, cx: &mut App) {
        let path = self.files[row.file].path.clone();
        let position = Position::new(
            row.line
                .unwrap_or_else(|| self.files[row.file].hits[0].line),
            0,
        );
        AppState::get(cx)
            .documents
            .clone()
            .update(cx, |documents, cx| {
                if let Ok(relative) = path.strip_prefix(&self.target.path) {
                    documents.request(
                        OpenFile {
                            target: self.target.clone(),
                            path: relative.into(),
                            position: Some(position),
                            preview: false,
                            placement: Default::default(),
                        },
                        cx,
                    );
                } else {
                    documents.emit(
                        DocumentEvent::External(self.target.clone(), path, position),
                        cx,
                    );
                }
            });
    }
}

fn file_rows(source: &File, file: usize, lines: Option<u32>) -> impl Iterator<Item = Row> + '_ {
    std::iter::once(Row { file, line: None }).chain(source.windows.iter().flat_map(move |window| {
        let end = lines.map_or(window.end, |lines| lines.min(window.end));
        (window.start..end).map(move |line| Row {
            file,
            line: Some(line),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::file_rows;
    use hive_core::files::excerpts::{File, Hit};
    #[test]
    fn excerpt_rows_clip_to_eof_and_keep_unreadable_file_headers() {
        let source = File {
            path: "/work/a.rs".into(),
            hits: vec![Hit {
                path: "/work/a.rs".into(),
                line: 2,
                message: String::new(),
            }],
            windows: vec![0..6, 10..14],
        };
        assert_eq!(file_rows(&source, 0, None).count(), 11);
        assert_eq!(
            file_rows(&source, 0, Some(3))
                .map(|row| row.line)
                .collect::<Vec<_>>(),
            [None, Some(0), Some(1), Some(2)]
        );
        assert_eq!(file_rows(&source, 0, Some(0)).count(), 1);
    }
}
