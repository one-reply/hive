//! What the lens after one dependency says, piece by piece, what each piece explains under
//! the pointer, and where each piece goes.
use super::*;
use crate::state::documents::PackageRow;
use hive_core::package_lens::{Change, Channel, Lens, Upgrade, Version, Versions};

/// Space between two segments of a lens.
const SEGMENT_GAP: Pixels = px(4.);
/// Room on both sides of a segment's content, which its hover surface and its click cover.
pub(super) const PADDING: Pixels = px(5.);
/// A registry error is cut here: the lens is no place for a log. The tooltip has all of it.
const REASON_MAX_CHARS: usize = 60;
/// Advisory IDs a tooltip lists before it counts the rest.
const LISTED_ADVISORIES: usize = 6;

/// How a segment is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Quiet,
    Current,
    Patch,
    Minor,
    Major,
    Danger,
}

/// What clicking a segment does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Write this requirement into the entry.
    Upgrade(String),
    /// Open these OSV advisories.
    Advisories(Vec<String>),
}

/// One piece of a lens: an icon, a short text, what it means in full, and maybe an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Segment {
    pub(super) icon: Lucide,
    pub(super) text: String,
    pub(super) tone: Tone,
    pub(super) action: Option<Action>,
    /// Lines shown under the pointer.
    pub(super) tooltip: String,
}

impl Segment {
    fn status(icon: Lucide, text: impl Into<String>, tone: Tone, tooltip: String) -> Self {
        Self {
            icon,
            text: text.into(),
            tone,
            action: None,
            tooltip,
        }
    }
}

/// The segments of one row, left to right: advisories first, then what the registry said.
pub(super) fn segments(row: &PackageRow) -> Vec<Segment> {
    let lens = match row {
        PackageRow::Checking(_) => {
            return vec![Segment::status(
                Lucide::CircleDashed,
                "checking…",
                Tone::Quiet,
                "Looking up newer releases with the project's package manager".into(),
            )];
        }
        PackageRow::Ready(lens) => lens,
    };
    let advisories = (!lens.advisories.is_empty()).then(|| Segment {
        icon: Lucide::ShieldAlert,
        text: advisory_count(lens.advisories.len(), lens.more_advisories),
        tone: Tone::Danger,
        action: Some(Action::Advisories(lens.advisories.clone())),
        tooltip: advisories_tooltip(lens),
    });
    let versions = match &lens.versions {
        Versions::Unavailable(reason) => vec![Segment::status(
            Lucide::CircleAlert,
            shorten(reason),
            Tone::Quiet,
            format!("No versions from the registry: {reason}"),
        )],
        Versions::Found(upgrades) if upgrades.is_empty() => vec![Segment::status(
            Lucide::CircleCheck,
            "up to date",
            Tone::Current,
            lines([
                Some(format!(
                    "The registry has no release newer than \"{}\"",
                    lens.dependency.spec
                )),
                locked(lens),
            ]),
        )],
        Versions::Found(upgrades) => upgrades.iter().map(|u| upgrade(lens, u)).collect(),
    };
    advisories.into_iter().chain(versions).collect()
}

fn upgrade(lens: &Lens, upgrade: &Upgrade) -> Segment {
    let (tone, step) = match upgrade.change {
        Some(Change::Major) => (Tone::Major, " major"),
        Some(Change::Minor) => (Tone::Minor, " minor"),
        Some(Change::Patch) => (Tone::Patch, " patch"),
        None => (Tone::Minor, ""),
    };
    let meaning = match (upgrade.channel, upgrade.change) {
        (Channel::Wanted, _) => "The newest release the current range allows",
        (Channel::Latest, Some(Change::Major)) => {
            "The latest release, a new major version: check it for breaking changes"
        }
        (Channel::Latest, _) => "The latest release on the registry",
    };
    Segment {
        icon: Lucide::ArrowUp,
        text: format!("{}{step}", upgrade.version),
        tone,
        action: Some(Action::Upgrade(upgrade.spec.clone())),
        tooltip: lines([
            Some(format!(
                "Click to change \"{}\" to \"{}\"",
                lens.dependency.spec, upgrade.spec
            )),
            Some(meaning.into()),
            locked(lens),
        ]),
    }
}

