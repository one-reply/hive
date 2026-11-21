//! A decision the user has to justify: one dialog whose note is required.
use super::*;

impl RunView {
    /// Asks for the note behind a decision about `subject`, then applies `decide` to the run.
    /// An empty note keeps the dialog open.
    pub(super) fn ask_reason(
        title: &'static str,
        subject: String,
        placeholder: &'static str,
        w: &mut Window,
        cx: &mut Context<Self>,
        decide: impl Fn(&mut Run, String) -> Result<(), String> + 'static,
    ) {
        let input = area("", placeholder, w, cx);
        let view = cx.weak_entity();
        let decide = Rc::new(decide);
        w.open_alert_dialog(cx, move |d, _, cx| {
            let (input, view, decide) = (input.clone(), view.clone(), decide.clone());
            d.title(title)
                .child(subtle(subject.clone(), cx))
                .child(paragraph("Details", &input, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Save")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let note = input.read(cx).value().trim().to_owned();
                            if note.is_empty() {
                                return false;
                            }
                            let decide = decide.clone();
                            let _ = view.update(cx, |v, cx| v.edit(cx, move |r| decide(r, note)));
                            true
                        }),
                )
        });
    }
}
