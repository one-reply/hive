//! What the overview's card says of each run and draft, read out of the store and the
//! project list before anything is drawn.
use super::super::library::{RunRow, draft_title};
use super::*;
use crate::state::pipelines::{Entry, ListedRun};
use hive_core::identity::ProjectIdentity;
use hive_core::pipeline::draft::LaunchDraft;

/// Where a click on a line leads.
#[derive(Clone)]
pub(super) enum Target {
    Run(u64),
    Draft(PathBuf),
}

/// The project a line belongs to, named and marked as the sidebar shows it.
pub(super) struct ProjectMark {
    pub(super) name: String,
    /// The badge and its color; `None` for a project that was removed from Hive.
    pub(super) badge: Option<(ProjectIdentity, Hsla)>,
}

/// How the local project at `owner` is shown. A project that is no longer in `projects`
/// keeps its folder's name and has no badge.
fn project_mark(projects: &[Project], owner: &Path) -> ProjectMark {
    let listed = projects
        .iter()
        .enumerate()
        .find(|(_, project)| project.host.is_local() && project.path == owner);
    match listed {
        Some((ix, project)) => ProjectMark {
            name: project.name.clone(),
            badge: Some((
                project.identity.clone(),
                project_badge::color(&project.identity, ix),
            )),
        },
        None => ProjectMark {
            name: path_name(owner),
            badge: None,
        },
    }
}

/// What the card shows of one run or draft, read out of the store before rendering.
pub(super) struct Line {
    pub(super) target: Target,
    /// `None` for a draft.
    pub(super) status: Option<RunStatus>,
    pub(super) title: String,
    pub(super) project: ProjectMark,
    /// What follows the project's name on the second line.
    pub(super) facts: String,
    pub(super) progress: Option<RunProgress>,
    /// False for a run whose project was removed from Hive.
    pub(super) reachable: bool,
}

fn run_line(listed: ListedRun, run: &Run, project: ProjectMark, now: u64) -> Line {
    let row = RunRow::of(run, listed.status, now);
    let facts = if listed.reachable {
        row.detail
    } else {
        "project removed from Hive".into()
    };
    Line {
        target: Target::Run(listed.id),
        status: Some(listed.status),
        title: run::task_title(&row.task),
        project,
        facts,
        progress: (!row.finished).then(|| run.progress()),
        reachable: listed.reachable,
    }
}

fn draft_line(draft: &LaunchDraft, project: ProjectMark) -> Line {
    Line {
        target: Target::Draft(draft.owner.clone()),
        status: None,
        title: draft_title(&draft.task),
        project,
        facts: draft.template.name.clone(),
        progress: None,
        reachable: true,
    }
}

/// The overview's groups with each entry read out of the store. An entry that vanished
/// between the overview and this read is skipped.
pub(super) fn read(overview: &Overview, cx: &App) -> Vec<(&'static str, Vec<Line>)> {
    let app = AppState::get(cx);
    let library = &app.store.read(cx).state.pipelines;
    let projects = &app.projects.read(cx).items;
    let now = hive_core::pipeline::now_ms();
    let line = |entry: &Entry| match entry {
        Entry::Run(listed) => {
            let run = library.runs.iter().find(|run| run.id() == listed.id)?;
            let project = project_mark(projects, run.owner());
            Some(run_line(*listed, run, project, now))
        }
        Entry::Draft(owner) => {
            let draft = library.drafts.iter().find(|draft| &draft.owner == owner)?;
            Some(draft_line(draft, project_mark(projects, owner)))
        }
    };
    overview
        .groups
        .iter()
        .map(|(group, entries)| (group.label(), entries.iter().filter_map(line).collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::project_mark;
    use crate::ui::project_badge;
    use hive_core::host::Host;
    use hive_core::identity::ProjectIdentity;
    use hive_core::projects::Project;
    use std::path::Path;

    fn project(name: &str, path: &str) -> Project {
        Project {
            name: name.into(),
            path: path.into(),
            host: Host::Local,
            filter: Default::default(),
            identity: ProjectIdentity::default(),
            worktrees: Vec::new(),
        }
    }

    #[test]
    fn a_line_is_marked_with_its_project_as_the_sidebar_shows_it() {
        let projects = [project("Backend", "/work/api"), project("web", "/work/web")];
        let web = project_mark(&projects, Path::new("/work/web"));
        assert_eq!(web.name, "web");
        // The second project in the sidebar has the second palette color there too.
        let (_, color) = web.badge.expect("a listed project has a badge");
        assert_eq!(color, project_badge::color(&ProjectIdentity::default(), 1));
        // A custom display name wins over the folder's.
        assert_eq!(
            project_mark(&projects, Path::new("/work/api")).name,
            "Backend"
        );
    }

    #[test]
    fn a_removed_project_keeps_its_folder_name_and_has_no_badge() {
        let projects = [project("api", "/work/api")];
        let gone = project_mark(&projects, Path::new("/work/gone"));
        assert_eq!(gone.name, "gone");
        assert!(gone.badge.is_none());
    }
}
