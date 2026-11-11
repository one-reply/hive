//! Inline Git blame: who last changed the caret line, drawn after the end of that line, and a
//! card with the full commit when the pointer rests on it.
//!
//! gpui-component's editor has no end-of-line decorations, so a canvas painted after the editor
//! reads the line's bounds from the layout the editor has just painted. A div positioned during
//! layout would lag a frame behind scrolling.
use super::*;
use crate::ui::workspace::ShowGitPanel;
use fade::Fade;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use hive_core::git::blame::{Commit, Origin};
use hive_core::git::history::{age, calendar_date};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

mod fade;

/// Space between the end of the line's text and the label.
const LABEL_GAP: Pixels = px(32.);
/// Space between the branch icon and the text.
const ICON_GAP: Pixels = px(6.);
/// The icon's size relative to the editor font size.
const ICON_SCALE: f32 = 0.9;
/// How much of the theme's success hue tints the muted label: visible, yet quieter than code.
const LABEL_TINT: f32 = 0.35;
/// Pointer travel time from the label into the card before the card closes.
const HIDE_DELAY: Duration = Duration::from_millis(250);
const CARD_WIDTH: Pixels = px(420.);
const CARD_MESSAGE_MAX_HEIGHT: Pixels = px(240.);
const SHORT_OID: usize = 7;
const CARD_FADE_IN: Duration = Duration::from_millis(160);
const CARD_FADE_OUT: Duration = Duration::from_millis(120);
/// How far below its place the card starts while it fades in.
const CARD_RISE: Pixels = px(4.);

/// The hover card for the label under the pointer.
#[derive(Default)]
pub(super) struct BlameCard {
    shown: Option<Shown>,
    card_hovered: bool,
    /// Fading out; `shown` is dropped when the fade ends.
    closing: bool,
    /// Bumped on every fade so the card's animation restarts.
    generation: usize,
    /// The pointer's grace period after leaving, then the fade-out.
    _hide: Option<Task<()>>,
}

#[derive(Clone, PartialEq)]
struct Shown {
    commit: Commit,
    /// Window position of the label's bottom-left corner.
    anchor: Point<Pixels>,
}

/// The label for one line's origin: who and when; the subject waits for the hover card.
fn label(origin: &Origin) -> String {
    match origin {
        Origin::Uncommitted => "You, uncommitted changes".into(),
        Origin::Commit(commit) => format!("{}, {}", commit.author, age(&commit.date)),
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..SHORT_OID).unwrap_or(oid)
}

/// Where a label is painted: after the end of its line.
#[derive(Clone)]
pub(super) struct Placed {
    origin: Origin,
    line_end: Bounds<Pixels>,
}

/// The label's fade, shared between the panel and the canvas that paints it every frame.
pub(super) type LabelFade = Rc<RefCell<Fade<(usize, Origin), Placed>>>;

/// The canvas that paints the label; it must follow the editor so it paints after it.
pub(super) fn overlay(
    panel: WeakEntity<EditorPanel>,
    document: Entity<Document>,
    editor: Entity<EditorState>,
    fade: LabelFade,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| paint(bounds, &panel, &document, &editor, &fade, window, cx),
    )
    // Without insets an absolute element keeps its in-flow spot, below the editor.
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn paint(
    bounds: Bounds<Pixels>,
    panel: &WeakEntity<EditorPanel>,
    document: &Entity<Document>,
    editor: &Entity<EditorState>,
    fade: &LabelFade,
    window: &mut Window,
    cx: &mut App,
) {
    let next = caret_line(document, editor, window, cx)
        .map(|(line, origin, line_end)| ((line, origin.clone()), Placed { origin, line_end }));
    let frame = fade.borrow_mut().advance(next, Instant::now());
    if frame.is_animating() {
        window.request_animation_frame();
    }
    if let Some((place, opacity)) = frame.leaving {
        paint_label(&place, opacity, bounds, window, cx);
    }
    let Some((_, place, opacity)) = frame.current else {
        return;
    };
    let label_bounds = paint_label(&place, opacity, bounds, window, cx);
    let Origin::Commit(commit) = place.origin else {
        return;
    };
    let over_label = move |position: &Point<Pixels>| {
        bounds.contains(position) && label_bounds.contains(position)
    };
    // Capture runs before the editor's own handler, so the click opens the commit instead of
    // moving the caret to the end of the line.
    let (click_panel, click_commit) = (panel.clone(), commit.clone());
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture
            || event.button != MouseButton::Left
            || !over_label(&event.position)
        {
            return;
        }
        cx.stop_propagation();
        let commit = click_commit.clone();
        let _ = click_panel.update(cx, |panel, cx| panel.open_blame_commit(commit, window, cx));
    });
    let panel = panel.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let over = over_label(&event.position);
        let shown = over.then(|| Shown {
            commit: commit.clone(),
            anchor: label_bounds.bottom_left(),
        });
        let _ = panel.update(cx, |panel, cx| panel.point_at_blame(shown, cx));
    });
}

