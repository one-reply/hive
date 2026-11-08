//! Identity of the installed release and where its notes are published.

/// Version of this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full release notes for this exact version, rather than the newest available release.
pub const URL: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/releases/tag/v",
    env!("CARGO_PKG_VERSION")
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_points_at_the_installed_version_tag() {
        assert!(URL.ends_with(&format!("/releases/tag/v{VERSION}")));
    }
}
