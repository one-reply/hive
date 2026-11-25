use super::*;
use crate::state::documents::SymbolLocation;
use gpui_kit::component::{
    IndexPath, WindowExt as _,
    list::{List, ListDelegate, ListItem, ListState},
    notification::Notification,
};
use hive_core::projects::Target;

struct Symbols {
    target: Target,
    rows: Vec<SymbolLocation>,
    query: String,
    error: Option<String>,
    selected: Option<IndexPath>,
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
        _: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let item = self.rows.get(ix.row)?;
        Some(
            ListItem::new(ix.row).child(
                h_flex().gap_3().child(item.name.clone()).child(
                    div()
                        .flex_1()
                        .truncate()
                        .child(item.path.display().to_string()),
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
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_owned();
        self.rows.clear();
        self.selected = None;
        self.error = None;
        if query.trim().is_empty() {
            return Task::ready(());
        }
        let registry = AppState::get(cx).documents.clone();
        let target = self.target.clone();
        let query = self.query.clone();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            let Some(task) = cx
                .update(|_, cx| {
                    registry.update(cx, |reg, cx| {
                        reg.workspace_symbols(&target, query.clone(), cx)
                    })
                })
                .ok()
            else {
                return;
            };
            let result = task.await;
            let _ = this.update_in(cx, |list, window, cx| {
                let data = list.delegate_mut();
                if data.query != query {
                    return;
                }
                match result {
                    Ok(rows) => data.rows = rows,
                    Err(e) => data.error = Some(e),
                }
                if !data.rows.is_empty() {
                    list.set_selected_index(Some(IndexPath::default()), window, cx);
                }
                cx.notify();
            });
        })
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().p_3().child(self.error.clone().unwrap_or_else(|| {
            if self.query.is_empty() {
                "Type a symbol name".into()
            } else {
                "No matching symbols".into()
            }
        }))
    }
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(item) = self.selected.and_then(|ix| self.rows.get(ix.row)).cloned() else {
            return;
        };
        let registry = AppState::get(cx).documents.clone();
        let task = registry
            .read(cx)
            .resolve_symbol(self.target.clone(), item, cx);
        window.close_dialog(cx);
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                let _ = cx.update(|window, cx| match result {
                    Ok(request) => registry.update(cx, |reg, cx| reg.request(request, cx)),
                    Err(error) => window.push_notification(Notification::error(error), cx),
                });
            })
            .detach();
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.close_dialog(cx);
    }
}

pub(in crate::ui) fn open(target: Target, window: &mut Window, cx: &mut App) {
    let list = cx.new(|cx| {
        ListState::new(
            Symbols {
                target,
                rows: vec![],
                query: String::new(),
                error: None,
                selected: None,
            },
            window,
            cx,
        )
        .searchable(true)
    });
    let focus = list.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Go to workspace symbol").child(
            List::new(&list)
                .h(px(360.))
                .search_placeholder("Search declarations…"),
        )
    });
    window.defer(cx, move |window, cx| {
        focus.update(cx, |l, cx| l.focus(window, cx))
    });
}
