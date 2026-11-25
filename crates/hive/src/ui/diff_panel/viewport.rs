//! The widget owns wrapping and row heights. Navigate its measured list directly.
use super::*;
use std::time::{Duration, Instant};

const TRAVEL: Duration = Duration::from_millis(220);

pub(super) struct Reveal {
    pub key: SharedString,
    start: Option<(Instant, Pixels)>,
    settled: bool,
}

impl Reveal {
    pub fn new(key: SharedString) -> Self {
        Self {
            key,
            start: None,
            settled: false,
        }
    }

    pub fn is_settled(&self) -> bool {
        self.settled
    }

    fn advance(&mut self, display: &DiffState, now: Instant, reduced: bool) -> bool {
        let Some(row) = display.header_row(&self.key) else {
            return false;
        };
        let list = display.list();
        if list.viewport_bounds().size.height <= px(0.) {
            return false;
        }
        if self.settled {
            return false;
        }
        let position = ListOffset {
            item_ix: row,
            offset_in_item: px(0.),
        };
        let origin = list.scroll_px_offset_for_scrollbar().y;
        let (start, from) = *self.start.get_or_insert((now, origin));
        list.scroll_to(position);
        let destination = list
            .scroll_px_offset_for_scrollbar()
            .y
            .max(-list.max_offset_for_scrollbar().y);
        let progress = now.saturating_duration_since(start).as_secs_f32() / TRAVEL.as_secs_f32();
        if reduced || progress >= 1. || (destination - origin).abs() < px(1.) {
            self.settled = true;
        } else {
            let offset = from + (destination - from) * ease_out_quint()(progress);
            list.set_offset_from_scrollbar(point(px(0.), offset));
        }
        true
    }
}

impl DiffPanel {
    /// Reloads the widget when the parsed patch changed.
    pub(super) fn sync_display(&mut self, cx: &mut Context<Self>) {
        let review = self.review.read(cx);
        if self.synced_generation == Some(review.generation) {
            return;
        }
        self.synced_generation = Some(review.generation);
        let files = review
            .order
            .iter()
            .map(|key| review.blocks[key].file.clone())
            .collect::<Vec<_>>();
        let settled = self
            .reveal
            .as_ref()
            .filter(|r| r.is_settled())
            .map(|r| r.key.clone());
        self.display.update(cx, |display, cx| {
            let top = display.list().logical_scroll_top();
            let anchor = display.file_at_row(top.item_ix).and_then(|file| {
                let row = display.header_row(file.path())?;
                Some((file.path().clone(), top.item_ix - row))
            });
            display.set_review_files(files, cx);
            if let Some(key) = settled
                && display.header_row(&key).is_some()
            {
                display.scroll_to_file(&key, cx);
            } else if let Some((key, within)) = anchor
                && let Some(header) = display.header_row(&key)
            {
                let next = display
                    .file_index(&key)
                    .and_then(|ix| display.files().get(ix + 1))
                    .and_then(|file| display.header_row(file.path()))
                    .unwrap_or(display.list().item_count());
                display.list().scroll_to(ListOffset {
                    item_ix: (header + within).min(next.saturating_sub(1)),
                    ..top
                });
            } else if top.item_ix > 0 {
                display.list().scroll_to(ListOffset {
                    item_ix: top
                        .item_ix
                        .min(display.list().item_count().saturating_sub(1)),
                    ..top
                });
            }
        });
    }

    pub(crate) fn reveal(&mut self, request: DiffRequest, cx: &mut Context<Self>) {
        self.visible.clear();
        let key = request.block_key();
        if self.display.read(cx).is_file_collapsed(&key) {
            self.display.update(cx, |display, cx| {
                display.set_file_collapsed(&key, false, cx)
            });
        }
        self.reveal = Some(Reveal::new(key));
        self.review
            .update(cx, |review, cx| review.select(request, cx));
        self.selection_changed(cx);
        cx.notify();
    }

