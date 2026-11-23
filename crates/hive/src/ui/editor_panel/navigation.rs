use super::*;
use crate::state::documents::DocumentEvent;
use gpui_kit::component::{
    IndexPath, WindowExt as _,
    list::{List, ListDelegate, ListItem, ListState},
};
use hive_core::symbols::Symbol;
use std::{cell::Cell, sync::Arc};

const WIDTH: Pixels = px(580.);
const HEIGHT: Pixels = px(360.);

pub(super) fn breadcrumbs(
    document: &Entity<Document>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let d = document.read(cx);
    let target = d.target.clone();
    let path = d.path.clone();
    let current = hive_core::symbols::containing(&d.symbols, d.editor.read(cx).cursor())
        .filter(|_| d.symbols_text.as_ref() == Some(d.editor.read(cx).text()))
        .map(|symbol| symbol.name.clone());
    let mut row = h_flex()
        .id("file-breadcrumbs")
        .flex_1()
        .min_w_0()
        .overflow_x_scroll()
        .gap_1();
    let root = target
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut parts = vec![(root, std::path::PathBuf::new())];
    let mut relative = std::path::PathBuf::new();
    for component in path.components() {
        relative.push(component);
        parts.push((
            component.as_os_str().to_string_lossy().into_owned(),
            relative.clone(),
        ));
    }
    for (index, (label, path)) in parts.into_iter().enumerate() {
        let target = target.clone();
        let hover = Hover::new(("breadcrumb", index), window, cx);
        if index > 0 {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child("/"),
            );
        }
        row = row.child(
            Button::new(("breadcrumb", index))
                .flex_shrink_0()
                .small()
                .cursor_pointer()
                .label(label)
                .tooltip("Reveal in Files")
                .on_click(move |_, _, cx| {
                    AppState::get(cx).documents.clone().update(cx, |_, cx| {
                        cx.emit(DocumentEvent::Reveal(target.clone(), path.clone()))
                    });
                })
                .smooth_hover(ButtonLook::Ghost, &hover, cx)
                .flex_shrink_0(),
        );
    }
    let document = document.clone();
    let hover = Hover::new("symbol-picker", window, cx);
    row.child(
        Button::new("symbol-picker")
            .flex_shrink_0()
            .small()
            .cursor_pointer()
            .icon(Lucide::ChevronRight)
            .label(current.unwrap_or_else(|| "Symbols".into()))
            .tooltip(crate::ui::key_hint::tooltip(
                "Go to symbol",
                &super::GoToSymbol,
                super::EDITOR_PANEL_CONTEXT,
                window,
            ))
            .on_click(move |_, window, cx| open(&document, window, cx))
            .smooth_hover(ButtonLook::Ghost, &hover, cx)
            .flex_shrink_0(),
    )
}

struct Symbols {
    document: Entity<Document>,
    symbols: Arc<Vec<Symbol>>,
    text: gpui_kit::component::input::Rope,
    rows: Vec<usize>,
    selected: Option<IndexPath>,
    query: String,
}

impl Symbols {
    fn refresh(&mut self, cx: &App) {
        let d = self.document.read(cx);
        self.text = d.editor.read(cx).text().clone();
        self.symbols = if d.symbols_text.as_ref() == Some(&self.text) {
            d.symbols.clone()
        } else {
            Arc::default()
        };
        self.rows = self
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(index, symbol)| {
                symbol
                    .name
                    .to_lowercase()
                    .contains(&self.query)
                    .then_some(index)
            })
            .collect();
    }
}

impl ListDelegate for Symbols {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.rows.len()
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let symbol = self.symbols.get(*self.rows.get(ix.row)?)?;
        Some(
            ListItem::new(ix.row).child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().truncate().child(format!(
                        "{}{}",
                        "  ".repeat(symbol.depth.min(8)),
                        symbol.name
                    )))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(symbol.kind.clone()),
                    ),
            ),
        )
    }
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }
    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_lowercase();
        self.refresh(cx);
        Task::ready(())
    }
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(symbol) = self
            .selected
            .and_then(|ix| self.rows.get(ix.row))
            .and_then(|ix| self.symbols.get(*ix))
        else {
            return;
        };
        let d = self.document.read(cx);
        if d.editor.read(cx).text() != &self.text {
            window.close_dialog(cx);
            return;
        }
        let request = OpenFile {
            target: d.target.clone(),
            path: d.path.clone(),
            position: Some(self.text.offset_to_position(symbol.offset)),
            preview: false,
            placement: Default::default(),
        };
        window.close_dialog(cx);
        window.defer(cx, move |_, cx| {
            AppState::get(cx)
                .documents
                .clone()
                .update(cx, |documents, cx| documents.request(request, cx))
        });
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.close_dialog(cx);
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().p_4().child("No matching symbols")
    }
}

pub(super) fn open(document: &Entity<Document>, window: &mut Window, cx: &mut App) {
    let d = document.read(cx);
    let mut delegate = Symbols {
        document: document.clone(),
        symbols: d.symbols.clone(),
        text: d.editor.read(cx).text().clone(),
        rows: (0..d.symbols.len()).collect(),
        selected: None,
        query: String::new(),
    };
    delegate.refresh(cx);
    let list = cx.new(|cx| ListState::new(delegate, window, cx).searchable(true));
    list.update(cx, |list, cx| {
        list.set_selected_index(Some(IndexPath::default()), window, cx)
    });
    let focus = Cell::new(true);
    let weak = list.downgrade();
    let subscription = cx.observe(document, move |_, cx| {
        let _ = weak.update(cx, |list, cx| {
            list.delegate_mut().refresh(cx);
            cx.notify();
        });
    });
    window.open_dialog(cx, move |dialog, window, cx| {
        let _keep_alive = &subscription;
        if focus.replace(false) {
            let list = list.clone();
            window.defer(cx, move |window, cx| {
                list.update(cx, |list, cx| list.focus(window, cx))
            });
        }
        dialog.title("Go to symbol").w(WIDTH).max_w(WIDTH).child(
            List::new(&list)
                .h(HEIGHT)
                .search_placeholder("Find a function, class, type…"),
        )
    });
}
