//! The project's launch draft as the library lists it.
use super::runs::{RowFace, row_frame};
use super::*;
use hive_core::pipeline::draft::LaunchDraft;

const UNTITLED_DRAFT: &str = "Untitled draft";

/// A draft is named by its task, and one whose task is still empty by what it is.
pub(in crate::ui::workspace::pipelines) fn draft_title(task: &str) -> String {
    let title = run::task_title(task);
    if title.is_empty() {
        UNTITLED_DRAFT.into()
    } else {
        title
    }
}

/// The draft's row; a click reopens the launch form with it, as **Resume draft** does.
pub(super) fn draft_row(
    draft: &LaunchDraft,
    window: &mut Window,
    cx: &mut Context<PipelinePanel>,
) -> AnyElement {
    let id = "library-draft";
    let hover = Hover::with_duration(id, WIDE_HOVER_DURATION, window, cx);
    let face = RowFace {
        icon: Lucide::Pencil,
        color: cx.theme().muted_foreground,
        title: draft_title(&draft.task),
        detail: draft.template.name.clone(),
        pill: "Draft",
    };
    on_activate(row_frame(id.into(), face, None, cx), cx, |p, w, cx| {
        p.show_draft(w, cx)
    })
    .hover_bg(&hover, cx.theme().background, cx.theme().muted)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{UNTITLED_DRAFT, draft_title};

    #[test]
    fn a_draft_is_named_by_the_first_line_of_its_task() {
        assert_eq!(draft_title("  Add login\nwith details"), "Add login");
        assert_eq!(draft_title(""), UNTITLED_DRAFT);
        assert_eq!(draft_title(" \n "), UNTITLED_DRAFT);
    }
}
