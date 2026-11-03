use crate::host::Host;
use crate::state::ProjectRef;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub host: Host,
    /// git worktrees, the main one first. Empty when the folder is not a git repo.
    pub worktrees: Vec<Worktree>,
}

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Blocking: runs `git` (possibly over ssh). Call from a background executor.
pub fn scan(project: &ProjectRef) -> Project {
    let path = &project.path;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    let worktrees = project
        .host
        .command("git", &["worktree", "list", "--porcelain"], Some(path))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_worktrees(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    Project { name, path: path.clone(), host: project.host.clone(), worktrees }
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    porcelain
        .split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .map(|block| {
            let mut path = PathBuf::new();
            let mut branch = "detached".to_string();
            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = PathBuf::from(p);
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = b.to_string();
                }
            }
            Worktree { path, branch }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_porcelain() {
        let out = "worktree /a\nHEAD abc\nbranch refs/heads/main\n\nworktree /b\nHEAD def\ndetached\n";
        let wt = super::parse_worktrees(out);
        assert_eq!(wt.len(), 2);
        assert_eq!(wt[0].branch, "main");
        assert_eq!(wt[1].branch, "detached");
        assert_eq!(wt[1].path.to_str(), Some("/b"));
    }
}
