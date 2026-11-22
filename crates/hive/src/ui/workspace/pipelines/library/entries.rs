//! What the library page lists, top to bottom. The page is a virtual list, so this order is
//! decided for every run while only the rows in view are drawn.
use crate::state::pipelines::RunStatus;

pub(super) const ELSEWHERE: &str = "Needs you in other projects";

/// One entry of the library page. The page is a virtual list, so a history of any length
/// costs only the rows in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(in crate::ui::workspace::pipelines) enum Entry {
    /// The title, the actions and the template cards.
    Workflows,
    Section(&'static str),
    Run(u64),
    /// A run of another project that waits for the user.
    Elsewhere(u64),
    NoRuns,
}

/// The page from top to bottom. `runs` are the project's runs, newest first, and every
/// section keeps that order; a section without runs has no heading.
pub(super) fn entries(runs: &[(u64, RunStatus)], elsewhere: &[u64]) -> Vec<Entry> {
    let mut entries = vec![Entry::Workflows];
    let mut sorted = runs.to_vec();
    // The sort is stable, so each section keeps the newest run first.
    sorted.sort_by_key(|(_, status)| history_section(*status).0);
    let mut previous = None;
    for (id, status) in sorted {
        let (_, section) = history_section(status);
        if previous != Some(section) {
            entries.push(Entry::Section(section));
            previous = Some(section);
        }
        entries.push(Entry::Run(id));
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

/// Where a run is listed in the history, in display order: what waits for the user comes
/// before what is working, and finished runs come last.
fn history_section(status: RunStatus) -> (u8, &'static str) {
    match status {
        RunStatus::NeedsYou | RunStatus::Failed => (0, "Needs you"),
        RunStatus::Running => (1, "Running"),
        RunStatus::Completed | RunStatus::Cancelled => (2, "Recent"),
    }
}

#[cfg(test)]
mod tests {
    use super::{ELSEWHERE, Entry, RunStatus, entries, history_section};

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
            entries(&runs, &[9]),
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
        assert_eq!(entries(&[], &[]), [Entry::Workflows, Entry::NoRuns]);
        assert_eq!(
            entries(&[], &[9]),
            [
                Entry::Workflows,
                Entry::Section(ELSEWHERE),
                Entry::Elsewhere(9),
                Entry::NoRuns,
            ]
        );
    }

    #[test]
    fn runs_that_wait_for_the_user_are_listed_before_working_and_finished_ones() {
        use RunStatus::*;
        let mut runs = [Completed, Running, Failed, Cancelled, NeedsYou];
        runs.sort_by_key(|status| history_section(*status).0);
        assert_eq!(runs, [Failed, NeedsYou, Running, Completed, Cancelled]);
        assert_eq!(history_section(Failed).1, history_section(NeedsYou).1);
        assert_eq!(history_section(Completed).1, history_section(Cancelled).1);
    }
}
