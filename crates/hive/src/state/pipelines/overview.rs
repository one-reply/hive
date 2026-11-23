//! Every project's runs and launch drafts at a glance, for a summary outside the panel.
use super::*;

/// Finished runs the overview lists; older ones stay in their project's library.
const RECENT_LIMIT: usize = 5;

/// Where a run or a draft is listed, in display order: what waits for the user comes before
/// what is working, and finished runs come last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    NeedsYou,
    Running,
    Drafts,
    Recent,
}

impl Group {
    pub(crate) fn of(status: RunStatus) -> Self {
        match status {
            RunStatus::NeedsYou | RunStatus::Failed => Self::NeedsYou,
            RunStatus::Running => Self::Running,
            RunStatus::Completed | RunStatus::Cancelled => Self::Recent,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NeedsYou => "Needs you",
            Self::Running => "Running",
            Self::Drafts => "Drafts",
            Self::Recent => "Recent",
        }
    }
}

/// A run as the overview lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListedRun {
    pub(crate) id: u64,
    pub(crate) status: RunStatus,
    /// False once the run's project was removed from Hive: there is no panel to show it in.
    pub(crate) reachable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Run(ListedRun),
    /// The unsubmitted launch form of the project at this path.
    Draft(PathBuf),
}

/// The runs and drafts of every project, grouped for a list that has to stay short.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Overview {
    /// Groups in display order, each newest first; a group without entries is left out.
    pub(crate) groups: Vec<(Group, Vec<Entry>)>,
    /// Finished runs older than the ones listed.
    pub(crate) hidden_recent: usize,
    /// Runs the user can open that wait for them or failed.
    pub(crate) needs_you: usize,
    /// Runs the user can open that are working.
    pub(crate) running: usize,
}

impl Overview {
    pub(crate) fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// Groups `runs` (newest first) and `drafts`. Every live run and every draft is listed;
/// of the finished runs only the newest few are.
fn arrange(
    runs: impl IntoIterator<Item = ListedRun>,
    drafts: impl IntoIterator<Item = PathBuf>,
) -> Overview {
    let mut overview = Overview::default();
    let mut listed: Vec<(Group, Entry)> = Vec::new();
    let mut recent = 0;
    for run in runs {
        let group = Group::of(run.status);
        match group {
            Group::NeedsYou if run.reachable => overview.needs_you += 1,
            Group::Running if run.reachable => overview.running += 1,
            Group::Recent => {
                recent += 1;
                if recent > RECENT_LIMIT {
                    overview.hidden_recent += 1;
                    continue;
                }
            }
            _ => {}
        }
        listed.push((group, Entry::Run(run)));
    }
    listed.extend(
        drafts
            .into_iter()
            .map(|owner| (Group::Drafts, Entry::Draft(owner))),
    );
    // The sort is stable, so each group keeps the newest run first.
    listed.sort_by_key(|(group, _)| *group);
    for (group, entry) in listed {
        match overview.groups.last_mut() {
            Some((last, entries)) if *last == group => entries.push(entry),
            _ => overview.groups.push((group, vec![entry])),
        }
    }
    overview
}

impl Pipelines {
    /// What every project is running, waiting on and drafting. A draft of a project that was
    /// removed from Hive is left out: nothing could open it.
    pub(crate) fn overview(&self, cx: &App) -> Overview {
        let library = &self.store.read(cx).state.pipelines;
        let runs = library.runs.iter().rev().map(|run| ListedRun {
            id: run.id(),
            status: self.status(run, cx),
            reachable: self.registered(run.owner(), cx),
        });
        let drafts = library
            .drafts
            .iter()
            .filter(|draft| self.registered(&draft.owner, cx))
            .map(|draft| draft.owner.clone());
        arrange(runs, drafts)
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: `gpui_kit::*` brings in GPUI's `test` attribute, which shadows `#[test]`.
    use super::{Entry, Group, ListedRun, Overview, RECENT_LIMIT, RunStatus, arrange};
    use std::path::PathBuf;

    fn run(id: u64, status: RunStatus) -> ListedRun {
        ListedRun {
            id,
            status,
            reachable: true,
        }
    }

    fn listed(id: u64, status: RunStatus) -> Entry {
        Entry::Run(run(id, status))
    }

    #[test]
    fn runs_that_wait_for_the_user_come_before_working_and_finished_ones() {
        use RunStatus::*;
        let mut runs = [Completed, Running, Failed, Cancelled, NeedsYou];
        runs.sort_by_key(|status| Group::of(*status));
        assert_eq!(runs, [Failed, NeedsYou, Running, Completed, Cancelled]);
        assert_eq!(Group::of(Failed), Group::of(NeedsYou));
        assert_eq!(Group::of(Completed), Group::of(Cancelled));
        assert!(Group::Running < Group::Drafts && Group::Drafts < Group::Recent);
    }

    #[test]
    fn groups_are_listed_in_order_of_urgency_each_newest_first() {
        use RunStatus::*;
        // Newest first, as the store is read.
        let runs = [
            run(6, Completed),
            run(5, Running),
            run(4, NeedsYou),
            run(3, Cancelled),
            run(2, Failed),
            run(1, Running),
        ];
        let overview = arrange(runs, [PathBuf::from("/api")]);
        assert_eq!(
            overview.groups,
            [
                (
                    Group::NeedsYou,
                    vec![listed(4, NeedsYou), listed(2, Failed)]
                ),
                (Group::Running, vec![listed(5, Running), listed(1, Running)]),
                (Group::Drafts, vec![Entry::Draft("/api".into())]),
                (
                    Group::Recent,
                    vec![listed(6, Completed), listed(3, Cancelled)]
                ),
            ]
        );
        assert_eq!((overview.needs_you, overview.running), (2, 2));
        assert_eq!(overview.hidden_recent, 0);
    }

    #[test]
    fn only_the_newest_finished_runs_are_listed_and_the_rest_are_counted() {
        let finished = (1..=8).rev().map(|id| run(id, RunStatus::Completed));
        let live = (9..=20).map(|id| run(id, RunStatus::Running));
        let overview = arrange(live.chain(finished), []);
        let [(Group::Running, running), (Group::Recent, recent)] = &overview.groups[..] else {
            panic!("unexpected groups: {:?}", overview.groups);
        };
        // A live run is never dropped, however many there are.
        assert_eq!(running.len(), 12);
        assert_eq!(recent.len(), RECENT_LIMIT);
        assert_eq!(recent[0], listed(8, RunStatus::Completed));
        assert_eq!(overview.hidden_recent, 3);
    }

    #[test]
    fn a_run_whose_project_was_removed_is_listed_but_not_counted() {
        let orphan = ListedRun {
            reachable: false,
            ..run(1, RunStatus::NeedsYou)
        };
        let overview = arrange([orphan, run(2, RunStatus::Running)], []);
        assert_eq!((overview.needs_you, overview.running), (0, 1));
        assert_eq!(
            overview.groups[0],
            (Group::NeedsYou, vec![Entry::Run(orphan)])
        );
    }

    #[test]
    fn nothing_to_list_is_an_empty_overview() {
        let overview = arrange([], []);
        assert_eq!(overview, Overview::default());
        assert!(overview.is_empty());
        assert!(!arrange([], [PathBuf::from("/api")]).is_empty());
    }
}
