//! Local folder selection and SSH project onboarding share every Add Project entry point.
use crate::{
    app::AppState,
    state::{remote_project::RemoteProject, store::Store},
    ui::{
        hover::{ButtonLook, Hover, SmoothButton as _},
        text_field::TextField,
    },
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

const WIDTH: Pixels = px(520.);
const FOLDERS_HEIGHT: Pixels = px(240.);
const FOLDER_HEIGHT: Pixels = px(28.);

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, |dialog, window, cx| {
        let local_hover = Hover::new("add-local", window, cx);
        let ssh_hover = Hover::new("add-ssh", window, cx);
        dialog.title("Add project").w(WIDTH).child(
            h_flex()
                .gap_3()
                .child(
                    Button::new("add-local")
                        .cursor_pointer()
                        .label("Local folder…")
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            Store::prompt_add_projects(AppState::get(cx).store.clone(), cx);
                        })
                        .smooth_hover(ButtonLook::Outline, &local_hover, cx),
                )
                .child(
                    Button::new("add-ssh")
                        .cursor_pointer()
                        .label("Through SSH…")
                        .on_click(|_, window, cx| {
                            window.close_dialog(cx);
                            window.defer(cx, ssh);
                        })
                        .smooth_hover(ButtonLook::Outline, &ssh_hover, cx),
                ),
        )
    });
}

fn ssh(window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| RemoteForm::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Add SSH project").w(WIDTH).child(form.clone())
    });
}

struct RemoteForm {
    state: Entity<RemoteProject>,
    host: Entity<InputState>,
    path: Entity<InputState>,
    folders: ListState,
    _subscription: Subscription,
}

impl RemoteForm {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|_| RemoteProject::new());
        let host = cx
            .new(|cx| InputState::new(window, cx).placeholder("SSH alias or user@host, e.g. home"));
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("~/projects"));
        let subscription = cx.observe_in(&state, window, |this, state, window, cx| {
            if let Some((_, directory)) = state.read(cx).directory() {
                this.folders.reset(
                    directory.children.len() + usize::from(directory.path.parent().is_some()),
                );
                let value = directory.path.to_string_lossy().into_owned();
                this.path
                    .update(cx, |input, cx| input.set_value(value, window, cx));
            }
            cx.notify();
        });
        Self {
            state,
            host,
            path,
            folders: ListState::new(0, ListAlignment::Top, FOLDER_HEIGHT),
            _subscription: subscription,
        }
    }
}

impl Render for RemoteForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let busy = state.busy();
        let connected = state.directory().is_some();
        let directory = state.directory().map(|(_, directory)| directory.clone());
        let error = state.error.clone();
        let connect_hover = Hover::new("ssh-connect", window, cx);
        let browse_hover = Hover::new("ssh-browse", window, cx);
        let add_hover = Hover::new("ssh-add", window, cx);
        let cancel_hover = Hover::new("ssh-cancel", window, cx);
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "Uses your SSH config and keys. Git and agents run on your Linux machine.",
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(TextField::input(
                        Input::new(&self.host).disabled(busy || connected),
                        &self.host,
                        cx,
                    )))
                    .child(
                        Button::new("ssh-connect")
                            .cursor_pointer()
                            .label(if connected { "Change host" } else { "Connect" })
                            .loading(busy && !connected)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let target = this.host.read(cx).value().to_string();
                                this.state.update(cx, |state, cx| {
                                    if state.directory().is_some() {
                                        state.disconnect(cx);
                                    } else {
                                        state.connect(target, cx);
                                    }
                                });
                            }))
                            .smooth_hover(ButtonLook::Outline, &connect_hover, cx),
                    ),
            )
            .when(connected, |body| {
                body.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(TextField::input(
                            Input::new(&self.path).disabled(busy),
                            &self.path,
                            cx,
                        )))
                        .child(
                            Button::new("ssh-browse")
                                .cursor_pointer()
                                .label("Open folder")
                                .loading(busy)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let path = PathBuf::from(this.path.read(cx).value().as_str());
                                    this.state.update(cx, |state, cx| state.browse(path, cx));
                                }))
                                .smooth_hover(ButtonLook::Outline, &browse_hover, cx),
                        ),
                )
            })
            .when_some(directory, |body, directory| {
                let mut paths = Vec::new();
                if let Some(parent) = directory.path.parent() {
                    paths.push(("..".to_owned(), parent.to_owned()));
                }
                paths.extend(directory.children.into_iter().map(|path| {
                    (
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        path,
                    )
                }));
                let view = cx.entity().downgrade();
                body.child(
                    list(self.folders.clone(), move |index, window, cx| {
                        let (name, path) = &paths[index];
                        view.update(cx, |_, cx| {
                            let hover = Hover::new(("remote-folder", index), window, cx);
                            let path = path.clone();
                            Button::new(("remote-folder", index))
                                .cursor_pointer()
                                .label(name.clone())
                                .disabled(busy)
                                .small()
                                .w_full()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.browse(path.clone(), cx))
                                }))
                                .smooth_hover(ButtonLook::Ghost, &hover, cx)
                                .into_any_element()
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                    })
                    .h(FOLDERS_HEIGHT)
                    .w_full(),
                )
            })
            .when_some(error, |body, error| {
                body.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("ssh-cancel")
                            .cursor_pointer()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx),
                    )
                    .child(
                        Button::new("ssh-add")
                            .cursor_pointer()
                            .label("Add this folder")
                            .disabled(!connected || busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.state.update(cx, |state, cx| state.add(cx)) {
                                    window.close_dialog(cx);
                                }
                            }))
                            .smooth_hover(ButtonLook::Outline, &add_hover, cx),
                    ),
            )
    }
}