fn advisories_tooltip(lens: &Lens) -> String {
    let ids = &lens.advisories;
    let heading = match &lens.resolved {
        Some(version) => format!("Known advisories for {} {version}:", lens.dependency.name),
        None => format!("Known advisories for {}:", lens.dependency.name),
    };
    let hidden = ids.len().saturating_sub(LISTED_ADVISORIES);
    let rest = match (hidden, lens.more_advisories) {
        (0, false) => None,
        (0, true) => Some("and more".to_owned()),
        (count, false) => Some(format!("and {count} more")),
        (count, true) => Some(format!("and {count}+ more")),
    };
    let click = if ids.len() == 1 {
        "Click to open it on osv.dev"
    } else {
        "Click to choose one to open on osv.dev"
    };
    std::iter::once(heading)
        .chain(ids.iter().take(LISTED_ADVISORIES).cloned())
        .chain(rest)
        .chain(std::iter::once(click.to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The locked release, when the requirement does not already name it exactly.
fn locked(lens: &Lens) -> Option<String> {
    let resolved = lens.resolved.as_ref()?;
    let named = Version::parse(&lens.dependency.spec).ok();
    (named.as_ref() != Some(resolved)).then(|| format!("Locked at {resolved}"))
}

fn lines<const N: usize>(lines: [Option<String>; N]) -> String {
    lines.into_iter().flatten().collect::<Vec<_>>().join("\n")
}

fn advisory_count(count: usize, more: bool) -> String {
    let plus = if more { "+" } else { "" };
    let noun = if count == 1 && !more {
        "advisory"
    } else {
        "advisories"
    };
    format!("{count}{plus} {noun}")
}

fn shorten(reason: &str) -> String {
    match reason.char_indices().nth(REASON_MAX_CHARS) {
        Some((end, _)) => format!("{}…", reason[..end].trim_end()),
        None => reason.to_owned(),
    }
}

/// The horizontal extent of each segment whose content is `widths` wide, placed from `start`.
/// Segments stop before the first one that would cross `limit`.
pub(super) fn layout(start: Pixels, limit: Pixels, widths: &[Pixels]) -> Vec<(Pixels, Pixels)> {
    let mut left = start;
    let mut extents = Vec::new();
    for width in widths {
        let right = left + PADDING * 2. + *width;
        if right > limit {
            break;
        }
        extents.push((left, right));
        left = right + SEGMENT_GAP;
    }
    extents
}

#[cfg(test)]
mod tests {
    use super::{Action, PackageRow, REASON_MAX_CHARS, Tone, layout, segments};
    use gpui_kit::px;
    use hive_core::package_lens::{Change, Channel, Lens, Manifest, Upgrade, Version, Versions};

    fn row(versions: Versions, advisories: &[&str], more_advisories: bool) -> PackageRow {
        row_for("^1.2.0", None, versions, advisories, more_advisories)
    }

    fn row_for(
        spec: &str,
        resolved: Option<&str>,
        versions: Versions,
        advisories: &[&str],
        more_advisories: bool,
    ) -> PackageRow {
        let text = format!(r#"{{"dependencies":{{"pkg":"{spec}"}}}}"#);
        PackageRow::Ready(Lens {
            dependency: Manifest::parse(&text).unwrap().entries.remove(0),
            resolved: resolved.map(|v| Version::parse(v).unwrap()),
            versions,
            advisories: advisories.iter().map(|id| id.to_string()).collect(),
            more_advisories,
        })
    }

    fn tooltips(row: &PackageRow) -> Vec<String> {
        segments(row).into_iter().map(|s| s.tooltip).collect()
    }

    fn upgrade_to(version: &str, change: Option<Change>, spec: &str) -> Upgrade {
        Upgrade {
            channel: Channel::Latest,
            version: Version::parse(version).unwrap(),
            change,
            spec: spec.into(),
        }
    }

    fn texts(row: &PackageRow) -> Vec<(String, Option<Action>)> {
        segments(row)
            .into_iter()
            .map(|segment| (segment.text, segment.action))
            .collect()
    }

    #[test]
    fn each_upgrade_is_its_own_click_with_the_exact_requirement() {
        let upgrades = vec![
            upgrade_to("1.4.2", Some(Change::Minor), "^1.4.2"),
            upgrade_to("2.0.0", Some(Change::Major), "^2.0.0"),
            upgrade_to("2.1.0", None, ">=2.1.0"),
        ];
        let segments = segments(&row(Versions::Found(upgrades), &[], false));
        assert_eq!(
            segments
                .iter()
                .map(|s| (s.text.as_str(), s.tone, s.action.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    "1.4.2 minor",
                    Tone::Minor,
                    Some(Action::Upgrade("^1.4.2".into()))
                ),
                (
                    "2.0.0 major",
                    Tone::Major,
                    Some(Action::Upgrade("^2.0.0".into()))
                ),
                (
                    "2.1.0",
                    Tone::Minor,
                    Some(Action::Upgrade(">=2.1.0".into()))
                ),
            ]
        );
    }

    #[test]
    fn advisories_come_first_and_statuses_take_no_clicks() {
        let advised = row(Versions::Found(Vec::new()), &["GHSA-1", "GHSA-2"], false);
        assert_eq!(
            texts(&advised),
            [
                (
                    "2 advisories".into(),
                    Some(Action::Advisories(vec!["GHSA-1".into(), "GHSA-2".into()]))
                ),
                ("up to date".into(), None),
            ]
        );
        let paged = row(Versions::Found(Vec::new()), &["GHSA-1"], true);
        assert_eq!(texts(&paged)[0].0, "1+ advisories");
        let single = row(Versions::Found(Vec::new()), &["GHSA-1"], false);
        assert_eq!(texts(&single)[0].0, "1 advisory");
        let waiting = PackageRow::Checking(
            Manifest::parse(r#"{"dependencies":{"pkg":"1"}}"#)
                .unwrap()
                .entries
                .remove(0),
        );
        assert_eq!(texts(&waiting), [("checking…".into(), None)]);
    }

    #[test]
    fn an_upgrade_tooltip_says_what_the_click_writes_and_what_is_locked() {
        let mut wanted = upgrade_to("1.4.2", Some(Change::Minor), "^1.4.2");
        wanted.channel = Channel::Wanted;
        let latest = upgrade_to("2.0.0", Some(Change::Major), "^2.0.0");
        let locked = row_for(
            "^1.2.0",
            Some("1.3.0"),
            Versions::Found(vec![wanted, latest]),
            &[],
            false,
        );
        assert_eq!(
            tooltips(&locked),
            [
                "Click to change \"^1.2.0\" to \"^1.4.2\"\n\
                 The newest release the current range allows\n\
                 Locked at 1.3.0",
                "Click to change \"^1.2.0\" to \"^2.0.0\"\n\
                 The latest release, a new major version: check it for breaking changes\n\
                 Locked at 1.3.0",
            ]
        );
        // A requirement that already names the locked release does not repeat it.
        let pinned = row_for(
            "1.2.0",
            Some("1.2.0"),
            Versions::Found(vec![upgrade_to("1.2.5", Some(Change::Patch), "1.2.5")]),
            &[],
            false,
        );
        assert_eq!(
            tooltips(&pinned),
            ["Click to change \"1.2.0\" to \"1.2.5\"\nThe latest release on the registry"]
        );
    }

    #[test]
    fn an_advisory_tooltip_lists_ids_and_counts_the_rest() {
        let ids: Vec<String> = (1..=8).map(|n| format!("GHSA-{n}")).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let many = row_for(
            "^1.2.0",
            Some("1.2.7"),
            Versions::Found(Vec::new()),
            &ids,
            true,
        );
        assert_eq!(
            tooltips(&many)[0],
            "Known advisories for pkg 1.2.7:\nGHSA-1\nGHSA-2\nGHSA-3\nGHSA-4\nGHSA-5\nGHSA-6\n\
             and 2+ more\nClick to choose one to open on osv.dev"
        );
        let one = row(Versions::Found(Vec::new()), &["GHSA-1"], false);
        assert_eq!(
            tooltips(&one)[0],
            "Known advisories for pkg:\nGHSA-1\nClick to open it on osv.dev"
        );
        let paged = row(Versions::Found(Vec::new()), &["GHSA-1", "GHSA-2"], true);
        assert!(tooltips(&paged)[0].contains("GHSA-2\nand more\n"));
    }

    #[test]
    fn a_status_tooltip_holds_what_its_text_leaves_out() {
        let reason = format!("npm view failed: {}", "detail ".repeat(20));
        let failed = row(Versions::Unavailable(reason.clone()), &[], false);
        assert_eq!(
            tooltips(&failed),
            [format!("No versions from the registry: {reason}")]
        );
        let current = row_for(
            "^1.2.0",
            Some("1.2.9"),
            Versions::Found(Vec::new()),
            &[],
            false,
        );
        assert_eq!(
            tooltips(&current),
            ["The registry has no release newer than \"^1.2.0\"\nLocked at 1.2.9"]
        );
    }

    #[test]
    fn a_long_registry_error_is_cut_on_a_character() {
        let reason = format!("{}é tail", "x".repeat(REASON_MAX_CHARS - 1));
        let failed = row(Versions::Unavailable(reason), &[], false);
        let text = &texts(&failed)[0].0;
        assert_eq!(text.chars().count(), REASON_MAX_CHARS + 1);
        assert!(text.ends_with("é…"), "{text}");
        let short = row(Versions::Unavailable("npm view failed".into()), &[], false);
        assert_eq!(texts(&short), [("npm view failed".into(), None)]);
    }

    #[test]
    fn segments_line_up_and_stop_before_the_edge() {
        let widths = [px(40.), px(30.), px(50.)];
        let unbounded = layout(px(100.), px(1000.), &widths);
        assert_eq!(
            unbounded,
            [
                (px(100.), px(150.)),
                (px(154.), px(194.)),
                (px(198.), px(258.))
            ]
        );
        // The third segment would cross the edge: it is left out, the first two stay.
        assert_eq!(layout(px(100.), px(257.), &widths), unbounded[..2]);
        assert!(layout(px(100.), px(149.), &widths).is_empty());
    }
}
