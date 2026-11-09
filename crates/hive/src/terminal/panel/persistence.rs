//! Convert pane entities to paths and back; the session owns all process work.
use super::*;
use hive_core::state::workspace::{Layout, SplitAxis, Tab, TerminalPane};

impl TerminalPanel {
    pub(crate) fn snapshot(&self, cx: &App) -> Tab {
        let mut index = 0;
        let mut active = 0;
        let panes = self.snapshot_panes(&self.panes, &mut index, &mut active, cx);
        Tab::Terminal { panes, active }
    }

    fn snapshot_panes(
        &self,
        pane: &PaneTree<Entity<TerminalView>>,
        index: &mut usize,
        active: &mut usize,
        cx: &App,
    ) -> Layout<TerminalPane> {
        match pane {
            PaneTree::Leaf(view) => {
                if *view == self.active {
                    *active = *index;
                }
                *index += 1;
                Layout::Leaf {
                    value: TerminalPane {
                        directory: view.read(cx).restore_directory(cx),
                        worktree: view.read(cx).cwd().map(Path::to_owned),
                        resume: view.read(cx).session().read(cx).resume.clone(),
                        name: view.read(cx).session().read(cx).name.clone(),
                    },
                }
            }
            PaneTree::Split { axis, children } => Layout::Split {
                axis: match axis {
                    Axis::Horizontal => SplitAxis::Horizontal,
                    Axis::Vertical => SplitAxis::Vertical,
                },
                sizes: self
                    .split_sizes
                    .get(&Self::split_id(pane, *axis, children.len()))
                    .cloned()
                    .unwrap_or_default(),
                children: children
                    .iter()
                    .map(|child| self.snapshot_panes(child, index, active, cx))
                    .collect(),
            },
        }
    }

    pub(crate) fn restore(
        host: &Host,
        panes: &Layout<TerminalPane>,
        active: usize,
        area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<Self>> {
        let panes =
            panes.filter_map(&mut |pane| pane.directory.is_absolute().then_some(pane.clone()))?;
        Some(cx.new(|cx| {
            let mut sizes = HashMap::new();
            let mut views = Vec::new();
            let tree = restore_panes(host, &panes, &mut sizes, &mut views, window, cx)
                .expect("filter_map removed empty and unknown nodes");
            let active = views.get(active).unwrap_or(&views[0]).clone();
            let mut panel = Self {
                host: host.clone(),
                panes: tree,
                active,
                pane_subscriptions: HashMap::new(),
                area,
                group: None,
                alone: false,
                split_sizes: sizes,
            };
            for view in views {
                panel.watch_pane(&view, window, cx);
            }
            panel
        }))
    }

    pub(super) fn split_id(
        pane: &PaneTree<Entity<TerminalView>>,
        axis: Axis,
        count: usize,
    ) -> SharedString {
        format!(
            "pane-split-{axis:?}-{}-{count}",
            pane.first_leaf().entity_id()
        )
        .into()
    }
}

fn restore_panes(
    host: &Host,
    layout: &Layout<TerminalPane>,
    sizes: &mut HashMap<SharedString, Vec<f32>>,
    views: &mut Vec<Entity<TerminalView>>,
    window: &mut Window,
    cx: &mut App,
) -> Option<PaneTree<Entity<TerminalView>>> {
    match layout {
        Layout::Leaf { value } => {
            let worktree = value.worktree.as_deref();
            let directory = Some(value.directory.as_path());
            let view = cx.new(|cx| {
                if !host.is_local() {
                    TerminalView::on_host(host, (worktree, directory), None, true, window, cx)
                } else {
                    match value.resume.clone() {
                        Some(resume) => {
                            TerminalView::resuming(worktree, directory, resume, window, cx)
                        }
                        None => TerminalView::in_directory(worktree, directory, window, cx),
                    }
                }
            });
            if value.name.is_some() {
                let session = view.read(cx).session().clone();
                session.update(cx, |session, cx| session.set_name(value.name.clone(), cx));
            }
            views.push(view.clone());
            Some(PaneTree::Leaf(view))
        }
        Layout::Split {
            axis,
            children,
            sizes: saved_sizes,
        } => {
            let axis = match axis {
                SplitAxis::Horizontal => Axis::Horizontal,
                SplitAxis::Vertical => Axis::Vertical,
            };
            let children: Vec<_> = children
                .iter()
                .filter_map(|child| restore_panes(host, child, sizes, views, window, cx))
                .collect();
            let count = children.len();
            let tree = PaneTree::Split { axis, children };
            sizes.insert(
                TerminalPanel::split_id(&tree, axis, count),
                saved_sizes.clone(),
            );
            Some(tree)
        }
        Layout::Unknown => None,
    }
}
