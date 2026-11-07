//! Local folder selection and SSH project onboarding share every Add Project entry point.
use crate::{
    app::AppState,
    state::{
        remote_project::{RemoteProject, known_hosts},
        store::Store,
    },
    ui::{
        hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _},
        text_field::TextField,
    },
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::motion::{MotionReveal, Transition, transition};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::host::ssh_config::SshHost;
use std::{path::PathBuf, time::Duration};

const WIDTH: Pixels = px(520.);
const FOLDERS_HEIGHT: Pixels = px(240.);
const FOLDER_HEIGHT: Pixels = px(28.);
const HOSTS_MAX_HEIGHT: Pixels = px(264.);
const ICON_TILE: Pixels = px(32.);
const ROW_BACKGROUND_OPACITY: f32 = 0.03;
const ROW_HOVER_OPACITY: f32 = 0.07;
const ROW_BORDER_OPACITY: f32 = 0.08;
const UNFOLD_DURATION: Duration = Duration::from_millis(220);
const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

pub(crate) fn open(window: &mut Window, cx: &mut App) {
    let chooser = cx.new(|cx| Chooser::new(window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Add project").w(WIDTH).child(chooser.clone())
    });
}

/// The SSH form, connecting at once when a known host was picked.
fn ssh(target: Option<SharedString>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| RemoteForm::new(target, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Add SSH project").w(WIDTH).child(form.clone())
    });
}

/// Picks where the project lives: a local folder, a known SSH host, or another one.
struct Chooser {
    /// `None` while `~/.ssh/config` is still being read.
    hosts: Option<Vec<SshHost>>,
    /// The "Another SSH host" block unfolds in place into this field.
    other: Entity<InputState>,
    adding_other: bool,
    _task: Task<()>,
    _subscription: Subscription,
}

impl Chooser {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let load = known_hosts(cx);
        let task = cx.spawn(async move |this, cx| {
            let hosts = load.await;
            let _ = this.update(cx, |this, cx| {
                this.hosts = Some(hosts);
                cx.notify();
            });
        });
        let other = cx
            .new(|cx| InputState::new(window, cx).placeholder("user@host or an alias, e.g. home"));
        let subscription =
            cx.subscribe_in(&other, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.connect_other(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            });
        Self {
            hosts: None,
            other,
            adding_other: false,
            _task: task,
            _subscription: subscription,
        }
    }

    fn set_adding_other(&mut self, adding: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_other = adding;
        if adding {
            self.other.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn other_target(&self, cx: &App) -> Option<SharedString> {
        let value = self.other.read(cx).value();
        let target = value.trim();
        (!target.is_empty()).then(|| SharedString::from(target.to_owned()))
    }

    fn connect_other(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.other_target(cx) else {
            return;
        };
        window.close_dialog(cx);
        window.defer(cx, move |window, cx| ssh(Some(target), window, cx));
    }

    fn render_hosts(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let hosts = v_flex().gap_1p5().flex_shrink_0();
        let list = match &self.hosts {
            None => return hosts.child(note("Reading ~/.ssh/config…", cx)),
            Some(list) if list.is_empty() => {
                return hosts.child(note("No hosts in ~/.ssh/config yet.", cx));
            }
            Some(list) => list,
        };
        hosts.children(list.iter().enumerate().map(|(index, host)| {
            let target = SharedString::from(host.alias.clone());
            option_row(
                ("add-ssh-host", index),
                row_content(
                    Lucide::Server.into(),
                    target.clone(),
                    host.address().map(Into::into),
                    Icon::new(Lucide::ChevronRight),
                    cx,
                ),
                window,
                cx,
            )
            .aria_label(target.clone())
            .on_click(move |_, window, cx| {
                let target = target.clone();
                window.close_dialog(cx);
                window.defer(cx, move |window, cx| ssh(Some(target), window, cx));
            })
        }))
    }

    /// A row that unfolds into a host field instead of opening another dialog.
    fn render_other(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let progress = transition(
            "add-ssh-other-progress",
            if self.adding_other { 1.0_f32 } else { 0. },
            Transition::new(UNFOLD_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let hover = Hover::new("add-ssh-other", window, cx).enabled(!self.adding_other);
        let can_connect = self.other_target(cx).is_some();
        let connect_hover = Hover::new("add-ssh-other-connect", window, cx).enabled(can_connect);
        // The library's disabled primary is a pale tile with a pale arrow; until there is a
        // target the button sits back like the field next to it.
        let connect_look = if can_connect {
            ButtonLook::Primary
        } else {
            ButtonLook::Outline
        };
        let fg = cx.theme().foreground;
        let header =
            row_content(
                Icon::new(Lucide::Plus).rotate(radians(QUARTER_TURN / 2. * progress)),
                "Another SSH host…",
                Some("user@host or any alias ssh understands".into()),
                Icon::new(Lucide::ChevronRight).rotate(radians(QUARTER_TURN * progress)),
                cx,
            )
            .id("add-ssh-other-header")
            .role(Role::Button)
            .aria_label("Another SSH host")
            .px_3()
            .py_2()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, window, cx| {
                this.set_adding_other(!this.adding_other, window, cx)
            }));
        // MotionReveal lays the form out as its own root, where an auto width shrinks to content.
        // The header's × already collapses the block, so the row only needs a submit button.
        // MotionReveal also clips to its bounds, so the top padding keeps the field's focus
        // ring, drawn outside the field, from being cut off.
        let form = h_flex()
            .w_full()
            .gap_2()
            .px_3()
            .pt_1()
            .pb_3()
            .opacity(progress)
            .child(div().flex_1().min_w_0().child(TextField::input(
                Input::new(&self.other),
                &self.other,
                cx,
            )))
            .child(
                Button::new("add-ssh-other-connect")
                    .cursor_pointer()
                    .icon(Lucide::ArrowRight)
                    .tooltip("Connect (Enter)")
                    .disabled(!can_connect)
                    .on_click(cx.listener(|this, _, window, cx| this.connect_other(window, cx)))
                    .smooth_hover(connect_look, &connect_hover, cx),
            );
        v_flex()
            .id("add-ssh-other-card")
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(fg.opacity(ROW_BORDER_OPACITY))
            .hover_bg(
                &hover,
                fg.opacity(ROW_BACKGROUND_OPACITY),
                fg.opacity(ROW_HOVER_OPACITY),
            )
            .child(header)
            .child(MotionReveal::new(
                "add-ssh-other-form",
                progress,
                form.into_any_element(),
            ))
    }
}

impl Render for Chooser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local = option_row(
            "add-local",
            row_content(
                Lucide::FolderOpen.into(),
                "Local folder",
                Some("Open a folder on this Mac".into()),
                Icon::new(Lucide::ChevronRight),
                cx,
            ),
            window,
            cx,
        )
        .aria_label("Local folder")
        .on_click(|_, window, cx| {
            window.close_dialog(cx);
            Store::prompt_add_projects(AppState::get(cx).store.clone(), cx);
        });
        v_flex().gap_4().child(local).child(
            v_flex()
                .gap_2()
                .child(section_label("SSH hosts", cx))
                .child(
                    div()
                        .id("add-ssh-hosts")
                        .max_h(HOSTS_MAX_HEIGHT)
                        .overflow_y_scroll()
                        .child(self.render_hosts(window, cx)),
                )
                .child(self.render_other(window, cx)),
        )
    }
}

