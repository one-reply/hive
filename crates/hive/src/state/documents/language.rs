use super::*;
use gpui_kit::component::input::Rope;
use hive_core::language_service::{self, Event, Family, Session, position};
use lsp_types::{
    notification::{DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument},
    *,
};
use std::collections::HashMap;

mod diagnostics;
#[cfg(test)]
mod relocation_tests;

struct Synced {
    text: Rope,
    version: i32,
}

pub(crate) struct LanguageSession {
    pub host: hive_core::host::Host,
    pub root: PathBuf,
    pub family: Family,
    pub capabilities: Option<ServerCapabilities>,
    pub error: Option<String>,
    pub diagnostics: HashMap<String, PublishDiagnosticsParams>,
    diagnostics_limited: bool,
    client: Session,
    documents: HashMap<Uri, Synced>,
    _events: Option<Task<()>>,
}

impl LanguageSession {
    fn saved(&self, uri: Uri, text: Option<String>) {
        let save = self
            .capabilities
            .as_ref()
            .and_then(|caps| caps.text_document_sync.as_ref())
            .and_then(|sync| match sync {
                TextDocumentSyncCapability::Options(options) => options.save.as_ref(),
                _ => None,
            });
        let include = match save {
            Some(TextDocumentSyncSaveOptions::Supported(true)) => false,
            Some(TextDocumentSyncSaveOptions::SaveOptions(options)) => {
                options.include_text.unwrap_or(false)
            }
            _ => return,
        };
        if let Err(error) = self
            .client
            .notify::<lsp_types::notification::DidSaveTextDocument>(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri },
                text: if include { text } else { None },
            })
        {
            tracing::warn!(%error,"language-server save notification failed");
        }
    }
    fn new(
        host: hive_core::host::Host,
        root: PathBuf,
        family: Family,
        cx: &mut Context<Self>,
    ) -> Self {
        let (client, events) = Session::start(host.clone(), root.clone(), family);
        let mut this = Self {
            host,
            root,
            family,
            capabilities: None,
            error: None,
            diagnostics: HashMap::new(),
            diagnostics_limited: false,
            client,
            documents: HashMap::new(),
            _events: None,
        };
        this.listen(events, cx);
        this
    }

    fn listen(&mut self, events: async_channel::Receiver<Event>, cx: &mut Context<Self>) {
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match event {
                            Event::Ready(capabilities) => {
                                this.capabilities = Some(*capabilities);
                                this.error = None;
                                this.documents.clear();
                                this.diagnostics.clear();
                                this.diagnostics_limited = false;
                            }
                            Event::Failed(error) => {
                                this.capabilities = None;
                                this.error = Some(error.to_string());
                                this.documents.clear();
                                this.diagnostics.clear();
                                this.diagnostics_limited = false;
                            }
                            Event::Diagnostics(params) => {
                                this.receive_diagnostics(params);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.capabilities = None;
                this.documents.clear();
                this.diagnostics.clear();
                this.diagnostics_limited = false;
                if this.error.is_none() {
                    this.error = Some("Language server stopped. Retry to reconnect.".into());
                }
                cx.notify();
            });
        }));
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        let (client, events) = Session::start(self.host.clone(), self.root.clone(), self.family);
        self.client = client;
        self.capabilities = None;
        self.error = None;
        self.documents.clear();
        self.diagnostics.clear();
        self.listen(events, cx);
        cx.notify();
    }

    pub(super) fn sync(
        &mut self,
        uri: Uri,
        path: &std::path::Path,
        text: &Rope,
    ) -> Result<Session, language_service::Error> {
        let capabilities =
            self.capabilities
                .as_ref()
                .ok_or(language_service::Error::Unavailable(
                    "initializing or stopped",
                ))?;
        if !synchronizes_buffers(capabilities.text_document_sync.as_ref()) {
            return Err(language_service::Error::Unavailable(
                "server does not synchronize buffers",
            ));
        }
        match self.documents.get(&uri) {
            Some(previous) if previous.text == *text => {}
            Some(previous) => {
                let version =
                    previous
                        .version
                        .checked_add(1)
                        .ok_or(language_service::Error::Unavailable(
                            "document version exhausted; retry",
                        ))?;
                self.client
                    .notify::<DidChangeTextDocument>(DidChangeTextDocumentParams {
                        text_document: VersionedTextDocumentIdentifier {
                            uri: uri.clone(),
                            version,
                        },
                        content_changes: vec![TextDocumentContentChangeEvent {
                            range: None,
                            range_length: None,
                            text: text.to_string(),
                        }],
                    })?;
                self.documents.insert(
                    uri,
                    Synced {
                        text: text.clone(),
                        version,
                    },
                );
            }
            None => {
                self.client
                    .notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
                        text_document: TextDocumentItem {
                            uri: uri.clone(),
                            language_id: Family::language_id(path).into(),
                            version: 1,
                            text: text.to_string(),
                        },
                    })?;
                self.documents.insert(
                    uri,
                    Synced {
                        text: text.clone(),
                        version: 1,
                    },
                );
            }
        }
        Ok(self.client.clone())
    }

    pub(super) fn close(&mut self, uri: &Uri) {
        if self.documents.remove(uri).is_some()
            && let Err(error) =
                self.client
                    .notify::<DidCloseTextDocument>(DidCloseTextDocumentParams {
                        text_document: TextDocumentIdentifier { uri: uri.clone() },
                    })
        {
            tracing::debug!(%error,"language document close failed");
        }
        self.diagnostics.remove(uri.as_str());
    }
}

