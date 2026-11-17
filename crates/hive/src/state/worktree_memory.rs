//! What a panel showed for a worktree, kept while another one is selected. Lives only for
//! the session: nothing here reaches `state.json`.
use hive_core::projects::Target;
use std::collections::HashMap;

pub(crate) struct WorktreeMemory<T> {
    parked: HashMap<Target, T>,
}

impl<T> Default for WorktreeMemory<T> {
    fn default() -> Self {
        Self {
            parked: HashMap::new(),
        }
    }
}

impl<T: Default + PartialEq> WorktreeMemory<T> {
    /// Parks `current` under `from` and returns what `to` had, or the default on a first
    /// visit. A default value is not kept, so only worktrees that differ from it take space.
    pub(crate) fn switch(&mut self, from: Option<&Target>, to: Option<&Target>, current: T) -> T {
        if let Some(from) = from {
            if current == T::default() {
                self.parked.remove(from);
            } else {
                self.parked.insert(from.clone(), current);
            }
        }
        to.and_then(|to| self.parked.remove(to)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::host::Host;

    fn target(path: &str) -> Target {
        Target {
            host: Host::Local,
            project: "/project".into(),
            path: path.into(),
        }
    }

    #[test]
    fn first_visit_starts_from_the_default() {
        let mut memory = WorktreeMemory::<String>::default();
        let shown = memory.switch(Some(&target("/a")), Some(&target("/b")), "filter".into());
        assert_eq!(shown, "");
    }

    #[test]
    fn returning_restores_what_each_worktree_showed() {
        let (a, b) = (target("/a"), target("/b"));
        let mut memory = WorktreeMemory::<String>::default();
        memory.switch(Some(&a), Some(&b), "in a".into());
        assert_eq!(memory.switch(Some(&b), Some(&a), "in b".into()), "in a");
        assert_eq!(memory.switch(Some(&a), Some(&b), "in a".into()), "in b");
    }

    #[test]
    fn worktrees_on_different_hosts_are_separate() {
        let local = target("/a");
        let remote = Target {
            host: Host::Ssh {
                target: "example".into(),
            },
            ..local.clone()
        };
        let mut memory = WorktreeMemory::<String>::default();
        memory.switch(Some(&local), Some(&remote), "local".into());
        assert_eq!(memory.switch(Some(&remote), None, "remote".into()), "");
        assert_eq!(memory.switch(None, Some(&local), String::new()), "local");
    }

    #[test]
    fn a_default_value_replaces_the_parked_one_without_taking_space() {
        let (a, b) = (target("/a"), target("/b"));
        let mut memory = WorktreeMemory::<String>::default();
        memory.switch(Some(&a), Some(&b), "in a".into());
        memory.switch(Some(&b), Some(&a), String::new());
        // Back in `a` the filter was cleared before leaving again.
        memory.switch(Some(&a), Some(&b), String::new());
        assert!(memory.parked.is_empty());
        assert_eq!(memory.switch(Some(&b), Some(&a), String::new()), "");
    }

    #[test]
    fn nothing_is_parked_without_a_selected_worktree() {
        let mut memory = WorktreeMemory::<String>::default();
        assert_eq!(memory.switch(None, None, "orphan".into()), "");
        assert!(memory.parked.is_empty());
    }
}
