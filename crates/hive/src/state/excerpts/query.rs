use super::*;

pub(crate) enum Query {
    References(WeakEntity<Document>),
    Problems,
    Content(files::content_search::Query),
}

impl Query {
    pub(super) fn run(
        &self,
        target: &Target,
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
            Self::Content(query) => {
                let (target, query) = (target.clone(), query.clone());
                cx.background_executor().spawn(super::super::file_io::run(
                    cx.background_executor().clone(),
                    move |cancel| {
                        if !target.host.is_local() {
                            return Err("Content search is available for local worktrees.".into());
                        }
                        let index = files::Access::new(&target.host, cancel)
                            .list_files(&target.path)
                            .map_err(|e| e.to_string())?;
                        let mut hits = Vec::new();
                        let summary = files::content_search::search(
                            &target.path,
                            &index.paths,
                            &query,
                            cancel,
                            |file| {
                                hits.extend(file.lines.into_iter().map(|line| Hit {
                                    path: target.path.join(&file.path),
                                    line: line.line,
                                    message: String::new(),
                                }));
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        Ok((hits, summary.truncated || index.truncated))
                    },
                ))
            }
        }
    }
}
