use super::*;
use gpui_kit::component::input::{DefinitionProvider, HoverProvider, Rope};
use hive_core::language_service::{self, position};
use lsp_types::{
    request::{Completion, GotoDefinition, HoverRequest},
    *,
};
use std::{path::Path, rc::Rc};

impl Document {
    pub(crate) fn references(
        document: &Entity<Self>,
        cx: &mut App,
    ) -> Task<Result<Vec<Location>, String>> {
        let editor = document.read(cx).editor.read(cx);
        let (text, offset) = (editor.text().clone(), editor.cursor());
        let captured = Snapshot::capture(
            &document.downgrade(),
            &text,
            offset,
            |caps| {
                caps.references_provider
                    .as_ref()
                    .is_some_and(|v| !matches!(v, OneOf::Left(false)))
            },
            cx,
        );
        let Some((snapshot, client, position)) = captured else {
            return Task::ready(Err(
                "References are unavailable. Start or retry the language tools.".into(),
            ));
        };
        let request = client.request::<request::References>(ReferenceParams {
            text_document_position: position,
            context: ReferenceContext {
                include_declaration: true,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        });
        cx.spawn(async move |cx| {
            let locations = request
                .map_err(|e| e.to_string())?
                .receive()
                .await
                .map_err(|e| e.to_string())?;
            if !cx.update(|cx| snapshot.current(cx)) {
                return Err("The source changed. Refresh references.".into());
            }
            Ok(locations.unwrap_or_default())
        })
    }

    pub(crate) fn definitions(
        document: &Entity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<Vec<LocationLink>>> {
        let editor = document.read(cx).editor.clone();
        let (provider, text, offset) = {
            let editor = editor.read(cx);
            (
                editor.lsp().definition_provider.clone(),
                editor.text().clone(),
                editor.cursor(),
            )
        };
        provider
            .map(|provider| provider.definitions(&text, offset, window, cx))
            .unwrap_or_else(|| Task::ready(Ok(Vec::new())))
    }
}

struct Snapshot {
    document: WeakEntity<Document>,
    text: Rope,
    target: Target,
    path: PathBuf,
}
impl Snapshot {
    fn capture(
        document: &WeakEntity<Document>,
        text: &Rope,
        offset: usize,
        feature: impl FnOnce(&ServerCapabilities) -> bool,
        cx: &mut App,
    ) -> Option<(Self, language_service::Session, TextDocumentPositionParams)> {
        let entity = document.upgrade()?;
        let (target, path) = {
            let d = entity.read(cx);
            if d.loading || d.path_busy || d.base.is_none() {
                return None;
            }
            if !d
                .language
                .as_ref()
                .and_then(|language| language.read(cx).capabilities.as_ref())
                .is_some_and(feature)
            {
                return None;
            }
            (d.target.clone(), d.path.clone())
        };
        let (client, uri) = entity.update(cx, |d, cx| d.language_client(text, cx))?;
        let position = position::at(&text.to_string(), offset)?;
        Some((
            Self {
                document: document.clone(),
                text: text.clone(),
                target,
                path,
            },
            client,
            TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri },
                position,
            },
        ))
    }
    fn current(&self, cx: &App) -> bool {
        self.document.upgrade().is_some_and(|d| {
            let d = d.read(cx);
            d.target == self.target && d.path == self.path && d.editor.read(cx).text() == &self.text
        })
    }
}

