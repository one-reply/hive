//! The two kinds of result rows. They share the tree's row height, hover and selection tint,
//! so the panel reads the same in both search modes.
use super::super::{
    ICON_SIZE, LABEL_SIZE, META_SIZE, icons,
    row::{CHEVRON_DURATION, CHEVRON_OPEN, SELECTED_BACKGROUND_OPACITY, SELECTED_BORDER_OPACITY},
    search::match_highlight,
};
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::Icon;
use hive_core::{files::content_search::LineMatch, settings::FilesSettings};

/// Monospace digits are about this wide relative to the font size.
const DIGIT_WIDTH: f32 = 0.62;
/// Line rows start under the file name, past the chevron column.
const LINE_INDENT: f32 = 20.;

/// What a file header shows.
pub(super) struct Header {
    pub path: PathBuf,
    pub count: usize,
    pub collapsed: bool,
}

pub(super) fn file(
    header: Header,
    selected: bool,
    settings: &FilesSettings,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let key: SharedString = format!("content-file:{}", header.path.display()).into();
    let scale = settings.font_size() / LABEL_SIZE;
    let turn = transition(
        (key.clone(), "chevron"),
        if header.collapsed { 0. } else { CHEVRON_OPEN },
        Transition::new(CHEVRON_DURATION),
        window,
        cx,
    );
    let name = header
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parent = header
        .path
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default();
    shell(key, selected, settings, 4., window, cx)
        .child(
            Icon::new(Lucide::ChevronRight)
                .rotate(radians(turn))
                .size(rems(12. * scale / 16.))
                .flex_none()
                .text_color(cx.theme().muted_foreground),
        )
        .child(icons::render(
            &header.path,
            false,
            false,
            settings.icons,
            rems(ICON_SIZE * scale / 16.),
            cx,
        ))
        .child(
            div()
                .flex_none()
                .max_w(relative(0.7))
                .truncate()
                .font_weight(FontWeight::MEDIUM)
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_ellipsis_start()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(rems(META_SIZE * scale / 16.))
                .text_color(cx.theme().muted_foreground)
                .child(parent),
        )
        .child(
            div()
                .flex_none()
                .px(rems(5. / 16.))
                .rounded_full()
                .bg(cx.theme().muted)
                .text_size(rems(META_SIZE * scale / 16.))
                .text_color(cx.theme().muted_foreground)
                .child(header.count.to_string()),
        )
}

pub(super) fn line(
    key: String,
    found: LineMatch,
    digits: usize,
    selected: bool,
    settings: &FilesSettings,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let scale = settings.font_size() / LABEL_SIZE;
    let meta = META_SIZE * scale;
    let style = match_highlight(cx);
    let highlights: Vec<_> = found
        .ranges
        .iter()
        .map(|range| (range.clone(), style))
        .collect();
    let mono = cx.theme().mono_font_family.clone();
    shell(
        key.into(),
        selected,
        settings,
        LINE_INDENT * scale,
        window,
        cx,
    )
    .child(
        div()
            .flex_none()
            .w(rems(digits as f32 * meta * DIGIT_WIDTH / 16.))
            .text_right()
            .font_family(mono.clone())
            .text_size(rems(meta / 16.))
            .text_color(cx.theme().muted_foreground)
            .child((found.line + 1).to_string()),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .font_family(mono)
            .text_size(rems((settings.font_size() - 1.) / 16.))
            .child(StyledText::new(found.preview).with_highlights(highlights)),
    )
}

/// A row of the tree's height with its hover and selection look.
fn shell(
    key: SharedString,
    selected: bool,
    settings: &FilesSettings,
    indent: f32,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (sidebar, foreground) = (theme.sidebar, theme.foreground);
    let font = theme.font_family.clone();
    let background = if selected {
        sidebar.blend(foreground.opacity(SELECTED_BACKGROUND_OPACITY))
    } else {
        sidebar
    };
    let hovered = if selected {
        background
    } else {
        background.blend(foreground.opacity(0.06))
    };
    let hover = Hover::new(key.clone(), window, cx);
    h_flex()
        .id(key)
        .w_full()
        .min_w_0()
        .h(rems(settings.row_height() / 16.))
        .gap(rems(6. / 16.))
        .pl(rems(indent / 16.))
        .pr_2()
        .rounded_sm()
        .border_1()
        .border_color(if selected {
            sidebar.blend(foreground.opacity(SELECTED_BORDER_OPACITY))
        } else {
            background
        })
        .font_family(font)
        .text_size(rems(settings.font_size() / 16.))
        .text_color(foreground)
        .cursor_pointer()
        .hover_bg(&hover, background, hovered)
}
