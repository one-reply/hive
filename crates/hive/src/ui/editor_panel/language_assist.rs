use super::*;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use std::time::Duration;

impl EditorPanel {
    pub(super) fn jump_diagnostic(
        &self,
        previous: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let documents = AppState::get(cx).documents.clone();
        let Some(task) = documents
            .read(cx)
            .adjacent_diagnostic(&self.document, previous, cx)
        else {
            window.push_notification(Notification::info("No diagnostics in this worktree."), cx);
            return;
        };
        let editor = self.document.read(cx).editor.clone();
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                cx.update(|window, cx| {
                    if !editor.focus_handle(cx).is_focused(window) {
                        return;
                    }
                    match result {
                        Ok(request) => documents.update(cx, |docs, cx| docs.request(request, cx)),
                        Err(error) => window.push_notification(Notification::error(error), cx),
                    }
                })
            })
            .detach_and_log_err(cx);
    }

    pub(super) fn signature_help(
        &mut self,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let document = self.document.clone();
        let origin = document.read(cx).editor.read(cx).cursor();
        self._signature = Some(cx.spawn_in(window, async move |this, cx| {
            if !explicit {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let Ok(request) = cx.update(|_, cx| Document::signature(&document, cx)) else {
                return;
            };
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this._signature = None;
                let editor = document.read(cx).editor.read(cx);
                if editor.cursor() != origin || !editor.focus_handle(cx).is_focused(window) {
                    return;
                }
                this.signature = match result {
                    Ok(signature) => signature,
                    Err(error) => {
                        tracing::debug!(%error,"signature help unavailable");
                        None
                    }
                };
                if explicit && this.signature.is_none() {
                    window.push_notification(
                        Notification::info("No signature available at the cursor."),
                        cx,
                    );
                }
                cx.notify();
            });
        }));
    }

    pub(super) fn render_signature(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        let signature = self.signature.as_ref()?;
        let editor = self.document.read(cx).editor.read(cx);
        if !editor.focus_handle(cx).is_focused(window) {
            return None;
        }
        let bounds = editor.range_to_bounds(&(editor.cursor()..editor.cursor()))?;
        let highlights = signature.active.iter().cloned().map(|range| {
            (
                range,
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    color: Some(cx.theme().primary),
                    ..Default::default()
                },
            )
        });
        let hint = div()
            .id("call-signature")
            .aria_label("Call signature")
            .px_2()
            .py_1()
            .rounded_md()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_md()
            .text_sm()
            .font_family(cx.theme().mono_font_family.clone())
            .child(StyledText::new(signature.label.clone()).with_highlights(highlights));
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .position(bounds.origin)
                    .snap_to_window_with_margin(px(4.))
                    .child(hint),
            )
            .into_any_element(),
        )
    }
}
