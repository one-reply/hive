use super::*;
use crate::state::documents::SourcePreview;
use gpui_kit::component::{WindowExt as _, dialog::DialogButtonProps, spinner::Spinner};

const WIDTH: Pixels = px(1000.);
const HEIGHT: Pixels = px(600.);

pub(super) struct PreviewView {
    source: Entity<SourcePreview>,
    _observer: Subscription,
}
pub(super) fn view(source: Entity<SourcePreview>, cx: &mut App) -> Entity<PreviewView> {
    cx.new(|cx| {
        let observer = cx.observe(&source, |_, _, cx| cx.notify());
        PreviewView {
            source,
            _observer: observer,
        }
    })
}
impl Render for PreviewView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.source.read(cx);
        v_flex()
            .size_full()
            .min_h_0()
            .when(source.loading, |body| body.child(Spinner::new()))
            .when_some(source.error.clone(), |body, error| {
                body.child(div().p_3().child(error))
            })
            .child(
                Editor::new(&source.editor)
                    .readonly(true)
                    .bordered(false)
                    .size_full(),
            )
    }
}

pub(crate) fn open(
    target: hive_core::projects::Target,
    path: std::path::PathBuf,
    position: gpui_kit::component::input::Position,
    window: &mut Window,
    cx: &mut App,
) {
    let source = AppState::get(cx)
        .documents
        .clone()
        .update(cx, |documents, cx| {
            documents.preview(target, path, position, window, cx)
        });
    show(source, window, cx);
}

pub(super) fn show(source: Entity<SourcePreview>, window: &mut Window, cx: &mut App) {
    let title = format!("{} · read only", source.read(cx).path.display());
    let content = view(source.clone(), cx);
    window.open_alert_dialog(cx, move |dialog, _, _| {
        dialog
            .title(title.clone())
            .w(WIDTH)
            .max_w(WIDTH)
            .child(div().h(HEIGHT).child(content.clone()))
            .button_props(DialogButtonProps::default().ok_text("Close"))
    });
}
