//! The editor's completion menu through gpui-component's `CompletionProvider` hook.
//!
//! Names of the open file come from a `hive_core::completion::Index` rebuilt on a background
//! thread once typing pauses, so a keystroke only filters a cached list: the menu hides on
//! every edit and must reopen with the same frame, which an asynchronous answer would make
//! flicker. Import paths and imported names need the disk and are answered in the background.
use super::Document;
use gpui_kit::component::highlighter::LanguageRegistry;
use gpui_kit::component::input::{CompletionProvider, InputEdit, Rope, RopeExt as _};
use gpui_kit::*;
use hive_core::code_navigation::{ImportPathKind, ImportPaths, Navigator};
use hive_core::completion::{self, Family, Index, Request};
use hive_core::files;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A burst of keystrokes costs one parse, once typing pauses this long.
const REFRESH_DELAY: Duration = Duration::from_millis(150);
/// Bytes on each side of the cursor that `site_at` reads: a long multi-line import fits.
const SITE_WINDOW: usize = 4 * 1024;

/// Completion for one document; the document re-indexes it on edits.
pub(crate) struct Completions {
    document: WeakEntity<Document>,
    navigator: Arc<Navigator>,
    index: RefCell<Index>,
    /// Dropping it discards a build whose text has changed since.
    refresh: RefCell<Option<Task<()>>>,
}

impl Completions {
    pub(super) fn new(document: WeakEntity<Document>, navigator: Arc<Navigator>) -> Rc<Self> {
        Rc::new(Self {
            document,
            navigator,
            index: RefCell::default(),
            refresh: RefCell::default(),
        })
    }

    /// Re-indexes the document once typing pauses. A later call, or any edit, replaces the
    /// pending build, so an index never describes text older than the current one.
    pub(super) fn schedule_refresh(self: &Rc<Self>, cx: &mut App) {
        let this = Rc::downgrade(self);
        let task = cx.spawn(async move |cx| {
            cx.background_executor().timer(REFRESH_DELAY).await;
            let snapshot = cx.update(|cx| this.upgrade().and_then(|this| this.snapshot(cx)));
            let index = match snapshot {
                Some(snapshot) => {
                    let build = async move { snapshot.build() };
                    cx.background_executor().spawn(build).await
                }
                // Turned off, or renamed to a language without a grammar: drop the old names.
                None => Index::default(),
            };
            if let Some(this) = this.upgrade() {
                *this.index.borrow_mut() = index;
            }
        });
        *self.refresh.borrow_mut() = Some(task);
    }

    /// Keeps the comment and string spans aligned with edits made since the last build.
    pub(super) fn edit(&self, edits: &[(InputEdit, Rope)]) {
        let mut index = self.index.borrow_mut();
        for (edit, _) in edits {
            index.edit(edit.start_byte, edit.old_end_byte, edit.new_end_byte);
        }
    }

    fn snapshot(&self, cx: &App) -> Option<Snapshot> {
        let document = self.document.upgrade()?;
        let document = document.read(cx);
        if !document.completes_code(cx) {
            return None;
        }
        let name = files::editor_language(&document.path);
        let grammar = LanguageRegistry::singleton().language(&name)?.language?;
        let editor = document.editor.read(cx);
        Some(Snapshot {
            text: editor.text().clone(),
            cursor: editor.cursor(),
            grammar,
        })
    }
}

/// What a background build needs, taken from the editor on the main thread.
struct Snapshot {
    text: Rope,
    cursor: usize,
    grammar: tree_sitter::Language,
}

impl Snapshot {
    fn build(self) -> Index {
        let started = Instant::now();
        let text = self.text.to_string();
        let index = match completion::parse(&text, &self.grammar) {
            Some(tree) => Index::build(&tree, &text, Some(self.cursor)),
            None => Index::default(),
        };
        tracing::debug!(bytes = text.len(), elapsed = ?started.elapsed(), "completion index built");
        index
    }
}

impl CompletionProvider for Completions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<gpui_kit::Result<CompletionResponse>> {
        let Some(document) = self.document.upgrade() else {
            return no_items();
        };
        let (host, root, path, enabled) = {
            let document = document.read(cx);
            (
                document.target.host.clone(),
                document.target.path.clone(),
                document.path.clone(),
                document.completes_code(cx),
            )
        };
        if !enabled {
            return no_items();
        }
        let family = Family::for_path(&path);
        match answer(&self.index.borrow(), text, offset, family) {
            Answer::Ready(items) => Task::ready(Ok(CompletionResponse::Array(items))),
            Answer::ImportPath { typed, replace } => {
                let (navigator, text) = (self.navigator.clone(), text.clone());
                cx.background_executor().spawn(async move {
                    let paths = navigator.import_paths(&host, &root, &path, &typed);
                    let items = import_path_items(&text, paths, &typed, &replace);
                    Ok(CompletionResponse::Array(items))
                })
            }
            // Module resolution reads configs and modules on this machine only.
            Answer::ImportName { .. } if !host.is_local() => no_items(),
            Answer::ImportName {
                specifier,
                prefix,
                replace,
            } => {
                let (navigator, text) = (self.navigator.clone(), text.clone());
                cx.background_executor().spawn(async move {
                    let names = navigator.export_names(&root, &path, &specifier, &prefix);
                    let items = names.into_iter().map(|name| {
                        item(
                            &text,
                            name,
                            CompletionItemKind::VARIABLE,
                            None,
                            &replace,
                            &prefix,
                        )
                    });
                    Ok(CompletionResponse::Array(items.collect()))
                })
            }
        }
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, cx: &mut App) -> bool {
        self.document.upgrade().is_some_and(|document| {
            let document = document.read(cx);
            document.completes_code(cx)
                && completion::is_trigger(new_text, Family::for_path(&document.path))
        })
    }
}

