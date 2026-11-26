//! `state.json` as released versions of Hive wrote it, one sample per schema version.
//!
//! Each sample was written by that release's own `hive-core` (`State::load` then
//! `State::save`, built from the release tag), not typed by hand: the two newest also carry
//! a finished pipeline run. Their content has the shape of a real data folder (nine projects,
//! split layouts, terminal, file and diff tabs) with every name, path, text and id replaced.
//! How they were made is in `wiki/decisions/state-schema-version.md`.

use super::*;

/// The release that wrote the sample, the schema version it stamped, and the file.
const SAMPLES: [(&str, u32, &str); 7] = [
    (
        "1.0.25",
        1,
        include_str!("../fixtures/state-1.0.25-schema-1.json"),
    ),
    (
        "1.0.26",
        2,
        include_str!("../fixtures/state-1.0.26-schema-2.json"),
    ),
    (
        "1.0.27",
        3,
        include_str!("../fixtures/state-1.0.27-schema-3.json"),
    ),
    (
        "1.0.28",
        4,
        include_str!("../fixtures/state-1.0.28-schema-4.json"),
    ),
    (
        "1.0.30",
        5,
        include_str!("../fixtures/state-1.0.30-schema-5.json"),
    ),
    (
        "1.0.31",
        6,
        include_str!("../fixtures/state-1.0.31-schema-6.json"),
    ),
    (
        "1.0.39",
        7,
        include_str!("../fixtures/state-1.0.39-schema-7.json"),
    ),
];

struct Folder(PathBuf);

impl Folder {
    fn holding(release: &str, file: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-state-compatibility-{release}-{}-{}",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("state.json"), file).unwrap();
        Self(path)
    }

    fn load(&self) -> LoadedState {
        State::load_from(&self.0).unwrap()
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// `state` as it would be written. Through text, as the file is: an `f32` setting prints as
/// the shortest number that reads back to it, not as its exact `f64` expansion.
fn as_json(state: &State) -> Value {
    json(&serde_json::to_string(state).unwrap())
}

/// How many tabs a saved workspace holds, whatever nests them.
fn tabs(workspace: &Value) -> usize {
    match workspace {
        Value::Object(object) => {
            let own = usize::from(matches!(
                object.get("kind").and_then(Value::as_str),
                Some("terminal" | "file" | "diff")
            ));
            own + object.values().map(tabs).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(tabs).sum(),
        _ => 0,
    }
}

fn strings<'a>(list: &'a Value, field: &str) -> Vec<&'a str> {
    let entries = list.as_array().map(Vec::as_slice).unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| entry[field].as_str())
        .collect()
}

/// Each value under `written` is in `read` at the same path; objects are compared key by key.
fn assert_reads_as_written(read: &Value, written: &Value, release: &str, path: &str) {
    let Some(fields) = written.as_object() else {
        assert_eq!(read, written, "{release}: {path}");
        return;
    };
    for (name, value) in fields {
        assert_reads_as_written(&read[name], value, release, &format!("{path}.{name}"));
    }
}

