//! Selecting an auto-import dispatches to Document for both keyboard and mouse.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::base::input::{MoveDown, MoveUp};
use gpui_kit::component::{
    input::{Enter, Escape, IndentInline},
    text::TextView,
};
use lsp_types::{Documentation, MarkupKind};

const WIDTH: Pixels = px(440.);
const LIST_HEIGHT: Pixels = px(196.);
const DOC_HEIGHT: Pixels = px(140.);

pub(super) fn keys(
    body: Stateful<Div>,
    document: &Entity<Document>,
    cx: &Context<EditorPanel>,
) -> Stateful<Div> {
    body.capture_action(
        cx.listener(|this, _: &MoveUp, window, cx| this.move_import(false, window, cx)),
    )
    .capture_action(
        cx.listener(|this, _: &MoveDown, window, cx| this.move_import(true, window, cx)),
    )
    .capture_action(accept_on_enter(document.clone()))
    .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
        if accept(&this.document, None, window, cx) {
            window.prevent_default();
            cx.stop_propagation();
        }
    }))
    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
        if this.document.update(cx, |doc, cx| {
            let had = doc.import_menu.take().is_some();
            if had {
                cx.notify();
            }
            had
        }) {
            cx.stop_propagation();
        }
    }))
}

pub(crate) fn accept_on_enter(
    document: Entity<Document>,
) -> impl Fn(&Enter, &mut Window, &mut App) + 'static {
    move |action, window, cx| {
        if Enter::is_primary(action) && accept(&document, None, window, cx) {
            window.prevent_default();
            cx.stop_propagation();
        }
    }
}

fn accept(
    document: &Entity<Document>,
    index: Option<usize>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !document.read(cx).editor.focus_handle(cx).is_focused(window) {
        return false;
    }
    let Some(menu) = document.update(cx, |doc, cx| {
        let menu = doc.import_menu.take();
        if menu.is_some() {
            cx.notify();
        }
        menu
    }) else {
        return false;
    };
    let index = index.unwrap_or(menu.selected);
    menu.accept(index, window, cx);
    true
}

impl EditorPanel {
    fn move_import(&self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .document
            .read(cx)
            .editor
            .focus_handle(cx)
            .is_focused(window)
        {
            return;
        }
        self.document.update(cx, |doc, cx| {
            if let Some(menu) = &mut doc.import_menu {
                menu.step(down);
                self.import_scroll.scroll_to_item(menu.selected);
                cx.notify();
                cx.stop_propagation();
            }
        });
    }

    pub(super) fn render_imports(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let doc = self.document.read(cx);
        let menu = doc.import_menu.as_ref()?;
        let editor = doc.editor.read(cx);
        if editor.cursor() != menu.cursor || !editor.focus_handle(cx).is_focused(window) {
            return None;
        }
        let bounds = editor.range_to_bounds(&(menu.cursor..menu.cursor))?;
        let selected = menu.selected;
        let items: Vec<_> = menu
            .items()
            .map(|item| (item.label.clone(), item.detail.clone()))
            .collect();
        let documentation = menu.items().nth(selected)?.documentation.clone();
        let theme = cx.theme();
        let (bg, muted, selected_bg, border) = (
            theme.background,
            theme.muted_foreground,
            theme.accent,
            theme.border,
        );
        let scroll = self.import_scroll.clone();
        let list = v_flex()
            .id("import-candidates")
            .role(Role::ListBox)
            .aria_label("Code suggestions")
            .track_scroll(&scroll)
            .max_h(LIST_HEIGHT)
            .overflow_y_scroll()
            .children(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, (label, detail))| {
                        let hover = Hover::new(("import-suggestion-hover", index), window, cx);
                        h_flex()
                            .id(("import-suggestion", index))
                            .role(Role::ListBoxOption)
                            .aria_label(label.clone())
                            .w_full()
                            .cursor_pointer()
                            .gap_2()
                            .px_2()
                            .py_0p5()
                            .text_sm()
                            .child(div().flex_shrink_0().child(label))
                            .children(detail.map(|detail| {
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_right()
                                    .text_color(muted)
                                    .child(detail)
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    window.prevent_default();
                                    cx.stop_propagation();
                                    accept(&this.document, Some(index), window, cx);
                                }),
                            )
                            .hover_bg(
                                &hover,
                                if index == selected { selected_bg } else { bg },
                                selected_bg,
                            )
                    }),
            );
        let card = v_flex()
            .id("language-completions")
            .occlude()
            .w(WIDTH)
            .max_w(window.viewport_size().width)
            .bg(bg)
            .border_1()
            .border_color(border)
            .shadow_md()
            .rounded_md()
            .overflow_hidden()
            .child(list)
            .children(documentation.map(|docs| {
                let content = match docs {
                    Documentation::MarkupContent(docs) if docs.kind == MarkupKind::Markdown => {
                        TextView::markdown("language-completion-docs", docs.value)
                            .into_any_element()
                    }
                    Documentation::MarkupContent(docs) => {
                        div().child(docs.value).into_any_element()
                    }
                    Documentation::String(plain) => div().child(plain).into_any_element(),
                };
                div()
                    .id("language-completion-detail")
                    .max_h(DOC_HEIGHT)
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(border)
                    .px_2()
                    .py_1()
                    .text_sm()
                    .child(content)
            }));
        Some(
            deferred(
                anchored()
                    .position(point(bounds.left(), bounds.bottom()))
                    .snap_to_window_with_margin(px(4.))
                    .child(card),
            )
            .into_any_element(),
        )
    }
}
