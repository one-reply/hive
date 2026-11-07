//! Release highlights bundled with the installed version, available offline.

/// Version whose release notes are included in this build.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Full release notes for this exact version, rather than the newest available release.
pub const URL: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/releases/tag/v",
    env!("CARGO_PKG_VERSION")
);

const NOTES: &str = include_str!(concat!(
    "../../../docs/releases/v",
    env!("CARGO_PKG_VERSION"),
    ".md"
));

/// The Markdown highlights, excluding installation and maintainer instructions.
pub fn highlights() -> &'static str {
    highlights_in(NOTES)
}

fn highlights_in(notes: &str) -> &str {
    let Some((_, body)) = notes.split_once("\n## What's new\n") else {
        return "Read the full release notes for details about this version.";
    };
    body.split_once("\n## ")
        .map_or(body, |(highlights, _)| highlights)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_release_has_highlights_for_its_version() {
        assert!(NOTES.starts_with(&format!("# Hive {VERSION} ")));
        assert!(highlights().starts_with("- "));
        assert!(!highlights().contains("## Installation"));
        assert!(!highlights().contains("## Verification"));
        assert!(URL.ends_with(&format!("/releases/tag/v{VERSION}")));
    }

    #[test]
    fn highlights_stop_at_the_next_section_or_end_of_file() {
        assert_eq!(
            highlights_in(
                "# Release\n\n## What's new\n\n- Fixed a bug.\n\n## Installation\nSkip this."
            ),
            "- Fixed a bug."
        );
        assert_eq!(
            highlights_in("\n## What's new\n- Fixed a bug.\n"),
            "- Fixed a bug."
        );
    }

    #[test]
    fn missing_section_has_a_useful_fallback() {
        for notes in ["", "# Release\n## Installation\nInstructions"] {
            assert_eq!(
                highlights_in(notes),
                "Read the full release notes for details about this version."
            );
        }
    }
}
