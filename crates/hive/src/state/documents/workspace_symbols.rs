//! Named declarations from the language servers active in this worktree.
use super::*;
use hive_core::language_service::position;
use lsp_types::{
    Location, OneOf, WorkspaceSymbolParams, WorkspaceSymbolResponse,
    request::WorkspaceSymbolRequest,
};
use std::path::Path;

#[derive(Clone)]
pub(crate) struct SymbolLocation {
    pub name: String,
    pub path: PathBuf,
    position: Position,
}

fn symbol(root: &Path, name: String, location: Location) -> Option<SymbolLocation> {
    let absolute = position::path(&location.uri)?;
    let path = hive_core::files::location::relative(root, &absolute)?;
    Some(SymbolLocation {
        name,
        path: path.into(),
        position: location.range.start,
    })
}

impl Documents {
    pub(crate) fn workspace_symbols(
        &self,
        target: &Target,
        query: String,
        cx: &mut App,
    ) -> Task<Result<Vec<SymbolLocation>, String>> {
        let params = WorkspaceSymbolParams {
            query,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let requests: Vec<_> = self
            .problem_sessions(target)
            .into_iter()
            .filter_map(|session| {
                session
                    .read(cx)
                    .symbol_client()
                    .map(|client| client.request::<WorkspaceSymbolRequest>(params.clone()))
            })
            .collect();
        let root = target.path.clone();
        cx.spawn(async move |_| {
            if requests.is_empty() {
                return Err(
                    "Open a Rust or TypeScript file and start its language tools to find symbols."
                        .into(),
                );
            }
            let mut rows = Vec::new();
            let mut last_error = None;
            for request in requests {
                let response = match request {
                    Ok(request) => request.receive().await,
                    Err(e) => Err(e),
                };
                match response {
                    Ok(Some(WorkspaceSymbolResponse::Flat(items))) => rows.extend(
                        items
                            .into_iter()
                            .filter_map(|item| symbol(&root, item.name, item.location))
                            .take(1000),
                    ),
                    Ok(Some(WorkspaceSymbolResponse::Nested(items))) => rows.extend(
                        items
                            .into_iter()
                            .filter_map(|item| match item.location {
                                OneOf::Left(loc) => symbol(&root, item.name, loc),
                                OneOf::Right(_) => None,
                            })
                            .take(1000),
                    ),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(%e,"workspace symbol query failed");
                        last_error = Some(e.to_string());
                    }
                }
            }
            if rows.is_empty()
                && let Some(error) = last_error
            {
                return Err(error);
            }
            rows.sort_by(|a, b| {
                (&a.name, &a.path, a.position).cmp(&(&b.name, &b.path, b.position))
            });
            rows.dedup_by(|a, b| a.path == b.path && a.position == b.position && a.name == b.name);
            Ok(rows)
        })
    }

    pub(crate) fn resolve_symbol(
        &self,
        target: Target,
        symbol: SymbolLocation,
        cx: &App,
    ) -> Task<Result<OpenFile, String>> {
        self.resolve_position(target, symbol.path, symbol.position, cx)
    }

    pub(crate) fn resolve_position(
        &self,
        target: Target,
        path: PathBuf,
        server_position: Position,
        cx: &App,
    ) -> Task<Result<OpenFile, String>> {
        let buffer = self.buffer(&target.host, &target.path.join(&path), cx);
        cx.background_executor().spawn(crate::state::file_io::run(
            cx.background_executor().clone(),
            move |cancel| {
                let text = match buffer {
                    Some(text) => text,
                    None => hive_core::files::Access::new(&target.host, cancel)
                        .read(&target.path, &path)
                        .map_err(|e| e.to_string())?
                        .text(),
                };
                let position = position::editor_position(&text, server_position)
                    .ok_or("The location changed. Request it again.")?;
                Ok(OpenFile {
                    target,
                    path,
                    position: Some(position),
                    preview: false,
                    placement: Default::default(),
                })
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{Location, Path, symbol};
    #[test]
    fn workspace_symbol_uses_component_boundaries() {
        for (uri, ok) in [
            ("file:///work/src/file.tsx", true),
            ("file:///work2/file.tsx", false),
            ("file:///work/.git/a", false),
        ] {
            let loc = Location::new(uri.parse().unwrap(), lsp_types::Range::default());
            assert_eq!(symbol(Path::new("/work"), "name".into(), loc).is_some(), ok);
        }
    }
}