/// Paint the icon and text of one label, clipped to the editor; returns its bounds.
fn paint_label(
    place: &Placed,
    opacity: f32,
    clip: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Bounds<Pixels> {
    let text = SharedString::from(label(&place.origin));
    let theme = cx.theme();
    let color = theme
        .muted_foreground
        .blend(theme.success.opacity(LABEL_TINT))
        .opacity(opacity);
    let run = TextRun {
        len: text.len(),
        font: font(theme.mono_font_family.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let font_size = theme.mono_font_size;
    let shaped = window
        .text_system()
        .shape_line(text, font_size, &[run], None);
    let line_end = place.line_end;
    let line_height = line_end.size.height;
    let icon_size = font_size * ICON_SCALE;
    let icon = Bounds::new(
        point(
            line_end.right() + LABEL_GAP,
            line_end.top() + (line_height - icon_size) / 2.,
        ),
        size(icon_size, icon_size),
    );
    let position = point(icon.right() + ICON_GAP, line_end.top());
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        let painted = window
            .paint_svg(
                icon,
                Lucide::GitBranch.path(),
                None,
                TransformationMatrix::unit(),
                color,
                cx,
            )
            .and_then(|_| shaped.paint(position, line_height, TextAlign::Left, None, window, cx));
        if let Err(error) = painted {
            tracing::warn!(%error, "blame label paint failed");
        }
    });
    Bounds::from_corners(
        point(icon.left(), line_end.top()),
        point(position.x + shaped.width, line_end.bottom()),
    )
}

/// The caret line, its origin and the bounds of its end, when the label should show.
fn caret_line(
    document: &Entity<Document>,
    editor: &Entity<EditorState>,
    window: &Window,
    cx: &App,
) -> Option<(usize, Origin, Bounds<Pixels>)> {
    let document = document.read(cx);
    let editor = editor.read(cx);
    if !document.shows_blame(cx)
        || !editor.focus_handle(cx).is_focused(window)
        || !editor.selected_range().is_empty()
    {
        return None;
    }
    let line = editor.cursor_position().line as usize;
    let origin = document.blame().line(line)?.clone();
    let end = editor.text().line_end_offset(line);
    Some((line, origin, editor.range_to_bounds(&(end..end))?))
}

impl EditorPanel {
    /// The pointer is on a label (`Some`) or elsewhere; leaving lets it reach the card first.
    fn point_at_blame(&mut self, shown: Option<Shown>, cx: &mut Context<Self>) {
        let Some(shown) = shown else {
            self.leave_blame(cx);
            return;
        };
        let card = &mut self.blame_card;
        card._hide = None;
        let same = card.shown.as_ref() == Some(&shown);
        if same && !card.closing {
            return;
        }
        card.closing = false;
        card.generation += 1;
        if !same {
            let oid = shown.commit.oid.clone();
            card.shown = Some(shown);
            self.document
                .update(cx, |document, cx| document.load_commit_message(&oid, cx));
        }
        cx.notify();
    }

    fn leave_blame(&mut self, cx: &mut Context<Self>) {
        let card = &mut self.blame_card;
        if card.shown.is_none() || card.closing || card.card_hovered || card._hide.is_some() {
            return;
        }
        card._hide = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(HIDE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                this.blame_card._hide = None;
                if !this.blame_card.card_hovered {
                    this.close_blame_card(cx);
                }
            });
        }));
    }

    /// Show the commit in Git → History, like a click on it in the commit list.
    fn open_blame_commit(&mut self, commit: Commit, window: &mut Window, cx: &mut Context<Self>) {
        self.close_blame_card(cx);
        let git = AppState::get(cx).git.clone();
        // History lists the repository Git shows; a file of another worktree would open a
        // commit that list cannot load.
        if git.read(cx).target.as_ref() != Some(&self.document.read(cx).target) {
            window.push_notification(
                Notification::info("Select this file's worktree to open its commits."),
                cx,
            );
            return;
        }
        git.update(cx, |git, cx| git.select_commit(commit.into(), cx));
        window.dispatch_action(Box::new(ShowGitPanel), cx);
    }

    /// Fade the card out; also when the caret moves, since the card describes a line the label
    /// no longer shows.
    pub(super) fn close_blame_card(&mut self, cx: &mut Context<Self>) {
        let card = &mut self.blame_card;
        card.card_hovered = false;
        if card.shown.is_none() || card.closing {
            return;
        }
        card.closing = true;
        card.generation += 1;
        card._hide = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(CARD_FADE_OUT).await;
            let _ = this.update(cx, |this, cx| {
                let card = &mut this.blame_card;
                card._hide = None;
                card.closing = false;
                card.shown = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_blame_card(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let Shown { commit, anchor } = self.blame_card.shown.clone()?;
        let closing = self.blame_card.closing;
        let fade = if closing { CARD_FADE_OUT } else { CARD_FADE_IN };
        let message = self
            .document
            .read(cx)
            .blame()
            .message(&commit.oid)
            .cloned()
            .unwrap_or_else(|| commit.summary.clone().into());
        let copy_hover = Hover::new("blame-copy-sha", window, cx);
        let open_hover = Hover::new("blame-open-commit", window, cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let oid = commit.oid.clone();
        let open = commit.clone();
        let card = v_flex()
            .id("blame-card")
            .occlude()
            .w(CARD_WIDTH)
            .p_3()
            .gap_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.tokens.popover)
            .text_color(theme.popover_foreground)
            .text_sm()
            .shadow_md()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                let card = &mut this.blame_card;
                card.card_hovered = *hovered;
                if *hovered {
                    card._hide = None;
                    // Caught while fading out: bring it back.
                    if std::mem::take(&mut card.closing) {
                        card.generation += 1;
                        cx.notify();
                    }
                } else {
                    this.leave_blame(cx);
                }
            }))
            .child(
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(commit.author),
                    )
                    .when(!commit.email.is_empty(), |row| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(muted)
                                .child(format!("<{}>", commit.email)),
                        )
                    }),
            )
            .child(
                div()
                    .id("blame-message")
                    .max_h(CARD_MESSAGE_MAX_HEIGHT)
                    .overflow_y_scroll()
                    .child(message),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_color(muted)
                    .child(div().flex_1().child(calendar_date(&commit.date)))
                    .child(
                        Button::new("blame-open-commit")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::GitCommitHorizontal)
                            .label(short_oid(&commit.oid).to_owned())
                            .tooltip("Open commit in History")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_blame_commit(open.clone(), window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &open_hover, cx),
                    )
                    .child(
                        Button::new("blame-copy-sha")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::Copy)
                            .accessibility_label("Copy commit SHA")
                            .tooltip("Copy commit SHA")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(oid.clone()))
                            })
                            .smooth_hover(ButtonLook::Ghost, &copy_hover, cx),
                    ),
            );
        Some(
            deferred(
                anchored()
                    .position(anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(card.with_animation(
                        ("blame-card", self.blame_card.generation),
                        Animation::new(fade).with_easing(ease_out_quint()),
                        move |card, t| {
                            let shown = if closing { 1. - t } else { t };
                            card.opacity(shown).mt(CARD_RISE * (1. - shown))
                        },
                    )),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{label, short_oid};
    use hive_core::git::blame::{Commit, Origin};

    #[test]
    fn labels_name_author_and_age_only() {
        let commit = Commit {
            oid: "b4eb94f00ec0aaa2554e0307c2d8f85833458738".into(),
            author: "Ada Lovelace".into(),
            // Unparseable, so the age is the date itself and the test is clock-independent.
            date: "yesterday".into(),
            summary: "Add numbers".into(),
            ..Commit::default()
        };
        assert_eq!(label(&Origin::Commit(commit)), "Ada Lovelace, yesterday");
        assert_eq!(label(&Origin::Uncommitted), "You, uncommitted changes");
    }

    #[test]
    fn short_oid_tolerates_short_input() {
        assert_eq!(short_oid("b4eb94f00ec0aaa"), "b4eb94f");
        assert_eq!(short_oid("abc"), "abc");
    }
}
