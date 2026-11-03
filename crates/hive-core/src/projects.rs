//! Projects as scanned from disk: a folder plus its git worktrees.

use crate::git;
use crate::host::Host;
use crate::state::{ProjectRef, WorktreeFilter};
use std::path::{Path, PathBuf};

/// Suffix of the sibling folder Hive creates worktrees in: `<parent>/<repo>-worktrees/<name>`.
const WORKTREES_DIR_SUFFIX: &str = "-worktrees";

/// A project folder and its git worktrees.
#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub host: Host,
    pub filter: WorktreeFilter,
    /// git worktrees, the main one first. Empty when the folder is not a git repo.
    pub worktrees: Vec<Worktree>,
}

/// What a worktree has checked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// A local branch, without the `refs/heads/` prefix.
    Branch(String),
    Detached,
}

/// One checkout of the repository (`git worktree list`).
#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: Head,
}

impl Worktree {
    /// The worktree's folder name.
    pub fn name(&self) -> String {
        file_name(&self.path)
    }

    /// The checked-out branch, `None` on a detached HEAD.
    pub fn branch(&self) -> Option<&str> {
        match &self.head {
            Head::Branch(branch) => Some(branch),
            Head::Detached => None,
        }
    }
}

impl Project {
    /// Where Hive creates this project's worktrees: a sibling of the repository root,
    /// `<repo>-worktrees`, so it works the same on every host and stays out of the repo itself.
    /// The root comes from git (the primary worktree), which resolves symlinks the same way
    /// as the paths it lists.
    pub fn worktrees_dir(&self) -> PathBuf {
        let root = self
            .worktrees
            .first()
            .map_or(self.path.as_path(), |primary| primary.path.as_path());
        root.with_file_name(format!("{}{WORKTREES_DIR_SUFFIX}", file_name(root)))
    }

    /// The main worktree (listed first by git). It cannot be removed.
    pub fn is_primary(&self, worktree: &Worktree) -> bool {
        self.worktrees
            .first()
            .is_some_and(|primary| primary.path == worktree.path)
    }

    /// Worktrees the sidebar shows, with their index into [`Project::worktrees`]. Like Orca,
    /// by default only the primary one and those in [`Project::worktrees_dir`]; worktrees made
    /// by other tools (e.g. Claude Code's `.claude/worktrees`) stay hidden until asked for.
    pub fn visible_worktrees(&self) -> impl Iterator<Item = (usize, &Worktree)> {
        let dir = self.worktrees_dir();
        self.worktrees
            .iter()
            .enumerate()
            .filter(move |(index, worktree)| match self.filter {
                WorktreeFilter::All => true,
                WorktreeFilter::Hive => *index == 0 || worktree.path.starts_with(&dir),
            })
    }

    /// How many worktrees [`Project::visible_worktrees`] leaves out.
    pub fn hidden_worktree_count(&self) -> usize {
        self.worktrees.len() - self.visible_worktrees().count()
    }
}

/// Blocking: runs `git` (possibly over ssh). Call from a background executor.
pub fn scan(project: &ProjectRef) -> Project {
    let path = &project.path;
    let worktrees = git::run(&project.host, path, &["worktree", "list", "--porcelain"])
        .inspect_err(|err| tracing::debug!(?path, %err, "not listing worktrees"))
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default();
    Project {
        name: file_name(path),
        path: path.clone(),
        host: project.host.clone(),
        filter: project.worktrees,
        worktrees,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Parses `git worktree list --porcelain`. Skips bare repositories and worktrees whose
/// folder is gone (`prunable`), since there is nothing to open in either.
fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter_map(|block| {
            let mut path = None;
            let mut head = Head::Detached;
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(PathBuf::from(p));
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    head = Head::Branch(b.to_string());
                } else if line == "bare" || line.starts_with("prunable") {
                    return None;
                }
            }
            Some(Worktree { path: path?, head })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a repo with Claude Code worktrees and one deleted worktree.
    const PORCELAIN: &str = "worktree /Users/me/projects/app
HEAD b246c77f63a0dde37c47128bc4592c585d116783
branch refs/heads/develop

worktree /private/tmp/app-deploy
HEAD e4393c8ba78a55618db8e6a333f006500421d2e2
branch refs/heads/codex/github-deploy
prunable gitdir file points to non-existent location

worktree /Users/me/projects/app/.claude/worktrees/elastic-jennings-17f280
HEAD d0b2919d3ef87216396b84a71006b208cfd620b1
detached

worktree /Users/me/projects/app-worktrees/lavender
HEAD 252b376cd79759512091d7876db28757cf9d7874
branch refs/heads/lavender
";

    fn project(filter: WorktreeFilter) -> Project {
        Project {
            name: "app".into(),
            path: PathBuf::from("/Users/me/projects/app"),
            host: Host::Local,
            filter,
            worktrees: parse_worktrees(PORCELAIN),
        }
    }

    #[test]
    fn parses_porcelain_skipping_prunable() {
        let worktrees = parse_worktrees(PORCELAIN);
        let heads: Vec<_> = worktrees.iter().map(|w| w.head.clone()).collect();
        assert_eq!(
            heads,
            [
                Head::Branch("develop".into()),
                Head::Detached,
                Head::Branch("lavender".into())
            ]
        );
        assert_eq!(worktrees[1].name(), "elastic-jennings-17f280");
    }

    #[test]
    fn worktrees_dir_is_a_sibling() {
        assert_eq!(
            project(WorktreeFilter::Hive).worktrees_dir(),
            Path::new("/Users/me/projects/app-worktrees")
        );
    }

    #[test]
    fn hides_worktrees_made_elsewhere() {
        let project = project(WorktreeFilter::Hive);
        let visible: Vec<_> = project
            .visible_worktrees()
            .map(|(index, _)| index)
            .collect();
        assert_eq!(visible, [0, 2]);
        assert_eq!(project.hidden_worktree_count(), 1);
    }

    #[test]
    fn shows_all_worktrees_when_asked() {
        assert_eq!(project(WorktreeFilter::All).visible_worktrees().count(), 3);
    }
}
