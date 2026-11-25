//! Human-readable source locations printed by compilers and pasted into file search.
use std::path::{Component, Path, PathBuf};

/// A zero-based line and character column, parsed from a one-based user location.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    /// Zero-based logical line.
    pub line: u32,
    /// Zero-based character within the line.
    pub column: u32,
}

impl Location {
    /// Parse `line` or `line:column`.
    pub fn parse(input: &str) -> Option<Self> {
        let (line, column) = input.trim().split_once(':').unwrap_or((input.trim(), "1"));
        Some(Self {
            line: index(line)?,
            column: index(column)?,
        })
    }

    /// Split a path and its numeric suffix. Colons elsewhere belong to the path.
    pub fn split(input: &str) -> Option<(&str, Self)> {
        let (prefix, last) = input.rsplit_once(':')?;
        let number = index(last)?;
        let (path, location) = match prefix
            .rsplit_once(':')
            .and_then(|(p, n)| Some((p, index(n)?)))
        {
            Some((path, line)) => (
                path,
                Self {
                    line,
                    column: number,
                },
            ),
            None => (
                prefix,
                Self {
                    line: number,
                    column: 0,
                },
            ),
        };
        (!path.is_empty()).then_some((path, location))
    }
}

fn index(n: &str) -> Option<u32> {
    n.bytes().all(|b| b.is_ascii_digit()).then_some(())?;
    n.parse::<u32>().ok()?.checked_sub(1)
}

/// Returns an editable worktree-relative file name without performing I/O.
pub fn relative<'a>(root: &Path, absolute: &'a Path) -> Option<&'a Path> {
    let path = absolute.strip_prefix(root).ok()?;
    (absolute.is_absolute() && valid_path(path)).then_some(path)
}

/// Select possible worktree-relative files named by a terminal location. Relative paths
/// can refer to output produced before a shell changed directories.
pub fn candidates(paths: &[PathBuf], root: &Path, name: &Path) -> Vec<PathBuf> {
    if name.is_absolute() {
        return relative(root, name)
            .map(Path::to_owned)
            .into_iter()
            .collect();
    }
    let suffix: PathBuf = name
        .components()
        .skip_while(|part| matches!(part, Component::CurDir | Component::ParentDir))
        .collect();
    if !valid_path(&suffix) {
        return Vec::new();
    }
    paths
        .iter()
        .filter(|p| valid_path(p) && p.ends_with(&suffix))
        .cloned()
        .collect()
}

fn valid_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && super::stays_inside(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_coordinates_from_colons_and_unicode_in_a_filename() {
        let point = Location { line: 9, column: 3 };
        assert_eq!(Location::parse("10:4"), Some(point));
        assert_eq!(
            Location::split("dir:案/a.ts:10:4"),
            Some(("dir:案/a.ts", point))
        );
        assert_eq!(
            Location::split("a.rs:4"),
            Some(("a.rs", Location { line: 3, column: 0 }))
        );
        for text in ["0", "3:0", "3:4:5", "1:-2", "1:+4", "4294967296"] {
            assert!(Location::parse(text).is_none());
        }
    }

    #[test]
    fn old_terminal_output_can_refer_to_either_directory() {
        let files = ["src/lib.rs", "crate/src/lib.rs", "x/not-src/lib.rs"].map(PathBuf::from);
        let root = Path::new("/workspace/a");
        assert_eq!(
            candidates(&files, root, Path::new("./src/lib.rs")),
            files[..2]
        );
        assert_eq!(
            candidates(&files, root, Path::new("/workspace/a/src/lib.rs")),
            files[..1]
        );
        for path in [
            "/workspace/b/src/lib.rs",
            "/workspace/a/../b/lib.rs",
            ".git/config",
            "a/../../etc/passwd",
        ] {
            assert!(candidates(&files, root, Path::new(path)).is_empty());
        }
    }
}
