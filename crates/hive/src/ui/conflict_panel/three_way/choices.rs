//! Per-conflict choices and the two gutter buttons that add or remove one side.
use super::*;

impl ConflictPanel {
    pub(super) fn render_conflict_header(
        &self,
        region: &ConflictRegion,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ix = region.index;
        let resolution = self.resolutions.get(ix).and_then(Option::as_ref);
        let editing_this = self.editing.as_ref().is_some_and(|e| e.conflict == ix);
        let has_base = region.base.is_some();
        let base_shown = self.shown_bases.contains(&ix);

        let left_taken = matches!(
            resolution,
            Some(
                ConflictResolution::Current
                    | ConflictResolution::BothCurrentFirst
                    | ConflictResolution::BothIncomingFirst
            )
        );
        let right_taken = matches!(
            resolution,
            Some(
                ConflictResolution::Incoming
                    | ConflictResolution::BothCurrentFirst
                    | ConflictResolution::BothIncomingFirst
            )
        );

        let [
            gutter_left_hover,
            gutter_right_hover,
            choose_both_hover,
            edit_hover,
            swap_hover,
            undo_hover,
            base_hover,
        ] = [
            "gutter-left",
            "gutter-right",
            "choose-both",
            "choose-edit",
            "choose-swap",
            "choose-undo",
            "choose-base",
        ]
        .map(|id| Hover::new((id, ix), window, cx));
        let theme = cx.theme();

        let cur_label = if region.current_label.is_empty() || region.current_label == "HEAD" {
            names.current_label.clone()
        } else {
            region.current_label.clone()
        };
        let inc_label = if region.incoming_label.is_empty() {
            names.incoming_label.clone()
        } else {
            region.incoming_label.clone()
        };

        let left_header = h_flex()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .gap_1p5()
            .items_center()
            .bg(theme.success.opacity(CONFLICT_HEADER_OPACITY))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(format!("Current · {cur_label}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{}L", region.current.len())),
            );

