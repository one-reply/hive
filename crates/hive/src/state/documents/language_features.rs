use super::*;
use gpui_kit::component::input::{DefinitionProvider, HoverProvider, Rope};
use hive_core::language_service::{self, position};
use lsp_types::{
    request::{Completion, GotoDefinition, HoverRequest},
    *,
};
use std::{path::Path, rc::Rc};

mod completion_items;
mod imports;
pub(crate) use imports::ImportMenu;
mod fixes;
pub(super) mod rename;
mod signature;
pub(crate) use signature::CallSignature;

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

#[derive(Clone)]
struct Snapshot {
    uri: Uri,
    version: i32,
    generation: u64,
    revision: Option<u64>,
    session: EntityId,
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
        let session = entity.read(cx).language.as_ref()?;
        let (generation, version) = session.read(cx).synchronized_revision(&uri)?;
        let position = position::at(&text.to_string(), offset)?;
        Some((
            Self {
                uri: uri.clone(),
                version,
                generation,
                revision: None,
                session: session.entity_id(),
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
            d.target == self.target
                && d.path == self.path
                && !d.path_busy
                && self.revision.is_none_or(|v| d.revision == v)
                && d.language.as_ref().is_some_and(|s| {
                    s.entity_id() == self.session
                        && s.read(cx).synchronized_revision(&self.uri)
                            == Some((self.generation, self.version))
                })
                && d.editor.read(cx).text() == &self.text
        })
    }
}

pub(super) fn completion(
    document: &WeakEntity<Document>,
    text: &Rope,
    offset: usize,
    context: CompletionContext,
    window: &mut Window,
    cx: &mut App,
) -> Option<Task<gpui_kit::Result<Option<CompletionResponse>>>> {
    let (mut snapshot, client, position) = Snapshot::capture(
        document,
        text,
        offset,
        |caps| caps.completion_provider.is_some(),
        cx,
    )?;
    let entity = snapshot.document.upgrade()?;
    let options = entity
        .read(cx)
        .language
        .as_ref()?
        .read(cx)
        .capabilities
        .as_ref()?
        .completion_provider
        .as_ref()?;
    let resolve = options.resolve_provider == Some(true);
    let context = completion_context(context, options);
    let request = client
        .request::<Completion>(CompletionParams {
            text_document_position: position,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: Some(context),
        })
        .ok()?;
    Some(window.spawn(cx, async move |cx| {
        let response = request.receive().await;
        if !cx.update(|_, cx| snapshot.current(cx))? {
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
        let family = hive_core::completion::Family::for_path(&snapshot.path);
        let items = cx
            .background_executor()
            .spawn(async move { completion_items::candidates(items, &source, offset, family) })
            .await;
        if !cx.update(|_, cx| snapshot.current(cx))? {
            return Ok(Some(CompletionResponse::Array(Vec::new())));
        }
        let deferred = items
            .iter()
            .any(|candidate| imports::needs_acceptance(&candidate.raw, resolve));
        tracing::debug!(
            items = items.len(),
            resolve,
            deferred,
            "language completion list"
        );
        if !deferred {
            return Ok(Some(CompletionResponse::Array(
                items.into_iter().map(|c| c.shown).collect(),
            )));
        }
        cx.update(|window, cx| {
            if let Some(document) = snapshot.document.upgrade() {
                let editor = document.read(cx).editor.read(cx);
                if editor.focus_handle(cx).is_focused(window) && editor.cursor() == offset {
                    document.update(cx, |doc, cx| {
                        // GPUI invokes the provider while its Change event is still queued.
                        // At menu publication the editor and document revisions agree.
                        snapshot.revision = Some(doc.revision);
                        doc.import_menu =
                            Some(ImportMenu::new(snapshot, client, items, offset, resolve));
                        cx.notify();
                    });
                }
            }
        })?;
        // Hive handles acceptance of this list for mouse and keyboard. A successful empty list
        // closes the widget's plain-text menu without starting the local-name fallback.
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: false,
            items: vec![],
        })))
    }))
}

// The widget passes the typed identifier as a trigger. The protocol reserves this
// trigger kind for exact characters advertised by the server (for example a dot).
fn completion_context(
    mut context: CompletionContext,
    options: &CompletionOptions,
) -> CompletionContext {
    if context.trigger_kind == CompletionTriggerKind::TRIGGER_CHARACTER
        && !context.trigger_character.as_ref().is_some_and(|trigger| {
            options
                .trigger_characters
                .as_ref()
                .is_some_and(|known| known.contains(trigger))
        })
    {
        context.trigger_kind = CompletionTriggerKind::INVOKED;
        context.trigger_character = None;
    }
    context
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
        lsp.code_action_providers
            .push(Rc::new(fixes::QuickFixes::new(source.clone())));
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
            let Hover { contents, range } = match request.receive().await {
                Ok(Some(hover)) => hover,
                Ok(None) => return Ok(None),
                Err(error) => {
                    tracing::debug!(%error,"hover unavailable");
                    return Ok(None);
                }
            };
            // The popover opens any link it renders; see `language_service::markdown`.
            let contents = cx
                .background_executor()
                .spawn(async move { language_service::markdown::hover(contents) })
                .await;
            if !cx.update(|cx| snapshot.current(cx)) {
                return Ok(None);
            }
            let range = range.and_then(|range| {
                let source = snapshot.text.to_string();
                position::editor_range(&source, range)
            });
            Ok(Some(Hover { contents, range }))
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