#[test]
fn every_released_schema_loads_whole_with_nothing_reset_or_dropped() {
    for (release, schema, file) in SAMPLES {
        let written = json(file);
        assert_eq!(written["version"], schema, "{release}");
        let loaded = Folder::holding(release, file).load();
        assert!(
            loaded.recovery.is_none(),
            "{release}: {:?}",
            loaded.recovery.map(|recovery| recovery.cause)
        );
        let read = as_json(&loaded.state);

        assert_eq!(
            strings(&read["projects"], "path"),
            strings(&written["projects"], "path")
        );
        assert_eq!(read["projects"].as_array().unwrap().len(), 9, "{release}");
        assert_eq!(
            strings(&read["workspace"]["projects"], "project"),
            strings(&written["workspace"]["projects"], "project"),
            "{release}"
        );
        assert_eq!(tabs(&read["workspace"]), 23, "{release}");
        assert_eq!(tabs(&written["workspace"]), 23, "{release}");
        assert_eq!(
            read["workspace"]["selection"], written["workspace"]["selection"],
            "{release}"
        );
        // Every setting the release wrote is read as written; settings added since then
        // may sit beside them with their defaults.
        assert_reads_as_written(&read["settings"], &written["settings"], release, "settings");
        let runs = |state: &Value| -> Vec<(u64, String)> {
            let runs = state["pipelines"]["runs"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            runs.iter()
                .map(|run| (run["id"].as_u64().unwrap(), run["phase"].to_string()))
                .collect()
        };
        assert_eq!(runs(&read), runs(&written), "{release}");
        assert_eq!(runs(&read).len(), usize::from(schema >= 2), "{release}");
    }
}

#[test]
fn what_a_release_did_not_know_yet_takes_its_default() {
    let load = |index: usize| {
        let (release, _, file) = SAMPLES[index];
        Folder::holding(release, file).load().state
    };
    // Before pipelines: the stock library.
    let before_pipelines = load(0);
    assert_eq!(
        before_pipelines.pipelines,
        crate::pipeline::Library::default()
    );
    assert!(!before_pipelines.settings.option_as_meta);
    // Before dictation, which is every release so far: unconfigured, with the stock prompt.
    // Before the package versions switch, too: dependency versions show.
    for index in 0..SAMPLES.len() {
        let settings = load(index).settings;
        assert_eq!(settings.dictation, crate::dictation::Recipe::default());
        assert!(settings.files.package_versions);
    }
    // Before the plan approval policy and review conditions: every recipe, a run's own copy
    // included, asks the user and runs each of its reviews, and no run leaves a review out.
    for index in 1..SAMPLES.len() {
        let pipelines = load(index).pipelines;
        let recipes = pipelines.templates.iter();
        for recipe in recipes.chain(pipelines.runs.iter().map(|run| run.template())) {
            assert_eq!(recipe.plan_approval, crate::pipeline::PlanApproval::Ask);
            assert!(!recipe.has_review_conditions());
        }
        for run in &pipelines.runs {
            assert!(run.skipped_reviews().is_empty());
        }
    }
    // Before the prompt library: its stock prompts, with the saved run kept.
    let before_prompts = load(1);
    assert_eq!(
        before_prompts.pipelines.prompts(),
        crate::pipeline::Library::default().prompts()
    );
    assert_eq!(before_prompts.pipelines.runs.len(), 1);
    assert_eq!(before_prompts.pipelines.templates.len(), 3);
    assert!(before_prompts.pipelines.templates[2].shared_contract);
    // The original specialized reviewer keeps its id and gains today's stock instructions.
    let legacy = load(2).pipelines;
    let current = crate::pipeline::Library::default();
    assert_eq!(
        legacy.prompt(11).unwrap().text,
        current.prompt(15).unwrap().text
    );
    assert!(legacy.prompt(15).is_none());
    // A finished run keeps its reports, sessions and timings.
    for index in 5..SAMPLES.len() {
        let state = as_json(&load(index));
        let run = &state["pipelines"]["runs"][0];
        assert_eq!(run["phase"], "accepted");
        let count = |field: &str| run[field].as_array().unwrap().len();
        assert_eq!(
            (count("reports"), count("sessions"), count("timings")),
            (4, 4, 13)
        );
        assert_eq!(count("history"), 6);
    }
}

#[test]
fn a_recipe_with_a_review_before_qa_is_saved_and_read_back_whole() {
    let (release, _, file) = SAMPLES[6];
    let folder = Folder::holding(release, file);
    let mut state = folder.load().state;
    // The sample's recipes predate stage roles: QA is their unmarked first review.
    let recipe = &mut state.pipelines.templates[0];
    assert_eq!(recipe.move_review(1, 0), Some(0));
    assert_eq!(recipe.qa_index(), 1);
    let moved = recipe.clone();
    state.save_to(&folder.0.join("state.json")).unwrap();
    let loaded = folder.load();
    assert!(loaded.recovery.is_none());
    assert_eq!(loaded.state.pipelines.templates[0], moved);
}

/// Drops every `field` key, wherever it nests.
fn without(value: &Value, field: &str) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(name, _)| *name != field)
                .map(|(name, inner)| (name.clone(), without(inner, field)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| without(v, field)).collect()),
        other => other.clone(),
    }
}

#[test]
fn saved_prompt_catalog_and_its_template_links_survive_the_new_defaults() {
    for (release, _, file) in &SAMPLES[5..] {
        let written = json(file);
        let loaded = as_json(&Folder::holding(release, file).load().state);
        for field in ["prompts", "templates", "runs"] {
            // A lane's `edits` is read into its profile's mode and no longer written.
            assert_eq!(
                loaded["pipelines"][field],
                without(&written["pipelines"][field], "edits"),
                "{release}: {field}"
            );
        }
    }
}