        let right_header = h_flex()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .gap_1p5()
            .items_center()
            .bg(theme.info.opacity(CONFLICT_HEADER_OPACITY))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{}L", region.incoming.len())),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(format!("Incoming · {inc_label}")),
            );

        let left_gutter_btn = div()
            .w(GUTTER_WIDTH)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_x_1()
            .border_color(theme.border)
            .bg(if left_taken {
                theme.success.opacity(0.2)
            } else {
                theme.muted.opacity(0.25)
            })
            .child(
                Button::new(("gutter-left", ix))
                    .cursor_pointer()
                    .xsmall()
                    .disabled(editing_this)
                    .label(if left_taken { "✓" } else { "»" })
                    .tooltip(if left_taken {
                        SharedString::from("Undo Current")
                    } else {
                        SharedString::from(format!("Accept Current ({cur_label})"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = toggle_side(
                            this.resolutions.get(ix).and_then(Option::as_ref),
                            Side::Ours,
                        );
                        this.resolve_conflict(ix, next, cx);
                    }))
                    .smooth_hover(ButtonLook::Ghost, &gutter_left_hover, cx),
            );

        let right_gutter_btn = div()
            .w(GUTTER_WIDTH)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .border_x_1()
            .border_color(theme.border)
            .bg(if right_taken {
                theme.info.opacity(0.2)
            } else {
                theme.muted.opacity(0.25)
            })
            .child(
                Button::new(("gutter-right", ix))
                    .cursor_pointer()
                    .xsmall()
                    .disabled(editing_this)
                    .label(if right_taken { "✓" } else { "«" })
                    .tooltip(if right_taken {
                        SharedString::from("Undo Incoming")
                    } else {
                        SharedString::from(format!("Accept Incoming ({inc_label})"))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let next = toggle_side(
                            this.resolutions.get(ix).and_then(Option::as_ref),
                            Side::Theirs,
                        );
                        this.resolve_conflict(ix, next, cx);
                    }))
                    .smooth_hover(ButtonLook::Ghost, &gutter_right_hover, cx),
            );

        let center_bg = match resolution {
            None => theme.warning.opacity(UNDECIDED_OPACITY),
            Some(ConflictResolution::Current) => theme.success.opacity(CONFLICT_HEADER_OPACITY),
            Some(ConflictResolution::Incoming) => theme.info.opacity(CONFLICT_HEADER_OPACITY),
            Some(_) => theme.primary.opacity(0.14),
        };

        let center_header = h_flex()
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .gap_1()
            .flex_wrap()
            .items_center()
            .bg(center_bg)
            .child(
                div()
                    .min_w(px(80.))
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(match resolution {
                        None => theme.warning,
                        Some(_) => theme.foreground,
                    })
                    .child(match resolution {
                        None => format!("Conflict #{}", ix + 1),
                        Some(res) => format!("✓ {}", res.label()),
                    }),
            )
            .when(has_base, |bar| {
                bar.child(
                    small_action(
                        ("choose-base", ix),
                        if base_shown { "Hide Base" } else { "Base" },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_base(ix, cx)))
                    .smooth_hover(ButtonLook::Ghost, &base_hover, cx),
                )
            })
            .when(resolution.is_none() && !editing_this, |bar| {
                bar.child(
                    small_action(("choose-both", ix), "Both")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resolve_conflict(
                                ix,
                                Some(ConflictResolution::BothCurrentFirst),
                                cx,
                            );
                        }))
                        .smooth_hover(ButtonLook::Ghost, &choose_both_hover, cx),
                )
            })
            .when(
                matches!(
                    resolution,
                    Some(
                        ConflictResolution::BothCurrentFirst
                            | ConflictResolution::BothIncomingFirst
                    )
                ) && !editing_this,
                |bar| {
                    bar.child(
                        small_action(("choose-swap", ix), "Swap Order")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = match this.resolutions.get(ix) {
                                    Some(Some(ConflictResolution::BothCurrentFirst)) => {
                                        Some(ConflictResolution::BothIncomingFirst)
                                    }
                                    _ => Some(ConflictResolution::BothCurrentFirst),
                                };
                                this.resolve_conflict(ix, next, cx);
                            }))
                            .smooth_hover(ButtonLook::Ghost, &swap_hover, cx),
                    )
                },
            )
            .when(self.editing.is_none(), |bar| {
                bar.child(
                    small_action(("choose-edit", ix), "Edit")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_editing(ix, window, cx);
                        }))
                        .smooth_hover(ButtonLook::Ghost, &edit_hover, cx),
                )
            })
            .when(resolution.is_some() && !editing_this, |bar| {
                bar.child(
                    small_action(("choose-undo", ix), "Undo")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.resolve_conflict(ix, None, cx);
                        }))
                        .smooth_hover(ButtonLook::Ghost, &undo_hover, cx),
                )
            });

        h_flex()
            .w_full()
            .min_w_0()
            .items_stretch()
            .border_t_1()
            .border_color(theme.border)
            .child(left_header)
            .child(left_gutter_btn)
            .child(center_header)
            .child(right_gutter_btn)
            .child(right_header)
            .into_any_element()
    }
}

/// A second check adds the other side in click order. Unchecking keeps the opposite side.
fn toggle_side(resolution: Option<&ConflictResolution>, side: Side) -> Option<ConflictResolution> {
    use ConflictResolution::*;
    match (resolution, side) {
        (Some(Current), Side::Ours) | (Some(Incoming), Side::Theirs) => None,
        (Some(Current), Side::Theirs) => Some(BothCurrentFirst),
        (Some(Incoming), Side::Ours) => Some(BothIncomingFirst),
        (Some(BothCurrentFirst | BothIncomingFirst), Side::Ours) => Some(Incoming),
        (Some(BothCurrentFirst | BothIncomingFirst), Side::Theirs) => Some(Current),
        (_, Side::Ours) => Some(Current),
        (_, Side::Theirs) => Some(Incoming),
    }
}

#[cfg(test)]
mod tests {
    use super::{ConflictResolution::*, Side, toggle_side};

    #[test]
    fn gutter_checks_add_in_click_order_and_remove_only_the_checked_side() {
        for (first, second, one, both, other) in [
            (
                Side::Ours,
                Side::Theirs,
                Current,
                BothCurrentFirst,
                Incoming,
            ),
            (
                Side::Theirs,
                Side::Ours,
                Incoming,
                BothIncomingFirst,
                Current,
            ),
        ] {
            assert_eq!(toggle_side(None, first), Some(one.clone()));
            assert_eq!(toggle_side(Some(&one), first), None);
            assert_eq!(toggle_side(Some(&one), second), Some(both.clone()));
            assert_eq!(toggle_side(Some(&both), first), Some(other));
            assert_eq!(toggle_side(Some(&both), second), Some(one));
        }
    }
}
