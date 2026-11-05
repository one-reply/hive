use super::*;
use crate::{
    app::AppState,
    state::{documents::OpenFile, git::Target},
};
use gpui_kit::component::{
    IndexPath,
    list::{List, ListDelegate, ListItem, ListState},
};
use std::cell::Cell;

const WIDTH: Pixels = px(600.);
const HEIGHT: Pixels = px(420.);

struct Paths {
    target: Target,
    files: Entity<Files>,
    paths: Vec<PathBuf>,
    filtered: Vec<usize>,
    selected: Option<IndexPath>,
    query: String,
    status: String,
}
impl Paths {
    fn filter(&mut self) {
        self.filtered = self
            .paths
            .iter()
            .enumerate()
            .filter(|(_, path)| path.to_string_lossy().to_lowercase().contains(&self.query))
            .map(|(index, _)| index)
            .collect();
        self.selected = (!self.filtered.is_empty()).then_some(IndexPath::default());
    }
}
impl ListDelegate for Paths {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.filtered.len()
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let path = self.paths.get(*self.filtered.get(ix.row)?)?;
        let settings = self.files.read(cx).settings.clone();
        Some(
            ListItem::new(ix.row)
                .aria_label(path.to_string_lossy().into_owned())
                .py_1()
                .text_size(rems(settings.font_size() / 16.))
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(icons::render(
                            path,
                            false,
                            false,
                            settings.icons,
                            rems(ICON_SIZE * settings.font_size() / LABEL_SIZE / 16.),
                            cx,
                        ))
                        .child(
                            v_flex()
                                .min_w_0()
                                .child(
                                    div().truncate().child(
                                        path.file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned(),
                                    ),
                                )
                                .child(
                                    div()
                                        .text_size(rems(
                                            META_SIZE * settings.font_size() / LABEL_SIZE / 16.,
                                        ))
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(
                                            path.parent()
                                                .unwrap_or(Path::new(""))
                                                .to_string_lossy()
                                                .into_owned(),
                                        ),
                                ),
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
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_lowercase();
        self.filter();
        Task::ready(())
    }
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if self.files.read(cx).target.as_ref() != Some(&self.target) {
            window.close_dialog(cx);
            return;
        }
        let Some(path) = self
            .selected
            .and_then(|ix| self.filtered.get(ix.row))
            .and_then(|ix| self.paths.get(*ix))
            .cloned()
        else {
            return;
        };
        let target = self.target.clone();
        window.close_dialog(cx);
        window.defer(cx, move |_, cx| {
            AppState::get(cx)
                .documents
                .clone()
                .update(cx, |documents, cx| {
                    documents.request(
                        OpenFile {
                            target,
                            path,
                            line: None,
                            preview: false,
                        },
                        cx,
                    )
                })
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
        div().p_4().child(if self.status.is_empty() {
            "No matching files".to_owned()
        } else {
            self.status.clone()
        })
    }
}

pub(crate) fn open(files: Entity<Files>, window: &mut Window, cx: &mut App) {
    let Some((target, task)) = files.read(cx).index(cx) else {
        return;
    };
    let list = cx.new(|cx| {
        ListState::new(
            Paths {
                target,
                files,
                paths: Vec::new(),
                filtered: Vec::new(),
                selected: None,
                query: String::new(),
                status: "Loading files…".into(),
            },
            window,
            cx,
        )
        .searchable(true)
    });
    let weak = list.downgrade();
    let load = window.spawn(cx, async move |cx| {
        let result = task.await;
        let _ = cx.update(|_, cx| {
            weak.update(cx, |list, cx| {
                let paths = list.delegate_mut();
                if paths.files.read(cx).target.as_ref() != Some(&paths.target) {
                    paths.paths.clear();
                    paths.filtered.clear();
                    paths.status = "Worktree changed. Open file search again.".into();
                    cx.notify();
                    return;
                }
                match result {
                    Ok(index) => {
                        paths.paths = index.paths;
                        paths.status = if index.truncated {
                            "Index limited to 100,000 files"
                        } else {
                            ""
                        }
                        .into();
                        paths.filter();
                    }
                    Err(error) => paths.status = error,
                }
                cx.notify();
            })
        });
    });
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        let _keep_alive = &load;
        if first.replace(false) {
            let list = list.clone();
            window.defer(cx, move |window, cx| {
                list.update(cx, |list, cx| list.focus(window, cx))
            });
        }
        dialog
            .title("Open file")
            .w(WIDTH)
            .when(!list.read(cx).delegate().status.is_empty(), |dialog| {
                dialog.child(
                    div()
                        .text_xs()
                        .child(list.read(cx).delegate().status.clone()),
                )
            })
            .child(
                List::new(&list)
                    .h(HEIGHT)
                    .search_placeholder("Search file name or path…"),
            )
    });
}