    pub(super) fn track_viewport(&self, cx: &Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let input = panel.clone();
        canvas(
            move |_, window, cx| {
                // Read the list after layout. Paint this observer before the widget:
                // ScrollBounce consumes vertical wheel events in the capture phase.
                let panel = panel.clone();
                window.defer(cx, move |window, cx| {
                    let _ = panel.update(cx, |this, cx| this.viewport_changed(window, cx));
                });
            },
            move |bounds, _, window, _| {
                let input = input.clone();
                window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
                    if phase.capture() && bounds.contains(&event.position) {
                        let _ = input.update(cx, |this, _| this.reveal = None);
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn viewport_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let display = self.display.read(cx);
        let list = display.list();
        if list.is_scrollbar_dragging() {
            self.reveal = None;
        }
        if let Some(reveal) = &mut self.reveal
            && reveal.advance(display, cx.background_executor().now(), cx.reduce_motion())
        {
            let panel = cx.entity_id();
            window.on_next_frame(move |_, cx| cx.notify(panel));
        }
        let top = list.logical_scroll_top().item_ix;
        let Some(file) = display.file_at_row(top) else {
            return;
        };
        let first = file.path().clone();
        let files = display.files();
        let start = display.file_index(&first).unwrap_or(0);
        let bottom = list.viewport_bounds().bottom();
        let mut visible = vec![first.clone()];
        for file in files.iter().skip(start + 1) {
            let Some(bounds) = display
                .header_row(file.path())
                .and_then(|ix| list.bounds_for_item(ix))
            else {
                break;
            };
            if bounds.top() >= bottom {
                break;
            }
            visible.push(file.path().clone());
        }
        // The last short block can be completely visible even though it cannot
        // reach the top. Keep a deliberate sidebar choice until the next gesture.
        if self
            .reveal
            .as_ref()
            .is_some_and(|r| display.header_row(&r.key).is_none())
            && self.review.read(cx).listed
        {
            self.reveal = None;
        }
        let active = self.reveal.as_ref().map(|r| &r.key).unwrap_or(&first);
        let review = self.review.read(cx);
        let request = review
            .blocks
            .get(active)
            .map(|b| b.entry.request.clone())
            .filter(|r| *r != review.request);
        let moved = request.is_some();
        if visible != self.visible || moved {
            self.visible.clone_from(&visible);
            self.review.update(cx, |review, cx| {
                if let Some(request) = request {
                    review.request = request;
                }
                review.set_visible(visible, cx);
            });
            if moved {
                self.selection_changed(cx);
            }
        }
    }

    pub(super) fn selection_changed(&self, cx: &mut Context<Self>) {
        if let Some(area) = self.area.upgrade() {
            cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Reveal, TRAVEL};
    use gpui_kit as gpui;
    use gpui_kit::component::diff::DiffFile;
    use gpui_kit::component::diff::{Diff, DiffState};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, div, px,
    };

    struct Host(Entity<DiffState>);
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(640.)).h(px(240.)).child(Diff::new(&self.0))
        }
    }

    #[gpui::test]
    fn file_reveal_travels_through_the_list_and_a_new_target_replaces_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (host, cx) = cx.add_window_view(|_, cx| {
            let files = (0..24).map(|ix| DiffFile::notice(format!("section-{ix}"), "Content"));
            Host(cx.new(|cx| DiffState::new(files, cx)))
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let display = host.read_with(cx, |host, _| host.0.clone());
        cx.update(|_, cx| {
            let display = display.read(cx);
            let y = || -display.list().scroll_px_offset_for_scrollbar().y;
            let now = cx.background_executor().now();
            let mut down = Reveal::new("section-23".into());
            assert!(down.advance(display, now, false));
            assert_eq!(y(), px(0.));
            down.advance(display, now + TRAVEL / 4, false);
            let intermediate = y();
            assert!(
                intermediate > px(0.) && intermediate < display.list().max_offset_for_scrollbar().y
            );
            let mut up = Reveal::new("section-0".into());
            up.advance(display, now + TRAVEL / 4, false);
            up.advance(display, now + TRAVEL / 2, false);
            assert!(y() < intermediate && y() > px(0.));
            up.advance(display, now + TRAVEL * 2, false);
            assert_eq!(y(), px(0.));
            let mut instant = Reveal::new("section-12".into());
            instant.advance(display, now, true);
            assert!(instant.is_settled());
            assert_eq!(
                display.list().logical_scroll_top().item_ix,
                display.header_row("section-12").unwrap()
            );
        });
    }
}