#[test]
fn a_lane_that_accepted_edits_launches_in_that_mode_after_the_update() {
    for (release, _, file) in &SAMPLES[3..] {
        let mut written = json(file);
        // The third template has two lanes; only the first accepted edits.
        let template = &mut written["pipelines"]["templates"][2]["lanes"][0];
        template["edits"] = "accept_edits".into();
        let run = &mut written["pipelines"]["runs"][0]["lanes"][0];
        run["edits"] = "accept_edits".into();
        let loaded = Folder::holding(release, &written.to_string()).load();
        assert!(loaded.recovery.is_none(), "{release}");
        let pipelines = &loaded.state.pipelines;
        let accepted = Some(crate::pipeline::PermissionMode::AcceptEdits);
        let lanes = &pipelines.templates[2].lanes;
        assert_eq!(lanes[0].profile.mode, accepted, "{release}");
        assert_eq!(
            pipelines.runs[0].lanes()[0].profile.mode,
            accepted,
            "{release}"
        );
        // The lanes that kept asking still follow their role.
        assert_eq!(lanes[1].profile.mode, None, "{release}");
        assert_eq!(
            pipelines.templates[2].planner.profile.mode, None,
            "{release}"
        );
    }
}

#[test]
fn a_released_file_saved_by_this_version_reads_back_the_same() {
    for (release, _, file) in SAMPLES {
        let folder = Folder::holding(release, file);
        let first = folder.load().state;
        first.save_to(&folder.0.join("state.json")).unwrap();
        let saved = json(&fs::read_to_string(folder.0.join("state.json")).unwrap());
        assert_eq!(saved["version"], SCHEMA_VERSION, "{release}");
        let second = folder.load();
        assert!(second.recovery.is_none(), "{release}");
        assert_eq!(as_json(&second.state), as_json(&first), "{release}");
    }
}

/// Adds a field no version knows to the object at `path`, and to each element when it is a
/// list.
fn add_unknown_field(file: &mut Value, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        match file {
            Value::Array(items) => items
                .iter_mut()
                .for_each(|item| add_unknown_field(item, &[])),
            Value::Object(object) => {
                object.insert(
                    "from_the_future".into(),
                    serde_json::json!({"anything": [1, "x"]}),
                );
            }
            _ => {}
        }
        return;
    };
    match file {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| add_unknown_field(item, path)),
        Value::Object(object) => {
            if let Some(inner) = object.get_mut(*first) {
                add_unknown_field(inner, rest);
            }
        }
        _ => {}
    }
}

#[test]
fn fields_a_later_version_adds_to_a_released_file_do_not_cost_anything() {
    let places: [&[&str]; 14] = [
        &[],
        &["settings"],
        &["settings", "files"],
        &["settings", "commit_message"],
        &["projects"],
        &["projects", "identity"],
        &["workspace"],
        &["workspace", "projects"],
        &["workspace", "selection"],
        &["pipelines"],
        &["pipelines", "templates"],
        &["pipelines", "prompts"],
        &["pipelines", "runs"],
        &["pipelines", "runs", "lanes"],
    ];
    for (release, _, file) in SAMPLES {
        let expected = as_json(&Folder::holding(release, file).load().state);
        let mut extended = json(file);
        for place in places {
            add_unknown_field(&mut extended, place);
        }
        assert!(
            extended.to_string().matches("from_the_future").count() > 20,
            "{release}"
        );
        let loaded = Folder::holding(release, &extended.to_string()).load();
        assert!(
            loaded.recovery.is_none(),
            "{release}: {:?}",
            loaded.recovery.map(|recovery| recovery.cause)
        );
        assert_eq!(as_json(&loaded.state), expected, "{release}");
    }
}

#[test]
fn a_released_file_with_a_section_missing_loads_the_rest() {
    for (release, _, file) in SAMPLES {
        for section in ["settings", "projects", "workspace", "pipelines", "version"] {
            let mut trimmed = json(file);
            trimmed.as_object_mut().unwrap().remove(section);
            let loaded = Folder::holding(release, &trimmed.to_string()).load();
            assert!(loaded.recovery.is_none(), "{release} without {section}");
            let projects = if section == "projects" { 0 } else { 9 };
            assert_eq!(
                loaded.state.projects.len(),
                projects,
                "{release} without {section}"
            );
        }
    }
}
