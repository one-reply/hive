//! Older layouts could hold one diff tab per file, including across splits.
use super::{Layout, ProjectWorkspace, Tab, Tabs};
use std::{collections::HashMap, path::Path};

impl ProjectWorkspace {
    pub(super) fn merge_review_tabs(&mut self) {
        let Some(layout) = &self.layout else { return };
        let mut kept = HashMap::new();
        let mut index = 0;
        if !choose(layout, self.focused, &mut index, &mut kept) {
            return;
        }
        let mut old = 0;
        let mut new = 0;
        let mut focused = 0;
        let layout = layout.filter_map(&mut |tabs| {
            let mut items = Vec::new();
            let mut active = 0;
            for (i, tab) in tabs.items.iter().enumerate() {
                let keep = match tab {
                    Tab::Diff { worktree, .. } => kept
                        .get(worktree.as_path())
                        .is_some_and(|&(_, ix)| ix == old),
                    _ => true,
                };
                if keep {
                    if old <= self.focused {
                        focused = new;
                    }
                    if i <= tabs.active {
                        active = items.len();
                    }
                    items.push(tab.clone());
                    new += 1;
                }
                old += 1;
            }
            (!items.is_empty()).then_some(Tabs { items, active })
        });
        self.layout = layout;
        self.focused = focused;
    }
}

fn choose<'a>(
    layout: &'a Layout<Tabs>,
    focused: usize,
    index: &mut usize,
    kept: &mut HashMap<&'a Path, (u8, usize)>,
) -> bool {
    let mut duplicates = false;
    match layout {
        Layout::Leaf { value } => {
            for (i, tab) in value.items.iter().enumerate() {
                if let Tab::Diff { worktree, .. } = tab {
                    let priority = if *index == focused {
                        2
                    } else if i == value.active {
                        1
                    } else {
                        0
                    };
                    let previous = kept.entry(worktree).or_insert((priority, *index));
                    duplicates |= previous.1 != *index;
                    if priority > previous.0 {
                        *previous = (priority, *index);
                    }
                }
                *index += 1;
            }
        }
        Layout::Split { children, .. } => {
            for child in children {
                duplicates |= choose(child, focused, index, kept);
            }
        }
        Layout::Unknown => (),
    }
    duplicates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_diff_tabs_across_splits_restore_the_focused_revision_and_other_worktrees() {
        let saved = r#"{"project":"/repo","focused":2,"layout":{"kind":"split","axis":"horizontal","children":[
            {"kind":"leaf","value":{"items":[{"kind":"diff","worktree":"/repo","path":"first","source":"WorkingTree"}]}},
            {"kind":"leaf","value":{"active":1,"items":[
                {"kind":"file","worktree":"/repo","path":"editor"},
                {"kind":"diff","worktree":"/repo","path":"second","source":{"Commit":"abc"}},
                {"kind":"diff","worktree":"/other","path":"first","source":"Staged"}]}}]}}"#;
        let mut workspace: ProjectWorkspace = serde_json::from_str(saved).unwrap();
        workspace.merge_review_tabs();
        let Some(Layout::Leaf { value }) = &workspace.layout else {
            panic!("empty split should collapse")
        };
        assert_eq!(
            (workspace.focused, value.active, value.items.len()),
            (1, 1, 3)
        );
        assert!(
            matches!(&value.items[1],Tab::Diff{path,source:crate::git::diff::DiffSource::Commit(oid),..} if path=="second" && oid=="abc")
        );
        let restored = workspace.clone();
        workspace.merge_review_tabs();
        assert_eq!(workspace, restored);
    }
}
