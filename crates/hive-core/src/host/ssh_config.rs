//! Concrete host aliases from the user's OpenSSH config, so Add Project can offer them.
//!
//! Only what a picker needs is read: `Host` aliases and their `HostName`/`User`. Wildcard
//! patterns are not destinations, so they are skipped. `ssh` itself stays the authority on
//! how a target connects; this list is a convenience and degrades to empty on any error.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// `Include` cycles or deep chains stop here instead of recursing forever.
const MAX_INCLUDE_DEPTH: usize = 8;

/// One concrete `Host` alias from `~/.ssh/config`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshHost {
    /// What the user passes to `ssh`, and what Hive stores as the target.
    pub alias: String,
    /// The `HostName` the alias resolves to, when the config names one.
    pub hostname: Option<String>,
    /// The `User` the alias logs in as, when the config names one.
    pub user: Option<String>,
}

impl SshHost {
    /// `user@hostname` as far as the config states it, for a secondary label.
    pub fn address(&self) -> Option<String> {
        let hostname = self.hostname.as_deref()?;
        Some(match &self.user {
            Some(user) => format!("{user}@{hostname}"),
            None => hostname.to_owned(),
        })
    }
}

/// Aliases from `<home>/.ssh/config` and its `Include`s, in file order, without duplicates.
pub fn hosts(home: &Path) -> Vec<SshHost> {
    let ssh_dir = home.join(".ssh");
    let mut hosts = Vec::new();
    read_file(&ssh_dir.join("config"), home, &ssh_dir, 0, &mut hosts);
    hosts
}

fn read_file(path: &Path, home: &Path, ssh_dir: &Path, depth: usize, hosts: &mut Vec<SshHost>) {
    if depth > MAX_INCLUDE_DEPTH {
        tracing::warn!(path = %path.display(), "SSH config Include chain too deep");
        return;
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if depth > 0 || error.kind() != std::io::ErrorKind::NotFound {
                tracing::debug!(path = %path.display(), %error, "SSH config not readable");
            }
            return;
        }
    };
    // Indices of the hosts the current `Host` block declared; `HostName`/`User` apply to them.
    let mut block: Vec<usize> = Vec::new();
    for line in text.lines() {
        let Some((keyword, value)) = directive(line) else {
            continue;
        };
        match keyword.to_ascii_lowercase().as_str() {
            "host" => {
                block.clear();
                for alias in value.split_whitespace().filter(|alias| is_concrete(alias)) {
                    match hosts.iter().position(|host| host.alias == alias) {
                        Some(index) => block.push(index),
                        None => {
                            block.push(hosts.len());
                            hosts.push(SshHost {
                                alias: alias.to_owned(),
                                hostname: None,
                                user: None,
                            });
                        }
                    }
                }
            }
            "match" => block.clear(),
            // OpenSSH keeps the first value it sees for a host, so later ones don't override.
            "hostname" => {
                for &index in &block {
                    hosts[index]
                        .hostname
                        .get_or_insert_with(|| value.to_owned());
                }
            }
            "user" => {
                for &index in &block {
                    hosts[index].user.get_or_insert_with(|| value.to_owned());
                }
            }
            "include" => {
                for pattern in value.split_whitespace() {
                    for included in expand_include(pattern, home, ssh_dir) {
                        read_file(&included, home, ssh_dir, depth + 1, hosts);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `Keyword value` or `Keyword=value`, without comments and surrounding quotes.
fn directive(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let (keyword, rest) = line.split_at(split);
    let value = rest
        .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
        .trim()
        .trim_matches('"');
    (!value.is_empty()).then_some((keyword, value))
}

fn is_concrete(alias: &str) -> bool {
    !alias.contains(['*', '?', '!'])
}

/// Relative `Include` paths are relative to `~/.ssh`; a `*` is honored in the file name,
/// which covers the common `config.d/*` layout without a glob dependency.
fn expand_include(pattern: &str, home: &Path, ssh_dir: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => ssh_dir.join(pattern),
    };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let Some((prefix, suffix)) = name.split_once('*') else {
        return vec![path];
    };
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.len() >= prefix.len() + suffix.len()
                            && name.starts_with(prefix)
                            && name.ends_with(suffix)
                    })
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("hive-ssh-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch dir");
        directory
    }

    fn home(name: &str, config: &str) -> PathBuf {
        let home = scratch(name);
        fs::create_dir(home.join(".ssh")).expect("ssh dir");
        fs::write(home.join(".ssh/config"), config).expect("config");
        home
    }

    fn aliases(hosts: &[SshHost]) -> Vec<&str> {
        hosts.iter().map(|host| host.alias.as_str()).collect()
    }

    #[test]
    fn reads_aliases_with_hostname_and_user_and_skips_patterns() {
        let home = home(
            "aliases",
            "# personal\nHost home devbox\n  HostName 10.0.0.2\n  User me\n\n\
             Host *.internal !bad\n  User ops\n\
             Host=build\n  HostName = \"build.example.com\"\n  HostName ignored\n\
             Match host foo\n  User nobody\n",
        );
        let hosts = hosts(&home);
        assert_eq!(aliases(&hosts), ["home", "devbox", "build"]);
        assert_eq!(hosts[0].address().as_deref(), Some("me@10.0.0.2"));
        assert_eq!(hosts[1].address().as_deref(), Some("me@10.0.0.2"));
        assert_eq!(hosts[2].address().as_deref(), Some("build.example.com"));
    }

    #[test]
    fn follows_includes_with_wildcards_once_and_stops_on_cycles() {
        let home = home(
            "includes",
            "Include config.d/*.conf ~/.ssh/loop\nHost first\n",
        );
        let ssh = home.join(".ssh");
        fs::create_dir(ssh.join("config.d")).expect("dir");
        fs::write(ssh.join("config.d/b.conf"), "Host beta\n").expect("b");
        fs::write(ssh.join("config.d/a.conf"), "Host alpha first\n").expect("a");
        fs::write(ssh.join("config.d/skip.txt"), "Host skipped\n").expect("skip");
        fs::write(ssh.join("loop"), "Include loop\nHost looped\n").expect("loop");
        let hosts = hosts(&home);
        assert_eq!(aliases(&hosts), ["alpha", "first", "beta", "looped"]);
    }

    #[test]
    fn missing_or_unreadable_config_gives_no_hosts() {
        let home = scratch("missing");
        assert!(hosts(&home).is_empty());
        fs::create_dir_all(home.join(".ssh/config")).expect("dir as config");
        assert!(hosts(&home).is_empty());
    }

    #[test]
    fn tolerates_malformed_lines() {
        let home = home(
            "malformed",
            "Host\nHostName orphan\n   \nUser\nHost ok\n\u{feff}junk\n",
        );
        let hosts = hosts(&home);
        assert_eq!(aliases(&hosts), ["ok"]);
        assert_eq!(hosts[0].hostname, None);
    }
}
