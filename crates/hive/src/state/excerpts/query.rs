use super::*;

pub(crate) enum Query {
    References(WeakEntity<Document>),
    Problems,
    Content(files::content_search::Query, Option<std::path::PathBuf>),
}

impl Query {
    pub(super) fn run(
        &self,
        target: &Target,
        sources: &content_search::Buffers,
        cx: &mut App,
    ) -> Task<Result<(Vec<Hit>, bool), String>> {
        match self {
            Self::References(document) => {
                let Some(document) = document.upgrade() else {
                    return Task::ready(Err("Source tab closed. Open references again.".into()));
                };
                let task = Document::references(&document, cx);
                cx.spawn(async move |_| {
                    let locations = task.await?;
                    Ok((
                        locations
                            .into_iter()
                            .filter_map(|location| {
                                Some(Hit {
                                    path: hive_core::language_service::position::path(
                                        &location.uri,
                                    )?,
                                    line: location.range.start.line,
                                    column: 0,
                                    message: String::new(),
                                })
                            })
                            .collect(),
                        false,
                    ))
                })
            }
            Self::Problems => {
                let documents = AppState::get(cx).documents.read(cx);
                Task::ready(Ok((
                    documents.problems(target, cx),
                    documents.problems_limited(target, cx),
                )))
            }
            Self::Content(query, folder) => {
                let (folder, sources) = (folder.clone(), sources.clone());
                let (target, query) = (target.clone(), query.clone());
                cx.background_executor().spawn(super::super::file_io::run(
                    cx.background_executor().clone(),
                    move |cancel| {
                        if !target.host.is_local() {
                            return Err("Content search is available for local worktrees.".into());
                        }
                        let mut hits = Vec::new();
                        let summary = content_search::run(
                            &target,
                            &query,
                            folder.as_deref(),
                            &sources,
                            cancel,
                            |file| {
                                hits.extend(file.lines.into_iter().map(|line| Hit {
                                    path: target.path.join(&file.path),
                                    line: line.line,
                                    column: line.column,
                                    message: String::new(),
                                }));
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        Ok((hits, summary.truncated || summary.index_truncated))
                    },
                ))
            }
        }
    }
}