impl Documents {
    pub(super) fn attach_language(&mut self, document: &Entity<Document>, cx: &mut Context<Self>) {
        let (target, path) = {
            let d = document.read(cx);
            (d.target.clone(), d.path.clone())
        };
        let Some(family) = Family::for_path(&path) else {
            return;
        };
        self.language_sessions
            .retain(|(_, _, _, session)| session.upgrade().is_some());
        let session = self
            .language_sessions
            .iter()
            .find(|(host, root, kind, _)| {
                *host == target.host && *root == target.path && *kind == family
            })
            .and_then(|(_, _, _, session)| session.upgrade())
            .unwrap_or_else(|| {
                let session = cx.new(|cx| {
                    LanguageSession::new(target.host.clone(), target.path.clone(), family, cx)
                });
                self.language_sessions.push((
                    target.host.clone(),
                    target.path.clone(),
                    family,
                    session.downgrade(),
                ));
                session
            });
        document.update(cx, |document, cx| document.attach_language(session, cx));
    }
}

impl Document {
    pub(super) fn language_saved(&self, cx: &App) {
        if let Some(session) = &self.language
            && let Some(uri) = position::uri(&self.target.path.join(&self.path))
        {
            session.read(cx).saved(
                uri,
                self.base.as_ref().map(hive_core::files::Snapshot::text),
            );
        }
    }

    pub(super) fn reset_language(&mut self, cx: &mut Context<Self>) {
        self.close_language(cx);
        self._language_subscription = None;
        self._language_sync = None;
        self.language = None;
        self.diagnostics.clear();
        self.editor.update(cx, |editor, cx| {
            if let Some(diagnostics) = editor.diagnostics_mut() {
                diagnostics.clear();
            }
            cx.notify();
        });
        let document = cx.entity();
        cx.defer(move |cx| {
            crate::app::AppState::get(cx)
                .documents
                .clone()
                .update(cx, |documents, cx| documents.attach_language(&document, cx))
        });
    }
    fn attach_language(&mut self, session: Entity<LanguageSession>, cx: &mut Context<Self>) {
        self._language_subscription = Some(cx.observe(&session, |this, _, cx| {
            this.sync_language(cx);
            this.apply_diagnostics(cx);
            cx.notify();
        }));
        self.language = Some(session);
        self.sync_language(cx);
    }

