use super::OpenFile;

const LIMIT: usize = 100;

#[derive(Default)]
pub(crate) struct History {
    past: Vec<OpenFile>,
    future: Vec<OpenFile>,
}

impl History {
    pub(crate) fn record(&mut self, from: OpenFile) {
        if !self.past.last().is_some_and(|last| same(last, &from)) {
            push(&mut self.past, from);
        }
        self.future.clear();
    }

    pub(crate) fn step(&mut self, forward: bool, current: Option<OpenFile>) -> Option<OpenFile> {
        let (from, to) = if forward {
            (&mut self.future, &mut self.past)
        } else {
            (&mut self.past, &mut self.future)
        };
        let destination = from.pop()?;
        if let Some(current) = current {
            push(to, current);
        }
        Some(destination)
    }

    pub(crate) fn remove(&mut self, target: &hive_core::projects::Target, path: &std::path::Path) {
        for stack in [&mut self.past, &mut self.future] {
            stack.retain(|entry| entry.target != *target || !entry.path.starts_with(path));
        }
    }

    pub(crate) fn relocate(
        &mut self,
        target: &hive_core::projects::Target,
        from: &std::path::Path,
        to: &std::path::Path,
    ) {
        for entry in self.past.iter_mut().chain(&mut self.future) {
            if entry.target == *target
                && let Some(path) = hive_core::files::relocated_path(&entry.path, from, to)
            {
                entry.path = path;
            }
        }
    }
}

fn same(a: &OpenFile, b: &OpenFile) -> bool {
    a.target == b.target && a.path == b.path && a.position == b.position
}
fn push(stack: &mut Vec<OpenFile>, entry: OpenFile) {
    if stack.len() == LIMIT {
        stack.remove(0);
    }
    stack.push(entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn location(n: usize) -> OpenFile {
        OpenFile {
            target: hive_core::projects::Target {
                host: hive_core::host::Host::Local,
                project: "/project".into(),
                path: "/project".into(),
            },
            path: format!("{n}.rs").into(),
            position: Some(Default::default()),
            preview: false,
            placement: Default::default(),
        }
    }
    #[test]
    fn back_forward_branch_and_deleted_paths() {
        let mut history = History::default();
        history.record(location(1));
        history.record(location(1));
        assert_eq!(history.past.len(), 1);
        assert_eq!(
            history.step(false, Some(location(2))).unwrap().path,
            location(1).path
        );
        assert_eq!(
            history.step(true, Some(location(1))).unwrap().path,
            location(2).path
        );
        history.step(false, Some(location(2)));
        history.record(location(3));
        assert!(history.step(true, None).is_none());
        history.remove(&location(3).target, &location(3).path);
        assert!(history.step(false, None).is_none());
    }
    #[test]
    fn history_is_bounded() {
        let mut history = History::default();
        for n in 0..150 {
            history.record(location(n));
        }
        assert_eq!(history.past.len(), LIMIT);
        assert_eq!(history.past[0].path, location(50).path);
    }

    #[test]
    fn moves_remap_both_stacks_and_leave_other_hosts_and_worktrees_unchanged() {
        let mut source = location(1);
        source.path = "src/nested/a.rs".into();
        let mut other_root = source.clone();
        other_root.target.path = "/other".into();
        let mut other_host = source.clone();
        other_host.target.host = hive_core::host::Host::Ssh {
            target: "remote".into(),
        };
        let mut history = History {
            past: vec![other_root.clone(), source.clone()],
            future: vec![other_host.clone(), source.clone()],
        };
        history.relocate(
            &source.target,
            std::path::Path::new("src"),
            std::path::Path::new("lib"),
        );
        assert_eq!(
            history.step(false, None).unwrap().path,
            std::path::Path::new("lib/nested/a.rs")
        );
        assert_eq!(
            history.step(true, None).unwrap().path,
            std::path::Path::new("lib/nested/a.rs")
        );
        assert_eq!(history.step(false, None).unwrap().path, other_root.path);
        assert_eq!(history.step(true, None).unwrap().path, other_host.path);
        history.record(source.clone());
        history.relocate(
            &source.target,
            &source.path,
            std::path::Path::new("src/c.rs"),
        );
        assert_eq!(
            history.step(false, None).unwrap().path,
            std::path::Path::new("src/c.rs")
        );
    }
}
