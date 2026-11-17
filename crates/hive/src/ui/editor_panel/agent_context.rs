use super::*;
use crate::{state::documents::DocumentEvent, ui::text_field::TextField};
use gpui_kit::component::{
    IndexPath, WindowExt as _,
    dialog::DialogButtonProps,
    input::{Textarea, TextareaState},
    searchable_list::SearchableVec,
    select::{Select, SelectState},
};
use hive_core::agent::AgentKind;

const WIDTH: Pixels = px(680.);
const ROWS: usize = 16;

pub(super) fn open(document: &Entity<Document>, window: &mut Window, cx: &mut App) {
    let document = document.read(cx);
    let Some(prompt) = document.selection_draft(cx) else {
        return;
    };
    let target = document.target.clone();
    let input = cx.new(|cx| {
        TextareaState::new(window, cx)
            .rows(ROWS)
            .default_value(prompt)
    });
    let agents = AppState::get(cx).agents.clone();
    let sessions = agents
        .read(cx)
        .sessions(&target.host, &target.path)
        .to_vec();
    let local = target.host.is_local();
    let labels: Vec<String> = if local {
        AgentKind::ALL
            .iter()
            .map(|kind| kind.name().to_owned())
            .collect()
    } else {
        sessions
            .iter()
            .map(|session| {
                format!(
                    "{} · {} ({})",
                    session.kind.name(),
                    session.title.as_deref().unwrap_or("Agent"),
                    session.session
                )
            })
            .collect()
    };
    let select = cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(labels.clone()),
            (!labels.is_empty()).then_some(IndexPath::default()),
            window,
            cx,
        )
    });
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let (input, select, labels, target, agents, sessions) = (input.clone(), select.clone(), labels.clone(), target.clone(), agents.clone(), sessions.clone());
        dialog.title("Prepare agent draft").w(WIDTH).max_w(WIDTH)
            .child(TextField::textarea(Textarea::new(&input), &input,cx))
            .child(Select::new(&select).placeholder("Copy only — no running agent").w_full())
            .child(div().text_sm().text_color(cx.theme().muted_foreground).child(if local {"Uses this worktree's preferred agent session, or opens one. The draft is not submitted."} else {"Copies the draft and opens the selected SSH agent. Paste it when ready."}))
            .button_props(DialogButtonProps::default().ok_text(if local {"Prepare draft"} else {"Copy & go to agent"}).show_cancel(true)
                .on_ok(move |_, window,cx| {
                    let prompt = input.read(cx).value().to_string();
                    if prompt.trim().is_empty() { return false; }
                    let index = select.read(cx).selected_value().and_then(|label| labels.iter().position(|candidate| candidate == label));
                    if local {
                        let Some(kind) = index.and_then(|i| AgentKind::ALL.get(i)).copied() else { return false; };
                        let target = target.clone();
                        window.defer(cx,move |_,cx| AppState::get(cx).documents.clone().update(cx,|_,cx| cx.emit(DocumentEvent::AgentDraft(target,kind,prompt))));
                    } else {
                        cx.write_to_clipboard(ClipboardItem::new_string(prompt));
                        if let Some(session) = index.and_then(|i| sessions.get(i)).map(|session| session.session) {
                            let agents=agents.clone(); window.defer(cx,move |_,cx| agents.update(cx,|agents,cx| agents.reveal(session,cx)));
                        }
                    }
                    true
                }))
    });
}
