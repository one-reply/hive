use super::*;

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [save_hover, changes_hover, conflict_hover, sources_hover] =
            ["file-save", "file-changes", "file-conflict", "file-sources"]
                .map(|id| Hover::new(id, window, cx));
        let document = self.document.read(cx);
        let editor = document.editor.clone();
        let header = format!(
            "{} · {}",
            document.path.display(),
            document.target.path.display()
        );
        let (loading, saving, dirty, ready) = (
            document.loading,
            document.saving,
            document.dirty,
            document.base.is_some() && !document.path_busy,
        );
        let error = document
            .error
            .as_ref()
            .map(|error| error.message().to_owned());
        let conflict = document.conflict.is_some();
        let raster = document.content.is_raster();
        let svg_mode = match document.content {
            Content::Svg { mode, .. } => Some(mode),
            _ => None,
        };
        let image = document.content.shows_image().then(|| {
            image_preview::render(
                &document
                    .content
                    .preview()
                    .expect("image content has a preview")
                    .state,
                cx,
            )
        });
        let source_error = match &document.content {
            Content::Svg { source_error, .. } => source_error.clone(),
            _ => None,
        };
        let mut body = v_flex()
            .id("file-content")
            .role(Role::Group)
            .aria_label(header.clone())
            .key_context("EditorPanel")
            .track_focus(&self.image_focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| {
                this.document.update(cx, |d, cx| {
                    d.save(None, window, cx);
                });
            }))
            .on_action(
                cx.listener(|this, _: &CloseFile, window, cx| this.request_close(window, cx)),
            )
            .child(
                h_flex()
                    .id("file-toolbar")
                    .role(Role::Toolbar)
                    .aria_label("File actions")
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(header))
                    .when_some(svg_mode, |header, current| {
                        [SvgMode::Preview, SvgMode::Code].into_iter().fold(
                            header,
                            |header, mode| {
                                let (id, label) = match mode {
                                    SvgMode::Preview => ("svg-preview", "Preview"),
                                    SvgMode::Code => ("svg-code", "Code"),
                                };
                                let hover = Hover::new(id, window, cx);
                                header.child(
                                    Button::new(id)
                                        .small()
                                        .cursor_pointer()
                                        .label(label)
                                        .selected(current == mode)
                                        .disabled(mode == SvgMode::Code && !ready)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.document.update(cx, |d, cx| {
                                                d.set_svg_mode(mode, window, cx)
                                            });
                                            if mode == SvgMode::Preview {
                                                this.image_focus.focus(window, cx);
                                            }
                                        }))
                                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                                )
                            },
                        )
                    })
                    .child(
                        Button::new("file-sources")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::ChevronDown)
                            .tooltip("Choose unstaged or staged changes")
                            .disabled(!matches!(
                                self.changes,
                                FileChanges::StagedAndUnstaged | FileChanges::Unknown
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_changes(true, window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &sources_hover, cx),
                    )
                    .child(
                        Button::new("file-changes")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::GitCompareArrows)
                            .tooltip("Show changes")
                            .disabled(self.changes == FileChanges::Clean)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_changes(false, window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &changes_hover, cx),
                    )
                    .when(!raster, |header| {
                        header.child(
                            Button::new("file-save")
                                .small()
                                .cursor_pointer()
                                .icon(Lucide::Save)
                                .tooltip("Save")
                                .loading(saving)
                                .disabled(!ready || !dirty || saving || conflict)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.document.update(cx, |d, cx| {
                                        d.save(None, window, cx);
                                    });
                                }))
                                .smooth_hover(ButtonLook::Ghost, &save_hover, cx),
                        )
                    }),
            );
        if let Some(error) = source_error {
            body = body.child(
                div()
                    .px_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(error),
            );
        }
        if dirty {
            body = body.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Unsaved buffer · Git diff shows the disk version"),
            );
        }
        if let Some(error) = error {
            body = body.child(div().p_2().text_color(cx.theme().danger).child(error));
        }
        if conflict {
            body = body.child(
                Button::new("file-conflict")
                    .cursor_pointer()
                    .label("File changed on disk · Resolve conflict")
                    .on_click(cx.listener(|this, _, window, cx| this.resolve_conflict(window, cx)))
                    .smooth_hover(ButtonLook::Outline, &conflict_hover, cx),
            );
        }
        if let Some(image) = image {
            let document = self.document.downgrade();
            return body.child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.image_focus.focus(window, cx)),
                    )
                    .when(svg_mode.is_some(), |view| {
                        view.child(
                            canvas(
                                move |bounds, window, cx| {
                                    let scale = window.scale_factor();
                                    window.defer(cx, move |_, cx| {
                                        if let Some(document) = document.upgrade() {
                                            document.update(cx, |d, cx| {
                                                d.resize_image(bounds.size, scale, cx)
                                            });
                                        }
                                    });
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                    })
                    .child(image),
            );
        }
        if loading && !ready {
            body = body.child(gpui_kit::component::spinner::Spinner::new());
        }
        let blame = blame::overlay(
            cx.weak_entity(),
            self.document.clone(),
            editor.clone(),
            self.blame_fade.clone(),
        );
        body.capture_action(suggestions::accept_on_tab(editor.clone()))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        Editor::new(&editor)
                            .readonly(!ready)
                            .bordered(false)
                            .size_full(),
                    )
                    .child(blame)
                    .children(self.render_blame_card(window, cx)),
            )
    }
}
