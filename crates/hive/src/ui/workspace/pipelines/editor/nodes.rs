use super::super::map::{BoxAt, units};
use super::*;
use crate::ui::{agent_logo, hover::SmoothHoverBg as _};

impl TemplateEditor {
    fn node(
        &self,
        item: Item,
        layout: &Layout,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (kind, ix, name, profile) = match item {
            Item::Planner => (
                "planner",
                0,
                &self.template.planner.name,
                &self.template.planner.profile,
            ),
            Item::Development => (
                "development",
                0,
                &self.template.development.name,
                &self.template.development.profile,
            ),
            Item::Lane(i) => (
                "lane",
                i,
                &self.template.lanes[i].name,
                &self.template.lanes[i].profile,
            ),
            Item::Review(i) => (
                "review",
                i,
                &self.template.reviews[i].name,
                &self.template.reviews[i].profile,
            ),
        };
        let drag = match item {
            Item::Lane(_) => self.template.lanes.len() > 1,
            Item::Review(1..) => self.template.reviews.len() > 2,
            _ => false,
        };
        let at = layout.placement(item);
        let role = self.role(item);
        let color = role.color(cx);
        let id = SharedString::from(format!("pipeline-{kind}-{ix}"));
        let hover = Hover::new(id.clone(), w, cx);
        let selected = item == self.selected;
        let view = cx.weak_entity();
        let owner = cx.entity().entity_id();
        let agent = if selected {
            self.inspector.agent
        } else {
            profile.agent
        };
        let model = if selected {
            self.inspector.model.read(cx).value().to_string()
        } else {
            profile.model.clone()
        };
        let title = if selected {
            self.inspector.name.read(cx).value().to_string()
        } else {
            name.clone()
        };
        let detail = if model.is_empty() {
            agent.name().to_owned()
        } else {
            format!("{} · {}", agent.name(), model)
        };
        let glyph = if item == Item::Development {
            Lucide::GitFork
        } else {
            role.icon()
        };
        let main = h_flex()
            .gap_1()
            .child(agent_logo::icon(agent, cx).size_3())
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            );
        let sub = if item == Item::Development {
            let n = self.template.lanes.len();
            subtle(
                format!("{n} parallel {}", if n == 1 { "lane" } else { "lanes" }),
                cx,
            )
        } else {
            main
        };
        let mut node = h_flex()
            .id(id.clone())
            .absolute()
            .left(units(at.x))
            .top(units(at.y))
            .w(units(at.w))
            .h(units(at.h))
            .px_2()
            .gap_2()
            .rounded_lg()
            .role(Role::Button)
            .aria_label(format!("Configure {title}"))
            .tab_index(0)
            .cursor_pointer()
            .border_1()
            .border_color(if selected { color } else { cx.theme().border })
            .when(selected, |e| e.border_2())
            .focus_visible(move |s| s.border_color(color))
            .child(
                div()
                    .size_6()
                    .flex_none()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color.alpha(0.14))
                    .child(Icon::new(glyph).size_4().text_color(color)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .truncate()
                            .child(title.clone()),
                    )
                    .child(sub),
            )
            .when(drag, |e| {
                e.child(
                    Icon::new(Lucide::GripVertical)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                )
            })
            .on_click(cx.listener(move |s, _, w, cx| s.select(item, w, cx)))
            .on_key_down(cx.listener(move |s, e: &KeyDownEvent, w, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    s.select(item, w, cx)
                }
            }));
        if drag {
            node = node
                .on_drag(
                    Dragged {
                        editor: owner,
                        item,
                        label: title,
                    },
                    |d, _, _, cx| cx.new(|_| d.clone()),
                )
                .drag_over::<Dragged>(move |s, d, _, cx| {
                    if d.editor == owner
                        && d.item != item
                        && std::mem::discriminant(&d.item) == std::mem::discriminant(&item)
                    {
                        s.border_color(cx.theme().drag_border)
                    } else {
                        s
                    }
                })
                .on_drop(move |d: &Dragged, w, cx| {
                    if d.editor == owner {
                        let _ = view.update(cx, |s, cx| s.move_to(d.item, item, w, cx));
                    }
                });
        }
        node.hover_bg(&hover, cx.theme().sidebar, cx.theme().accent)
            .into_any_element()
    }
    pub(super) fn canvas(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let l = Layout::new(self.template.lanes.len(), self.template.reviews.len());
        let mut graph = div()
            .relative()
            .flex_none()
            .w(units(l.width))
            .h(units(l.height))
            .child(map::wires(l.clone(), self.selected, &self.template, cx))
            .child(marker(
                l.start,
                Lucide::Play,
                "Start",
                cx.theme().muted_foreground,
                cx,
            ))
            .child(marker(
                l.approve,
                Lucide::UserRoundCheck,
                "Approve plan",
                RoleKind::Human.color(cx),
                cx,
            ))
            .child(marker(
                l.join,
                Lucide::GitMerge,
                "Join lanes",
                RoleKind::Build.color(cx),
                cx,
            ))
            .child(marker(
                l.end,
                Lucide::Flag,
                "Final acceptance",
                RoleKind::Human.color(cx),
                cx,
            ));
        for item in [Item::Planner, Item::Development]
            .into_iter()
            .chain((0..l.lanes.len()).map(Item::Lane))
            .chain((0..l.reviews.len()).map(Item::Review))
        {
            graph = graph.child(self.node(item, &l, w, cx));
        }
        v_flex()
            .h_full()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .overflow_hidden()
                    .child(map::grid(cx))
                    .child(
                        div()
                            .id("workflow-canvas")
                            .size_full()
                            .overflow_scroll()
                            .child(
                                v_flex()
                                    .min_w(units(l.width))
                                    .w_full()
                                    .items_center()
                                    .child(graph),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(Lucide::CornerUpLeft)
                            .size_3()
                            .text_color(cx.theme().warning),
                    )
                    .child(subtle("Findings return to development", cx)),
            )
    }
}
fn marker(at: BoxAt, icon: Lucide, label: &'static str, color: Hsla, cx: &App) -> Div {
    h_flex()
        .absolute()
        .left(units(at.x))
        .top(units(at.y))
        .w(units(at.w))
        .h(units(at.h))
        .justify_center()
        .gap_1()
        .bg(cx.theme().background)
        .rounded_full()
        .border_1()
        .border_color(color.alpha(0.42))
        .child(Icon::new(icon).size_3().text_color(color))
        .child(div().text_xs().child(label))
}
