//! Projects as scanned from disk: a folder plus its git worktrees.

use crate::git;
use crate::host::Host;
use crate::identity::ProjectIdentity;
use crate::state::{ProjectRef, WorktreeFilter};
use std::path::{Path, PathBuf};

/// Suffix of the sibling folder Hive creates worktrees in: `<parent>/<repo>-worktrees/<name>`.
const WORKTREES_DIR_SUFFIX: &str = "-worktrees";

/// A project folder and its git worktrees.
#[derive(Clone, Debug)]
pub struct Project {
    /// The display name: the custom one from [`ProjectIdentity`], else the folder name.
    pub name: String,
    pub path: PathBuf,
    pub host: Host,
    pub filter: WorktreeFilter,
    pub identity: ProjectIdentity,
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

/// The checkout a selection points at: what Git, Files and terminals operate on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    /// The registered project folder.
    pub project: PathBuf,
    /// The folder to work in: the selected worktree, or the project folder itself.
    pub path: PathBuf,
    /// Where `path` lives.
    pub host: Host,
}

impl Target {
    /// The one rule for turning a sidebar selection into a target: the selected worktree,
    /// or, for a project without worktrees (a plain folder, or a repository whose scan
    /// failed), the project folder itself. A failed scan then surfaces as a Git error
    /// instead of an empty panel.
    pub fn of(project: &Project, worktree: Option<&Worktree>) -> Self {
        Self {
            project: project.path.clone(),
            path: worktree.map_or_else(|| project.path.clone(), |w| w.path.clone()),
            host: project.host.clone(),
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
        let mut name = root.file_name().unwrap_or_default().to_os_string();
        name.push(WORKTREES_DIR_SUFFIX);
        root.with_file_name(name)
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
pub fn scan(project: &ProjectRef) -> Result<Project, git::GitError> {
    let worktrees = match git::bytes(
        &project.host,
        &project.path,
        &["worktree", "list", "--porcelain", "-z"],
        None,
        16 * 1024 * 1024,
        false,
    ) {
        Ok(output) => parse_worktrees(&output)?,
        Err(error) if error.is_not_repository() => Vec::new(),
        Err(error) => return Err(error),
    };
    Ok(Project {
        worktrees,
        ..Project::from(project)
    })
}

impl From<&ProjectRef> for Project {
    fn from(project: &ProjectRef) -> Self {
        Self {
            name: project
                .identity
                .name()
                .map_or_else(|| file_name(&project.path), str::to_owned),
            path: project.path.clone(),
            host: project.host.clone(),
            filter: project.worktrees,
            identity: project.identity.clone(),
            worktrees: Vec::new(),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// NUL-delimited porcelain keeps newlines and non-UTF-8 Unix paths unambiguous.
fn parse_worktrees(porcelain: &[u8]) -> Result<Vec<Worktree>, git::GitError> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let mut worktrees = Vec::new();
    let mut path = None;
    let mut head = Head::Detached;
    let mut skip = false;
    for field in porcelain
        .split(|byte| *byte == 0)
        .chain(std::iter::once(&b""[..]))
    {
        if field.is_empty() {
            if let Some(path) = path.take()
                && !skip
            {
                worktrees.push(Worktree {
                    path,
                    head: head.clone(),
                });
            }
            head = Head::Detached;
            skip = false;
        } else if let Some(value) = field.strip_prefix(b"worktree ") {
            path = Some(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if let Some(value) = field.strip_prefix(b"branch refs/heads/") {
            head = Head::Branch(
                std::str::from_utf8(value)
                    .map_err(|_| {
                        git::GitError::Invalid(
                            "A branch name is not UTF-8; use the terminal for this repository."
                                .into(),
                        )
                    })?
                    .into(),
            );
        } else if field == b"bare" || field.starts_with(b"prunable") {
            skip = true;
        }
    }
    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_paths_keep_non_utf8_newlines_and_quotes() {
        use std::os::unix::ffi::OsStrExt;
        let worktrees =
            parse_worktrees(b"worktree /repo/\xff'\nname\0HEAD abc\0branch refs/heads/main\0\0")
                .unwrap();
        assert_eq!(
            worktrees[0].path.as_os_str().as_bytes(),
            b"/repo/\xff'\nname"
        );
        assert!(parse_worktrees(b"worktree /repo\0branch refs/heads/\xff\0").is_err());
    }

    #[test]
    fn target_is_the_worktree_or_else_the_project_folder() {
        let project = Project {
            name: "app".into(),
            path: PathBuf::from("/repo"),
            host: Host::Ssh {
                target: "box".into(),
            },
            filter: WorktreeFilter::Hive,
            identity: ProjectIdentity::default(),
            worktrees: vec![Worktree {
                path: PathBuf::from("/repo-worktrees/clover"),
                head: Head::Detached,
            }],
        };
        let worktree = Target::of(&project, project.worktrees.first());
        assert_eq!(worktree.path, Path::new("/repo-worktrees/clover"));
        assert_eq!(worktree.project, Path::new("/repo"));
        assert_eq!(worktree.host, project.host);
        let folder = Target::of(&project, None);
        assert_eq!(folder.path, Path::new("/repo"));
        assert_eq!(folder.project, Path::new("/repo"));
    }

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
            identity: ProjectIdentity::default(),
            worktrees: parse_worktrees(PORCELAIN.replace('\n', "\0").as_bytes()).unwrap(),
        }
    }

    #[test]
    fn parses_porcelain_skipping_prunable() {
        let worktrees = parse_worktrees(PORCELAIN.replace('\n', "\0").as_bytes()).unwrap();
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
