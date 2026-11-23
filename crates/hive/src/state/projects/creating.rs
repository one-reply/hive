//! The worktrees being created, and the switch that stops each one's `git worktree add`:
//! its checkout runs `post-checkout` and content filters, which may take minutes.

use hive_core::command::Cancellation;
use hive_core::host::Host;
use hive_core::worktree::WorktreeError;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Creations(Vec<Creation>);

struct Creation {
    host: Host,
    project: PathBuf,
    stop: Arc<Cancellation>,
}

impl Creations {
    /// Records a creation in `project` and returns the switch that stops it.
    pub(super) fn begin(&mut self, host: &Host, project: &Path) -> Arc<Cancellation> {
        let stop = Arc::new(Cancellation::default());
        self.0.push(Creation {
            host: host.clone(),
            project: project.to_owned(),
            stop: stop.clone(),
        });
        stop
    }

    /// Forgets the creation `stop` belongs to, once Git has returned.
    pub(super) fn finish(&mut self, stop: &Arc<Cancellation>) {
        self.0.retain(|creation| !Arc::ptr_eq(&creation.stop, stop));
    }

    /// How many worktrees `project` is creating.
    pub(super) fn count(&self, host: &Host, project: &Path) -> usize {
        self.of(host, project).count()
    }

    /// Ends Git for every worktree `project` is creating.
    pub(super) fn stop(&self, host: &Host, project: &Path) {
        for creation in self.of(host, project) {
            creation.stop.cancel();
        }
    }

    fn of<'a>(&'a self, host: &'a Host, project: &'a Path) -> impl Iterator<Item = &'a Creation> {
        self.0
            .iter()
            .filter(move |creation| creation.host == *host && creation.project == project)
    }
}

/// What the user is told when a worktree was not created. Git ended midway may have left the
/// folder, its registration or the branch behind; the sidebar shows what is there.
pub(super) fn failure(error: WorktreeError) -> String {
    match &error {
        WorktreeError::Git(git) if git.outcome_unknown() => format!(
            "Git was ended while creating the worktree, so it may be partly there. Check the sidebar and remove what is left before trying again.\n{error}"
        ),
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::git::GitError;

    #[test]
    fn stopping_a_project_ends_only_its_own_creations() {
        let remote = Host::Ssh {
            target: "devbox".into(),
        };
        let mut creations = Creations::default();
        let first = creations.begin(&Host::Local, Path::new("/api"));
        let second = creations.begin(&Host::Local, Path::new("/api"));
        let other = creations.begin(&Host::Local, Path::new("/web"));
        let elsewhere = creations.begin(&remote, Path::new("/api"));
        assert_eq!(creations.count(&Host::Local, Path::new("/api")), 2);

        creations.stop(&Host::Local, Path::new("/api"));
        assert!(first.is_cancelled() && second.is_cancelled());
        assert!(!other.is_cancelled() && !elsewhere.is_cancelled());

        // A stopped creation is still counted until Git has returned.
        assert_eq!(creations.count(&Host::Local, Path::new("/api")), 2);
        creations.finish(&first);
        assert_eq!(creations.count(&Host::Local, Path::new("/api")), 1);
        creations.finish(&second);
        assert_eq!(creations.count(&Host::Local, Path::new("/api")), 0);
        assert_eq!(creations.count(&Host::Local, Path::new("/web")), 1);
        assert_eq!(creations.count(&remote, Path::new("/api")), 1);
    }

    fn interrupted(mutation: bool) -> WorktreeError {
        WorktreeError::Git(GitError::Interrupted {
            command: "worktree add".into(),
            host: Host::Local,
            cwd: "/api".into(),
            source: hive_core::command::Error::Cancelled,
            mutation,
            guidance: "guidance",
        })
    }

    #[test]
    fn only_a_creation_ended_midway_warns_of_what_it_may_have_left() {
        assert!(failure(interrupted(true)).contains("may be partly there"));
        // Stopped before Git started, or refused by Git: nothing is left behind.
        assert!(!failure(interrupted(false)).contains("partly"));
        let refused = WorktreeError::Git(GitError::Invalid("a branch named x exists".into()));
        assert_eq!(failure(refused), "a branch named x exists");
    }
}
