//! What the library page lists, top to bottom. The page is a virtual list, so this order is
//! decided for every run while only the rows in view are drawn.
use crate::state::pipelines::{Group, RunStatus};

pub(super) const ELSEWHERE: &str = "Needs you in other projects";

/// One entry of the library page. The page is a virtual list, so a history of any length
/// costs only the rows in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::ui::workspace::pipelines) enum Entry {
    /// The title, the actions and the template cards.
    Workflows,
    Section(&'static str),
    Run(u64),
    /// The project's unsubmitted launch form.
    Draft,
    /// A run of another project that waits for the user.
    Elsewhere(u64),
    NoRuns,
}

/// The page from top to bottom. `runs` are the project's runs, newest first, and every
/// section keeps that order; a section without entries has no heading. The draft sits in
/// the same place as in the overview card: after the live runs, before the finished ones.
pub(super) fn entries(runs: &[(u64, RunStatus)], has_draft: bool, elsewhere: &[u64]) -> Vec<Entry> {
    let mut entries = vec![Entry::Workflows];
    let mut sorted: Vec<_> = runs
        .iter()
        .map(|&(id, status)| (Group::of(status), Entry::Run(id)))
        .chain(has_draft.then_some((Group::Drafts, Entry::Draft)))
        .collect();
    // The sort is stable, so each section keeps the newest run first.
    sorted.sort_by_key(|(group, _)| *group);
    let mut previous = None;
    for (group, entry) in sorted {
        if previous != Some(group) {
            entries.push(Entry::Section(group.label()));
            previous = Some(group);
        }
        entries.push(entry);
    }
    if !elsewhere.is_empty() {
        entries.push(Entry::Section(ELSEWHERE));
        entries.extend(elsewhere.iter().copied().map(Entry::Elsewhere));
    }
    if runs.is_empty() {
        entries.push(Entry::NoRuns);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::{ELSEWHERE, Entry, RunStatus, entries};

    #[test]
    fn the_page_lists_each_section_under_its_heading_newest_run_first() {
        use RunStatus::*;
        // Newest first, as the store is read.
        let runs = [
            (5, Completed),
            (4, Running),
            (3, NeedsYou),
            (2, Cancelled),
            (1, Failed),
        ];
        assert_eq!(
            entries(&runs, false, &[9]),
            [
                Entry::Workflows,
                Entry::Section("Needs you"),
                Entry::Run(3),
                Entry::Run(1),
                Entry::Section("Running"),
                Entry::Run(4),
                Entry::Section("Recent"),
                Entry::Run(5),
                Entry::Run(2),
                Entry::Section(ELSEWHERE),
                Entry::Elsewhere(9),
            ]
        );
    }

    #[test]
    fn a_project_without_runs_says_so_under_the_workflows() {
        assert_eq!(entries(&[], false, &[]), [Entry::Workflows, Entry::NoRuns]);
        assert_eq!(
            entries(&[], false, &[9]),
            [
                Entry::Workflows,
                Entry::Section(ELSEWHERE),
                Entry::Elsewhere(9),
                Entry::NoRuns,
            ]
        );
    }

    #[test]
    fn the_draft_comes_after_the_live_runs_and_before_the_finished_ones() {
        use RunStatus::*;
        let runs = [(3, Completed), (2, Running), (1, NeedsYou)];
        assert_eq!(
            entries(&runs, true, &[]),
            [
                Entry::Workflows,
                Entry::Section("Needs you"),
                Entry::Run(1),
                Entry::Section("Running"),
                Entry::Run(2),
                Entry::Section("Drafts"),
                Entry::Draft,
                Entry::Section("Recent"),
                Entry::Run(3),
            ]
        );
    }

    #[test]
    fn a_draft_alone_is_listed_and_the_project_still_has_no_runs() {
        assert_eq!(
            entries(&[], true, &[]),
            [
                Entry::Workflows,
                Entry::Section("Drafts"),
                Entry::Draft,
                Entry::NoRuns,
            ]
        );
    }
}
