//! ⌘-hover / ⌘-click Go to Definition for imports, through gpui-component's LSP hooks: a
//! `DefinitionProvider` backed by `hive_core::code_navigation` and a `show_document` handler
//! that opens the target in a Hive editor tab instead of jumping inside the current buffer.
use super::{Document, Documents, OpenFile, TabPlacement};
use gpui_kit::component::input::{
    DefinitionProvider, EditorState, Rope, RopeExt as _, ShowDocumentHandler,
};
use gpui_kit::*;
use hive_core::code_navigation::{self, Navigator};
use hive_core::host::Host;
use lsp_types::{LocationLink, Position, Range, ShowDocumentParams, Uri};
use std::ops::Range as Span;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr as _;
use std::sync::{Arc, Mutex, PoisonError};

/// Enables Go to Definition on a local JavaScript/TypeScript document.
pub(super) fn install(
    document: &Entity<Document>,
    documents: WeakEntity<Documents>,
    navigator: Arc<Navigator>,
    cx: &mut App,
) {
    let (editor, supported) = {
        let document = document.read(cx);
        let supported =
            document.target.host == Host::Local && code_navigation::is_supported(&document.path);
        (document.editor.clone(), supported)
    };
    if !supported {
        return;
    }
    let provider = Rc::new(ImportDefinitions {
        document: document.downgrade(),
        navigator,
        last: Arc::default(),
    });
    let show = show_document(document.downgrade(), documents);
    editor.update(cx, |editor: &mut EditorState, _| {
        let lsp = editor.lsp_mut();
        lsp.definition_provider = Some(provider);
        lsp.show_document = Some(show);
    });
}

/// The last answered word: ⌘-hover asks again on every mouse move until it gets a hit.
struct Answer {
    text: Rope,
    word: Span<usize>,
    links: Vec<LocationLink>,
}

struct ImportDefinitions {
    document: WeakEntity<Document>,
    navigator: Arc<Navigator>,
    last: Arc<Mutex<Option<Answer>>>,
}

impl DefinitionProvider for ImportDefinitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<Vec<LocationLink>>> {
        // Read at request time: a rename or move changes the document's path.
        let Some((root, path)) = self.document.upgrade().map(|document| {
            let document = document.read(cx);
            (document.target.path.clone(), document.path.clone())
        }) else {
            return Task::ready(Ok(Vec::new()));
        };
        let word = text.word_range(offset).unwrap_or(offset..offset);
        if let Some(answer) = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|answer| answer.word == word && answer.text == *text)
        {
            return Task::ready(Ok(answer.links.clone()));
        }
        let (text, navigator, last) = (text.clone(), self.navigator.clone(), self.last.clone());
        cx.background_executor().spawn(async move {
            let links = links(&navigator, &root, &path, &text, offset);
            *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Answer {
                text,
                word,
                links: links.clone(),
            });
            Ok(links)
        })
    }
}

fn links(
    navigator: &Navigator,
    root: &Path,
    path: &Path,
    text: &Rope,
    offset: usize,
) -> Vec<LocationLink> {
    let definition = match navigator.definition(root, path, &text.to_string(), offset) {
        Ok(Some(definition)) => definition,
        Ok(None) => return Vec::new(),
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "go to definition failed");
            return Vec::new();
        }
    };
    let Some(target_uri) = file_uri(&root.join(&definition.path)) else {
        tracing::debug!(path = %definition.path.display(), "definition path is not a valid URI");
        return Vec::new();
    };
    let target = Position::new(definition.line, definition.column);
    vec![LocationLink {
        origin_selection_range: Some(Range::new(
            text.offset_to_position(definition.origin.start),
            text.offset_to_position(definition.origin.end),
        )),
        target_uri,
        target_range: Range::new(target, target),
        target_selection_range: Range::new(target, target),
    }]
}

/// Opens another file of the same worktree as a pinned tab at the definition: in place of the
/// clicked tab, or beside it with ⇧ held. The current file falls back to gpui-component's
/// in-buffer jump.
fn show_document(
    document: WeakEntity<Document>,
    documents: WeakEntity<Documents>,
) -> ShowDocumentHandler {
    Rc::new(
        move |params: &ShowDocumentParams, window: &mut Window, cx: &mut App| {
            let source = document.clone();
            let Some(document) = document.upgrade() else {
                return false;
            };
            let (target, current) = {
                let document = document.read(cx);
                (document.target.clone(), document.path.clone())
            };
            let Some(path) = file_path(&params.uri)
                .and_then(|path| path.strip_prefix(&target.path).ok().map(Path::to_path_buf))
            else {
                return false;
            };
            if path == current {
                return false;
            }
            let request = OpenFile {
                target,
                path,
                position: params.selection.map(|selection| selection.start),
                preview: false,
                placement: if window.modifiers().shift {
                    TabPlacement::Beside(source)
                } else {
                    TabPlacement::Replacing(source)
                },
            };
            // Called while the clicked editor is being updated; opening a tab reads every editor.
            let documents = documents.clone();
            cx.defer(move |cx| {
                if let Err(error) =
                    documents.update(cx, |documents, cx| documents.request(request, cx))
                {
                    tracing::debug!(%error, "documents closed before go to definition");
                }
            });
            true
        },
    )
}

/// Bytes kept verbatim in a `file://` path; everything else is percent-encoded.
fn is_path_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&byte)
}

fn file_uri(path: &Path) -> Option<Uri> {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if is_path_safe(byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&uri).ok()
}

fn file_path(uri: &Uri) -> Option<PathBuf> {
    let encoded = uri.as_str().strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut ix = 0;
    while ix < encoded.len() {
        if encoded[ix] == b'%' {
            let hex = std::str::from_utf8(encoded.get(ix + 1..ix + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            ix += 3;
        } else {
            bytes.push(encoded[ix]);
            ix += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::{Navigator, Rope, Uri, file_path, file_uri, links};
    use lsp_types::Position;
    use std::path::Path;
    use std::str::FromStr as _;

    #[test]
    fn link_points_from_the_clicked_word_to_the_declaring_file() {
        let root = std::env::temp_dir().join(format!("hive-definitions-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/b.ts"), "\nexport const Привіт = 1;\n").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let text = "import { Привіт } from './b';\nПривіт;\n";
        let rope = Rope::from(text);
        let offset = text.rfind("Привіт").unwrap() + 2;
        let found = links(
            &Navigator::default(),
            &root,
            Path::new("src/a.ts"),
            &rope,
            offset,
        );
        std::fs::remove_dir_all(&root).unwrap();
        let [link] = found.as_slice() else {
            panic!("expected one link, got {found:?}");
        };
        let origin = link.origin_selection_range.unwrap();
        assert_eq!(
            (origin.start, origin.end),
            (Position::new(1, 0), Position::new(1, 6))
        );
        assert_eq!(link.target_selection_range.start, Position::new(1, 13));
        assert_eq!(file_path(&link.target_uri), Some(root.join("src/b.ts")));
    }

    #[test]
    fn file_uri_round_trips_spaces_unicode_and_percent() {
        let path = Path::new("/Users/me/my repo/src/Привіт 100%#?.ts");
        let uri = file_uri(path).expect("valid uri");
        assert!(!uri.as_str().contains(' '));
        assert_eq!(file_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn non_file_uris_are_not_paths() {
        let uri = Uri::from_str("https://example.com/a").expect("valid uri");
        assert_eq!(file_path(&uri), None);
    }
}