pub(super) fn completion(
    document: &WeakEntity<Document>,
    text: &Rope,
    offset: usize,
    context: CompletionContext,
    cx: &mut App,
) -> Option<Task<gpui_kit::Result<Option<CompletionResponse>>>> {
    let (snapshot, client, position) = Snapshot::capture(
        document,
        text,
        offset,
        |caps| caps.completion_provider.is_some(),
        cx,
    )?;
    let request = client
        .request::<Completion>(CompletionParams {
            text_document_position: position,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: Some(context),
        })
        .ok()?;
    Some(cx.spawn(async move |cx| {
        let response = request.receive().await;
        if !cx.update(|cx| snapshot.current(cx)) {
            return Ok(Some(CompletionResponse::Array(Vec::new())));
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::debug!(%error,"completion fallback");
                return Ok(None);
            }
        };
        let items = match response {
            Some(CompletionResponse::Array(items)) => items,
            Some(CompletionResponse::List(list)) => list.items,
            None => return Ok(None),
        };
        let source = snapshot.text.to_string();
        Ok(Some(CompletionResponse::Array(
            items
                .into_iter()
                .filter_map(|item| safe_completion(item, &source))
                .collect(),
        )))
    }))
}

fn safe_completion(mut item: CompletionItem, text: &str) -> Option<CompletionItem> {
    if item.command.is_some()
        || item
            .additional_text_edits
            .as_ref()
            .is_some_and(|edits| !edits.is_empty())
        || item.insert_text_format == Some(InsertTextFormat::SNIPPET)
    {
        return None;
    }
    if let Some(edit) = item.text_edit.as_mut() {
        let CompletionTextEdit::Edit(edit) = edit else {
            return None;
        };
        edit.range = position::editor_range(text, edit.range)?;
    }
    Some(item)
}

struct Features {
    document: WeakEntity<Document>,
    fallback: Option<Rc<dyn DefinitionProvider>>,
}

pub(super) fn install(document: &Entity<Document>, documents: WeakEntity<Documents>, cx: &mut App) {
    let editor = document.read(cx).editor.clone();
    let source = document.downgrade();
    editor.update(cx, |editor, _| {
        let lsp = editor.lsp_mut();
        let provider = Rc::new(Features {
            document: source.clone(),
            fallback: lsp.definition_provider.take(),
        });
        lsp.hover_provider = Some(provider.clone());
        lsp.definition_provider = Some(provider);
        lsp.show_document = Some(super::definitions::show_document(source, documents));
    });
}

impl HoverProvider for Features {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<Option<Hover>>> {
        let Some((snapshot, client, position)) = Snapshot::capture(
            &self.document,
            text,
            offset,
            |caps| {
                caps.hover_provider
                    .as_ref()
                    .is_some_and(|value| !matches!(value, HoverProviderCapability::Simple(false)))
            },
            cx,
        ) else {
            return Task::ready(Ok(None));
        };
        let Ok(request) = client.request::<HoverRequest>(HoverParams {
            text_document_position_params: position,
            work_done_progress_params: Default::default(),
        }) else {
            return Task::ready(Ok(None));
        };
        cx.spawn(async move |cx| {
            let response = request.receive().await;
            if !cx.update(|cx| snapshot.current(cx)) {
                return Ok(None);
            }
            let mut hover = match response {
                Ok(Some(hover)) => hover,
                Ok(None) => return Ok(None),
                Err(error) => {
                    tracing::debug!(%error,"hover unavailable");
                    return Ok(None);
                }
            };
            if let Some(range) = hover.range {
                let source = snapshot.text.to_string();
                hover.range = position::editor_range(&source, range);
            }
            Ok(Some(hover))
        })
    }
}