/// What the menu shows at `offset`: names ready from the index, or a lookup that needs the disk.
#[derive(Debug)]
enum Answer {
    Ready(Vec<CompletionItem>),
    ImportPath {
        typed: String,
        replace: Range<usize>,
    },
    ImportName {
        specifier: String,
        prefix: String,
        replace: Range<usize>,
    },
}

fn answer(index: &Index, text: &Rope, offset: usize, family: Family) -> Answer {
    let (window, window_start) = window_around(text, offset);
    let Some(site) = completion::site_at(&window, offset - window_start, family) else {
        return Answer::Ready(Vec::new());
    };
    let replace = window_start + site.replace.start..window_start + site.replace.end;
    match site.request {
        Request::Name { .. } if index.is_quiet(offset) => Answer::Ready(Vec::new()),
        Request::Name { member } => {
            let kind = if member {
                CompletionItemKind::PROPERTY
            } else {
                CompletionItemKind::VARIABLE
            };
            let names = index.matches(&site.prefix, member).into_iter();
            Answer::Ready(
                names
                    .map(|name| item(text, name.to_owned(), kind, None, &replace, &site.prefix))
                    .collect(),
            )
        }
        Request::ImportPath => Answer::ImportPath {
            typed: site.prefix,
            replace,
        },
        Request::ImportName { specifier } => Answer::ImportName {
            specifier,
            prefix: site.prefix,
            replace,
        },
    }
}

/// Items for specifiers that continue `typed`; each replaces the specifier from `paths.from`.
fn import_path_items(
    text: &Rope,
    paths: ImportPaths,
    typed: &str,
    replace: &Range<usize>,
) -> Vec<CompletionItem> {
    let replace = replace.start + paths.from..replace.end;
    let typed = typed.get(paths.from..).unwrap_or_default();
    paths
        .items
        .into_iter()
        .map(|path| {
            let (kind, detail) = import_path_kind(path.kind);
            item(text, path.label, kind, detail, &replace, typed)
        })
        .collect()
}

fn no_items() -> Task<gpui_kit::Result<CompletionResponse>> {
    Task::ready(Ok(CompletionResponse::Array(Vec::new())))
}

/// The text within [`SITE_WINDOW`] of `offset`, and where it starts in `text`.
fn window_around(text: &Rope, offset: usize) -> (String, usize) {
    let start = text.floor_char_boundary(offset.saturating_sub(SITE_WINDOW));
    let end = text.ceil_char_boundary(offset.saturating_add(SITE_WINDOW).min(text.len()));
    (text.slice(start..end).to_string(), start)
}

/// An item that replaces `replace` with `label`. Without an explicit edit the menu would
/// replace from where it first opened, eating a `.` or `/` typed to open it.
fn item(
    text: &Rope,
    label: String,
    kind: CompletionItemKind,
    detail: Option<&str>,
    replace: &Range<usize>,
    typed: &str,
) -> CompletionItem {
    let range = lsp_types::Range::new(
        text.offset_to_position(replace.start),
        text.offset_to_position(replace.end),
    );
    CompletionItem {
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: label.clone(),
        })),
        label,
        kind: Some(kind),
        detail: detail.map(str::to_owned),
        // The menu paints this many leading bytes of the label as the match.
        filter_text: Some(typed.to_owned()),
        ..CompletionItem::default()
    }
}

