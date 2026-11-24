use super::super::map::{BoxAt, units};
use super::drag::{Ghost, Load, react, reaction};
use super::*;
use crate::ui::{agent_logo, hover::SmoothHoverBg as _};
use gpui_kit::component::tooltip::Tooltip;
use hive_core::agent::AgentKind;

/// What a stage card shows. The canvas and the card under the pointer draw it the same way.
#[derive(Clone)]
pub(super) struct Face {
    pub(super) role: RoleKind,
    pub(super) glyph: Lucide,
    pub(super) title: SharedString,
    pub(super) detail: Detail,
}

/// The second line of a stage card.
#[derive(Clone)]
pub(super) enum Detail {
    /// The CLI that runs the stage, with its model when one is chosen.
    Agent(AgentKind, SharedString),
    /// Plain words, such as the number of lanes.
    Note(SharedString),
}

impl Face {
    /// The role tile and the two lines of text, for a row the caller sizes and frames.
    pub(super) fn content(&self, cx: &App) -> [AnyElement; 2] {
        let color = self.role.color(cx);
        let detail = match &self.detail {
            Detail::Agent(agent, line) => h_flex()
                .gap_1()
                .child(agent_logo::icon(*agent, cx).size_3())
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(line.clone()),
                ),
            Detail::Note(note) => subtle(note.clone(), cx),
        };
        [
            div()
                .size_6()
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(color.alpha(0.14))
                .child(Icon::new(self.glyph).size_4().text_color(color))
                .into_any_element(),
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_sm()
                        .truncate()
                        .child(self.title.clone()),
                )
                .child(detail)
                .into_any_element(),
        ]
    }
}

impl TemplateEditor {
    /// The card of a stage as it is now, the open form's unsaved fields included.
    fn face(&self, item: Item, cx: &App) -> Face {
        let (name, profile) = match item {
            Item::Planner => (&self.template.planner.name, &self.template.planner.profile),
            Item::Development => (
                &self.template.development.name,
                &self.template.development.profile,
            ),
            Item::Lane(i) => (
                &self.template.lanes[i].name,
                &self.template.lanes[i].profile,
            ),
            Item::Review(i) => (
                &self.template.reviews[i].name,
                &self.template.reviews[i].profile,
            ),
        };
        let selected = item == self.selected;
        let (title, agent, model) = if selected {
            (
                self.inspector.name.read(cx).value().to_string(),
                self.inspector.agent,
                self.inspector.model.read(cx).value().to_string(),
            )
        } else {
            (name.clone(), profile.agent, profile.model.clone())
        };
        let role = self.role(item);
        let detail = if item == Item::Development {
            let n = self.template.lanes.len();
            Detail::Note(format!("{n} parallel {}", if n == 1 { "lane" } else { "lanes" }).into())
        } else if model.is_empty() {
            Detail::Agent(agent, agent.name().into())
        } else {
            Detail::Agent(agent, format!("{} · {}", agent.name(), model).into())
        };
        Face {
            role,
            glyph: if item == Item::Development {
                Lucide::GitFork
            } else {
                role.icon()
            },
            title: title.into(),
            detail,
        }
    }

    /// Whether the stage has another place to stand among its own kind.
    fn is_movable(&self, item: Item) -> bool {
        match item {
            Item::Lane(_) => self.template.lanes.len() > 1,
            Item::Review(_) => self.template.reviews.len() > 1,
            Item::Planner | Item::Development => false,
        }
    }