impl DefinitionProvider for Features {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<Vec<LocationLink>>> {
        let fallback = self
            .fallback
            .as_ref()
            .map(|fallback| fallback.definitions(text, offset, window, cx))
            .unwrap_or_else(|| Task::ready(Ok(Vec::new())));
        let Some((snapshot, client, position)) = Snapshot::capture(
            &self.document,
            text,
            offset,
            |caps| {
                caps.definition_provider
                    .as_ref()
                    .is_some_and(|value| !matches!(value, OneOf::Left(false)))
            },
            cx,
        ) else {
            return fallback;
        };
        let Ok(request) = client.request::<GotoDefinition>(GotoDefinitionParams {
            text_document_position_params: position,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }) else {
            return fallback;
        };
        cx.spawn(async move |cx| {
            let response = request.receive().await;
            if !cx.update(|cx| snapshot.current(cx)) {
                return Ok(Vec::new());
            }
            let response = match response {
                Ok(Some(response)) => response,
                Ok(None) => return fallback.await,
                Err(error) => {
                    tracing::debug!(%error,"definition fallback");
                    return fallback.await;
                }
            };
            let links = match response {
                GotoDefinitionResponse::Link(links) => links,
                GotoDefinitionResponse::Scalar(location) => vec![location_link(location)],
                GotoDefinitionResponse::Array(locations) => {
                    locations.into_iter().map(location_link).collect()
                }
            };
            let (host, root, source_path, source) = (
                snapshot.target.host.clone(),
                snapshot.target.path.clone(),
                snapshot.path.clone(),
                snapshot.text.to_string(),
            );
            let buffers: std::collections::HashMap<_, _> = cx.update(|cx| {
                let documents = crate::app::AppState::get(cx).documents.read(cx);
                links
                    .iter()
                    .take(32)
                    .filter_map(|link| {
                        let path = position::path(&link.target_uri)?;
                        Some((path.clone(), documents.buffer(&host, &path, cx)?))
                    })
                    .collect()
            });
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                    links
                        .into_iter()
                        .take(32)
                        .filter_map(|mut link| {
                            let path = position::path(&link.target_uri)?;
                            let target = if path == root.join(&source_path) {
                                source.clone()
                            } else if let Some(buffer) = buffers.get(&path) {
                                buffer.clone()
                            } else {
                                if !path.starts_with(&root)
                                    && language_service::Family::for_path(&path).is_none()
                                {
                                    return None;
                                }
                                let (read_root, relative) =
                                    if let Ok(relative) = path.strip_prefix(&root) {
                                        (root.as_path(), relative)
                                    } else {
                                        (path.parent()?, Path::new(path.file_name()?))
                                    };
                                match hive_core::files::Access::new(&host, cancellation)
                                    .read(read_root, relative)
                                {
                                    Ok(snapshot) => snapshot.text(),
                                    Err(error) => {
                                        tracing::debug!(%error,"definition source unavailable");
                                        return None;
                                    }
                                }
                            };
                            link.origin_selection_range = link
                                .origin_selection_range
                                .and_then(|range| position::editor_range(&source, range));
                            link.target_range = position::editor_range(&target, link.target_range)?;
                            link.target_selection_range =
                                position::editor_range(&target, link.target_selection_range)?;
                            Some(link)
                        })
                        .collect()
                })
                .await;
            if !cx.update(|cx| snapshot.current(cx)) {
                return Ok(Vec::new());
            }
            Ok(result)
        })
    }
}

fn location_link(location: Location) -> LocationLink {
    LocationLink {
        origin_selection_range: None,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    }
}
#[cfg(test)]
mod tests {
    use super::safe_completion;
    use lsp_types::*;
    #[test]
    fn unsupported_edits_are_omitted_and_utf16_ranges_become_scalar_columns() {
        let item = CompletionItem {
            label: "name".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(0, 2), Position::new(0, 3)),
                new_text: "name".into(),
            })),
            ..Default::default()
        };
        let safe = safe_completion(item.clone(), "😀x").unwrap();
        assert!(
            matches!(safe.text_edit,Some(CompletionTextEdit::Edit(edit)) if edit.range.start.character==1 && edit.range.end.character==2)
        );
        let mut snippet = item.clone();
        snippet.insert_text_format = Some(InsertTextFormat::SNIPPET);
        assert!(safe_completion(snippet, "😀x").is_none());
        let mut auto_import = item;
        auto_import.additional_text_edits = Some(vec![TextEdit {
            range: Range::default(),
            new_text: "import name".into(),
        }]);
        assert!(safe_completion(auto_import, "😀x").is_none());
    }
}
