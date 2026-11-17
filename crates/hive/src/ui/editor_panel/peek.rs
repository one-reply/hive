use super::*;
use crate::state::documents::SourcePreview;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use lsp_types::LocationLink;

const WIDTH: Pixels = px(680.);
const HEIGHT: Pixels = px(340.);
const MARGIN: Pixels = px(8.);

pub(super) struct Peek {
    links: Vec<LocationLink>,
    selected: usize,
    source: Entity<SourcePreview>,
    view: Entity<source_preview::PreviewView>,
    anchor: Point<Pixels>,
    origin: gpui_kit::component::input::Rope,
}

impl EditorPanel {
    pub(super) fn peek_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let anchor = {
            let editor = self.document.read(cx).editor.read(cx);
            editor
                .visible_row_range()
                .filter(|rows| rows.contains(&(editor.cursor_position().line as usize)))
                .and_then(|_| editor.range_to_bounds(&(editor.cursor()..editor.cursor())))
                .map(|bounds| point(bounds.left(), bounds.bottom() + MARGIN))
                .unwrap_or(editor.input_bounds().origin)
        };
        let task = Document::definitions(&self.document, window, cx);
        let origin = self.document.read(cx).editor.read(cx).text().clone();
        self._peek = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.document.read(cx).editor.read(cx).text() != &origin {
                    return;
                }
                match result {
                    Ok(links) if !links.is_empty() => {
                        this.install_peek(links, 0, anchor, window, cx)
                    }
                    Ok(_) => window.push_notification(
                        Notification::info(
                            "No definition found. Language tools may still be starting.",
                        ),
                        cx,
                    ),
                    Err(error) => {
                        window.push_notification(Notification::error(error.to_string()), cx)
                    }
                }
            });
        }));
    }

    fn install_peek(
        &mut self,
        links: Vec<LocationLink>,
        selected: usize,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = links.get(selected) else {
            return;
        };
        let Some(path) = hive_core::language_service::position::path(&link.target_uri) else {
            return;
        };
        let target = self.document.read(cx).target.clone();
        let source = AppState::get(cx)
            .documents
            .clone()
            .update(cx, |documents, cx| {
                documents.preview(target, path, link.target_selection_range.start, window, cx)
            });
        let view = source_preview::view(source.clone(), cx);
        self.peek = Some(Peek {
            links,
            selected,
            source,
            view,
            anchor,
            origin: self.document.read(cx).editor.read(cx).text().clone(),
        });
        cx.notify();
    }

    pub(super) fn close_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._peek = None;
        self.peek = None;
        self.document
            .read(cx)
            .editor
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn render_peek(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let peek = self.peek.as_ref()?;
        if self.document.read(cx).editor.read(cx).text() != &peek.origin {
            return None;
        }
        let title = peek
            .source
            .read(cx)
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let [previous, next, open, close] =
            ["peek-previous", "peek-next", "peek-open", "peek-close"]
                .map(|id| Hover::new(id, window, cx));
        let count = peek.links.len();
        let selected = peek.selected;
        let card = v_flex()
            .id("definition-peek")
            // Without this a click in the peek also lands on the editor underneath, which
            // moves its caret and so closes the peek.
            .occlude()
            .w(WIDTH)
            .h(HEIGHT)
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .shadow_md()
            .overflow_hidden()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.close_peek(window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_action(|_: &SaveFile, _, cx| cx.stop_propagation())
            .on_action(|_: &FormatDocument, _, cx| cx.stop_propagation())
            .on_action(|_: &FindReferences, _, cx| cx.stop_propagation())
            .on_action(|_: &GoToSymbol, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| this.close_peek(window, cx)))
            .child(
                h_flex()
                    .p_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(format!("{title} · definition {}/{count}", selected + 1)),
                    )
                    .child(
                        Button::new("peek-previous")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::ChevronLeft)
                            .tooltip("Previous definition")
                            .disabled(selected == 0)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(peek) = &this.peek {
                                    this.install_peek(
                                        peek.links.clone(),
                                        peek.selected.saturating_sub(1),
                                        peek.anchor,
                                        window,
                                        cx,
                                    );
                                }
                            }))
                            .smooth_hover(ButtonLook::Ghost, &previous, cx),
                    )
                    .child(
                        Button::new("peek-next")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::ChevronRight)
                            .tooltip("Next definition")
                            .disabled(selected + 1 == count)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(peek) = &this.peek {
                                    this.install_peek(
                                        peek.links.clone(),
                                        peek.selected + 1,
                                        peek.anchor,
                                        window,
                                        cx,
                                    );
                                }
                            }))
                            .smooth_hover(ButtonLook::Ghost, &next, cx),
                    )
                    .child(
                        Button::new("peek-open")
                            .small()
                            .cursor_pointer()
                            .label("Open")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let Some(peek) = this.peek.take() else {
                                    return;
                                };
                                let request = peek.source.read(cx).open_request();
                                if let Some(request) = request {
                                    AppState::get(cx)
                                        .documents
                                        .clone()
                                        .update(cx, |documents, cx| documents.request(request, cx));
                                } else {
                                    source_preview::show(peek.source, window, cx);
                                }
                                cx.notify();
                            }))
                            .smooth_hover(ButtonLook::Ghost, &open, cx),
                    )
                    .child(
                        Button::new("peek-close")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::X)
                            .tooltip("Close peek (Esc)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_peek(window, cx)),
                            )
                            .smooth_hover(ButtonLook::Ghost, &close, cx),
                    ),
            )
            .child(div().flex_1().min_h_0().child(peek.view.clone()));
        Some(
            deferred(
                anchored()
                    .position(peek.anchor)
                    .snap_to_window_with_margin(MARGIN)
                    .child(card),
            )
            .into_any_element(),
        )
    }
}