    fn node(
        &self,
        item: Item,
        layout: &Layout,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (kind, ix) = match item {
            Item::Planner => ("planner", 0),
            Item::Development => ("development", 0),
            Item::Lane(i) => ("lane", i),
            Item::Review(i) => ("review", i),
        };
        let face = self.face(item, cx);
        let movable = self.is_movable(item);
        let conditional = matches!(item, Item::Review(i) if self.is_conditional(i, cx));
        let at = layout.placement(item);
        let color = face.role.color(cx);
        let id = SharedString::from(format!("pipeline-{kind}-{ix}"));
        let hover = Hover::new(id.clone(), w, cx);
        let selected = item == self.selected;
        let mut node = h_flex()
            .id(id)
            .absolute()
            .left(units(at.x))
            .top(units(at.y))
            .w(units(at.w))
            .h(units(at.h))
            .px_2()
            .gap_2()
            .rounded_lg()
            .role(Role::Button)
            .aria_label(card_label(&face.title, conditional))
            .tab_index(0)
            .cursor_pointer()
            .border_1()
            .border_color(if selected { color } else { cx.theme().border })
            .when(selected, |e| e.border_2())
            .focus_visible(move |s| s.border_color(color))
            .children(face.content(cx))
            .when(conditional, |e| {
                e.child(
                    Icon::new(Lucide::Funnel)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                )
            })
            .when(movable, |e| {
                e.child(
                    Icon::new(Lucide::GripVertical)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                )
            });
        node = on_activate(node, cx, move |s, w, cx| s.select(item, w, cx));
        if movable {
            let dragged = Dragged {
                editor: cx.entity_id(),
                load: Load::Move(item),
                ghost: Ghost::Stage(face, Some(at)),
            };
            node = node.on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()));
        }
        react(node, reaction(self.dragging, item), cx)
            .hover_bg(&hover, cx.theme().sidebar, cx.theme().accent)
            .into_any_element()
    }

    /// The plan gate. A click switches between the user approving the plan and the recipe
    /// approving it; with a shared contract it always asks, and the click changes nothing.
    fn plan_gate(&self, at: BoxAt, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let words = plan_gate_words(&self.template);
        let (icon, color) = if self.template.auto_approves_plan() {
            (Lucide::FastForward, cx.theme().muted_foreground)
        } else {
            (Lucide::UserRoundCheck, RoleKind::Human.color(cx))
        };
        let id = SharedString::from("pipeline-plan-gate");
        let hover = Hover::new(id.clone(), w, cx);
        let gate = marker(at, icon, words.label, color, cx)
            .id(id)
            .role(Role::Button)
            .aria_label(format!("Plan approval: {}", words.hint))
            .tab_index(0)
            .focus_visible(move |s| s.border_color(color))
            .when(!self.template.shared_contract, |gate| gate.cursor_pointer())
            .tooltip(move |window, cx| Tooltip::new(words.hint).build(window, cx));
        on_activate(gate, cx, |s, _, cx| {
            s.template.toggle_plan_approval();
            cx.notify();
        })
        .hover_bg(&hover, cx.theme().background, cx.theme().accent)
        .into_any_element()
    }

    pub(super) fn canvas(&self, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let l = self.layout();
        let mut graph = div()
            .relative()
            .flex_none()
            .w(units(l.width))
            .h(units(l.height))
            .on_drag_move(
                cx.listener(|s, event: &DragMoveEvent<Dragged>, _, cx| s.drag_moved(event, cx)),
            )
            .on_drop(cx.listener(|s, dragged: &Dragged, w, cx| s.dropped(dragged, w, cx)))
            .child(map::wires(l.clone(), self.selected, &self.template, cx))
            .child(marker(
                l.start,
                Lucide::Play,
                "Start",
                cx.theme().muted_foreground,
                cx,
            ))
            .child(self.plan_gate(l.approve, w, cx))
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
        for review in 0..l.reviews.len() {
            graph = graph.child(self.route_port(review, &l, w, cx));
        }
        graph = graph.children(self.insertion_line(&l, cx));
        v_flex()
            .h_full()
            .flex_1()
            .min_w_0()
            .child(self.palette(w, cx))
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
/// What a screen reader says of a stage card; the funnel icon of a review limited to some
/// paths is put into words.
fn card_label(title: &str, conditional: bool) -> String {
    if conditional {
        format!("Configure {title}, runs only when the change touches its paths")
    } else {
        format!("Configure {title}")
    }
}

/// How the plan gate reads on the canvas.
#[derive(Debug, PartialEq)]
struct PlanGateWords {
    label: &'static str,
    /// Who approves the plan now and what a click changes.
    hint: &'static str,
}

fn plan_gate_words(template: &Template) -> PlanGateWords {
    if template.shared_contract {
        PlanGateWords {
            label: "Approve plan",
            hint: "You approve the plan: its shared contract is yours to read.",
        }
    } else if template.auto_approves_plan() {
        PlanGateWords {
            label: "Auto-approve",
            hint: "The plan is approved automatically. Click to approve it yourself.",
        }
    } else {
        PlanGateWords {
            label: "Approve plan",
            hint: "You approve the plan. Click to have it approved automatically.",
        }
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

#[cfg(test)]
mod tests {
    use super::{Template, card_label, plan_gate_words};

    #[test]
    fn a_conditional_reviews_card_says_so_to_a_screen_reader() {
        assert_eq!(card_label("QA", false), "Configure QA");
        assert_eq!(
            card_label("Security", true),
            "Configure Security, runs only when the change touches its paths"
        );
    }

    #[test]
    fn the_plan_gate_says_who_approves_and_what_a_click_changes() {
        let mut recipe = Template::default();
        let asking = plan_gate_words(&recipe);
        assert_eq!(asking.label, "Approve plan");
        assert!(
            asking
                .hint
                .ends_with("Click to have it approved automatically.")
        );

        recipe.toggle_plan_approval();
        let automatic = plan_gate_words(&recipe);
        assert_eq!(automatic.label, "Auto-approve");
        assert!(automatic.hint.ends_with("Click to approve it yourself."));

        // With a shared contract the gate always asks and offers nothing to click.
        recipe.set_shared_contract(true);
        let contract = plan_gate_words(&recipe);
        assert_eq!(contract.label, "Approve plan");
        assert!(!contract.hint.contains("Click"));
    }
}