/// The item kind of an import path, and the word the menu shows beside it.
fn import_path_kind(kind: ImportPathKind) -> (CompletionItemKind, Option<&'static str>) {
    match kind {
        ImportPathKind::Directory => (CompletionItemKind::FOLDER, Some("folder")),
        ImportPathKind::File => (CompletionItemKind::FILE, None),
        ImportPathKind::Alias => (CompletionItemKind::MODULE, Some("alias")),
        ImportPathKind::Package => (CompletionItemKind::MODULE, Some("package")),
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the `gpui_kit::*` glob brings in GPUI's own `test` macro.
    use super::{
        Answer, CompletionItem, CompletionItemKind, CompletionTextEdit, Family, ImportPathKind,
        ImportPaths, Index, LanguageRegistry, Rope, answer, completion, import_path_items,
    };
    use hive_core::code_navigation::ImportPath;
    use lsp_types::Position;
    use std::path::Path;

    /// The answer at `|` in `marked`, with the index built from the same text as the editor
    /// builds it: without the word under the cursor.
    fn answer_at(language: &str, marked: &str) -> (Answer, Rope) {
        let offset = marked.find('|').expect("a cursor mark");
        let text = marked.replacen('|', "", 1);
        let grammar = LanguageRegistry::singleton()
            .language(language)
            .and_then(|config| config.language)
            .expect("a bundled grammar");
        let tree = completion::parse(&text, &grammar).expect("parses");
        let index = Index::build(&tree, &text, Some(offset));
        let family = Family::for_path(Path::new(&format!("file.{language}")));
        let rope = Rope::from(text.as_str());
        (answer(&index, &rope, offset, family), rope)
    }

    fn ready(answer: &Answer) -> Vec<&str> {
        let Answer::Ready(items) = answer else {
            panic!("expected names, got {answer:?}");
        };
        items.iter().map(|item| item.label.as_str()).collect()
    }

    fn edit(item: &CompletionItem) -> (Position, Position, &str) {
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("expected a plain edit");
        };
        (edit.range.start, edit.range.end, edit.new_text.as_str())
    }

    #[test]
    fn names_replace_the_whole_word_and_paint_the_typed_part() {
        let (answer, _) = answer_at("ts", "let total = 1;\nconsole.log(to|tal2);\n");
        let Answer::Ready(items) = &answer else {
            panic!("expected names");
        };
        let [item] = items.as_slice() else {
            panic!("expected one item, got {items:?}");
        };
        assert_eq!(item.label, "total");
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(item.filter_text.as_deref(), Some("to"));
        assert_eq!(
            edit(item),
            (Position::new(1, 12), Position::new(1, 18), "total")
        );
    }

    #[test]
    fn members_come_first_after_member_access() {
        let (answer, _) = answer_at("ts", "user.name = 1;\nconst nap = 2;\nuser.na|;\n");
        assert_eq!(ready(&answer), ["name", "nap"]);
        let (answer, _) = answer_at("rs", "let v = Foo::new();\nv.len();\nFoo::|\n");
        assert_eq!(ready(&answer)[..2], ["len", "new"]);
        let (answer, _) = answer_at("py", "self.value = 1\nimport os.path\nself.va|\n");
        assert_eq!(ready(&answer), ["value"]);
    }

    #[test]
    fn comments_and_strings_of_any_language_stay_quiet() {
        for (language, marked) in [
            ("rs", "let total = 1;\n// see tot|\n"),
            ("rs", "let total = 1;\nlet s = \"tot|\";\n"),
            ("py", "total = 1\n\"\"\"\ntot|\n\"\"\"\n"),
            ("ts", "const total = 1;\n/**\n tot|\n */\n"),
        ] {
            let (answer, _) = answer_at(language, marked);
            assert!(ready(&answer).is_empty(), "{language}: {marked:?}");
        }
        let (answer, _) = answer_at("rs", "let total = 1;\ntot|\n");
        assert_eq!(ready(&answer), ["total"]);
    }

    #[test]
    fn far_cursors_map_back_from_the_window() {
        let marked = format!("{}\nconst value = 1;\nval|", "€".repeat(2000));
        let (answer, _) = answer_at("ts", &marked);
        let Answer::Ready(items) = &answer else {
            panic!("expected names");
        };
        assert_eq!(
            edit(&items[0]),
            (Position::new(2, 0), Position::new(2, 3), "value")
        );
    }

    #[test]
    fn import_paths_replace_the_segment_under_the_cursor() {
        let (answer, rope) = answer_at("tsx", "import x from './co|mp';\n");
        let Answer::ImportPath { typed, replace } = answer else {
            panic!("expected an import path, got {answer:?}");
        };
        assert_eq!(typed, "./co");
        let paths = ImportPaths {
            from: 2,
            items: vec![
                ImportPath {
                    label: "components".into(),
                    kind: ImportPathKind::Directory,
                },
                ImportPath {
                    label: "config".into(),
                    kind: ImportPathKind::File,
                },
            ],
        };
        let items = import_path_items(&rope, paths, &typed, &replace);
        assert_eq!(
            edit(&items[0]),
            (Position::new(0, 17), Position::new(0, 21), "components")
        );
        assert_eq!(items[0].detail.as_deref(), Some("folder"));
        assert_eq!(items[1].detail, None);
        assert_eq!(items[1].filter_text.as_deref(), Some("co"));
    }

    #[test]
    fn named_imports_ask_the_module() {
        let (answer, _) = answer_at("ts", "import { us| } from 'react';\n");
        let Answer::ImportName {
            specifier,
            prefix,
            replace,
        } = answer
        else {
            panic!("expected an import name, got {answer:?}");
        };
        assert_eq!((specifier.as_str(), prefix.as_str()), ("react", "us"));
        assert_eq!(replace, 9..11);
    }

    #[test]
    fn languages_without_names_offer_nothing() {
        let (answer, _) = answer_at("md", "# Title\n\nSome te|\n");
        assert!(ready(&answer).is_empty());
    }
}