    pub(super) fn schedule_language(&mut self, cx: &mut Context<Self>) {
        self.diagnostics.clear();
        self.editor.update(cx, |editor, cx| {
            if let Some(diagnostics) = editor.diagnostics_mut() {
                diagnostics.clear();
            }
            cx.notify();
        });
        self._language_sync = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |this, cx| this.sync_language(cx));
        }));
    }

    pub(super) fn sync_language(&mut self, cx: &mut Context<Self>) {
        if self.base.is_none() || self.loading || self.path_busy {
            return;
        }
        let text = self.editor.read(cx).text().clone();
        let _ = self.language_client(&text, cx);
    }

    pub(super) fn language_client(&self, text: &Rope, cx: &mut App) -> Option<(Session, Uri)> {
        let session = self.language.as_ref()?;
        if Family::for_path(&self.path) != Some(session.read(cx).family) {
            return None;
        }
        let uri = position::uri(&self.target.path.join(&self.path))?;
        session.read(cx).capabilities.as_ref()?;
        let client = session.update(cx, |session, cx| {
            match session.sync(uri.clone(), &self.path, text) {
                Ok(client) => Some(client),
                Err(error) => {
                    tracing::warn!(%error, "language buffer synchronization failed");
                    session.error = Some(error.to_string());
                    session.capabilities = None;
                    session.diagnostics.clear();
                    cx.notify();
                    None
                }
            }
        })?;
        Some((client, uri))
    }

    fn apply_diagnostics(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).text().clone();
        let uri = position::uri(&self.target.path.join(&self.path));
        let diagnostics = self
            .language
            .as_ref()
            .zip(uri.as_ref())
            .map(|(session, uri)| session.read(cx).current_diagnostics(uri, &text))
            .unwrap_or_default();
        if self.diagnostics == diagnostics {
            return;
        }
        let source = text.to_string();
        let converted: Vec<_> = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                let mut diagnostic = diagnostic.clone();
                diagnostic.range = position::editor_range(&source, diagnostic.range)?;
                Some(diagnostic)
            })
            .collect();
        self.diagnostics = diagnostics;
        self.editor.update(cx, |editor, cx| {
            if let Some(diagnostics) = editor.diagnostics_mut() {
                diagnostics.clear();
                diagnostics.extend(converted);
            }
            cx.notify();
        });
    }

    pub(super) fn close_language(&self, cx: &mut App) {
        if let Some(session) = &self.language
            && let Some(uri) = position::uri(&self.target.path.join(&self.path))
        {
            session.update(cx, |session, _| session.close(&uri));
        }
    }
}

fn synchronizes_buffers(sync: Option<&TextDocumentSyncCapability>) -> bool {
    match sync {
        Some(TextDocumentSyncCapability::Kind(kind)) => *kind != TextDocumentSyncKind::NONE,
        Some(TextDocumentSyncCapability::Options(options)) => {
            options.open_close == Some(true)
                && options
                    .change
                    .is_some_and(|kind| kind != TextDocumentSyncKind::NONE)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::synchronizes_buffers;
    use lsp_types::{
        TextDocumentSyncCapability as Sync, TextDocumentSyncKind as Kind, TextDocumentSyncOptions,
    };
    #[test]
    fn only_servers_that_receive_unsaved_buffers_enable_language_features() {
        assert!(!synchronizes_buffers(None));
        assert!(!synchronizes_buffers(Some(&Sync::Kind(Kind::NONE))));
        assert!(synchronizes_buffers(Some(&Sync::Kind(Kind::FULL))));
        let mut options = TextDocumentSyncOptions {
            change: Some(Kind::INCREMENTAL),
            ..Default::default()
        };
        assert!(!synchronizes_buffers(Some(&Sync::Options(options.clone()))));
        options.open_close = Some(true);
        assert!(synchronizes_buffers(Some(&Sync::Options(options))));
    }
}