/// A bordered, clickable choice around [`row_content`].
fn option_row(
    id: impl Into<ElementId>,
    content: Div,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let hover = Hover::new(id.clone(), window, cx);
    let fg = cx.theme().foreground;
    content
        .id(id)
        .role(Role::Button)
        .w_full()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(fg.opacity(ROW_BORDER_OPACITY))
        .hover_bg(
            &hover,
            fg.opacity(ROW_BACKGROUND_OPACITY),
            fg.opacity(ROW_HOVER_OPACITY),
        )
        .cursor_pointer()
}

/// Icon tile, title, optional muted detail and a trailing icon.
fn row_content(
    icon: Icon,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
    trailing: Icon,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let fg = theme.foreground;
    h_flex()
        .gap_3()
        .child(
            div()
                .size(ICON_TILE)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(fg.opacity(ROW_HOVER_OPACITY))
                .child(icon.size_4().text_color(fg)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(title.into()),
                )
                .when_some(detail, |column, detail| {
                    column.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .truncate()
                            .child(detail),
                    )
                }),
        )
        .child(trailing.size_4().text_color(theme.muted_foreground))
}

fn section_label(text: &'static str, cx: &App) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn note(text: &'static str, cx: &App) -> Div {
    div()
        .px_1()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

struct RemoteForm {
    state: Entity<RemoteProject>,
    host: Entity<InputState>,
    path: Entity<InputState>,
    folders: ListState,
    _subscription: Subscription,
}

impl RemoteForm {
    fn new(target: Option<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|_| RemoteProject::new());
        let host = cx.new(|cx| {
            let input =
                InputState::new(window, cx).placeholder("SSH alias or user@host, e.g. home");
            match &target {
                Some(target) => input.default_value(target.clone()),
                None => input,
            }
        });
        if let Some(target) = target {
            state.update(cx, |state, cx| state.connect(target.to_string(), cx));
        }
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
