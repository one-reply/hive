//! Where Claude Code keeps its files. Hive reads its sessions, its transcripts and its model
//! cache, so every reader follows `CLAUDE_CONFIG_DIR` from the same environment: the one the
//! CLI gets in a Hive terminal.
use crate::host::environment;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Claude Code's files on this Mac.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Locations {
    /// Sessions and transcripts: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub(crate) directory: PathBuf,
    /// The global config with the model cache: inside `$CLAUDE_CONFIG_DIR`, else in home.
    pub(crate) global_config: PathBuf,
}

/// `None` without `CLAUDE_CONFIG_DIR` and without a home folder.
pub(crate) fn locations() -> Option<Locations> {
    locate(
        environment::var("CLAUDE_CONFIG_DIR"),
        crate::state::home_dir().as_deref(),
    )
}

/// A relative `configured` folder would depend on the directory the CLI started in, which is
/// not this one, so it counts as unset.
fn locate(configured: Option<OsString>, home: Option<&Path>) -> Option<Locations> {
    match configured
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        Some(directory) => Some(Locations {
            global_config: directory.join(".claude.json"),
            directory,
        }),
        None => home.map(|home| Locations {
            directory: home.join(".claude"),
            global_config: home.join(".claude.json"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_folder_holds_both_the_sessions_and_the_global_config() {
        let home = Path::new("/Users/me");
        assert_eq!(
            locate(Some("/srv/claude".into()), Some(home)),
            Some(Locations {
                directory: "/srv/claude".into(),
                global_config: "/srv/claude/.claude.json".into(),
            })
        );
        let default = Some(Locations {
            directory: "/Users/me/.claude".into(),
            global_config: "/Users/me/.claude.json".into(),
        });
        assert_eq!(locate(None, Some(home)), default);
        assert_eq!(locate(Some("claude".into()), Some(home)), default);
        assert_eq!(locate(Some("".into()), Some(home)), default);
        assert_eq!(locate(None, None), None);
    }
}
