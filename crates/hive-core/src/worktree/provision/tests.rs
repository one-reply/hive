use super::*;

struct Scratch(PathBuf);

impl Scratch {
    /// A repository with one commit, and an empty folder standing in for the new worktree.
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hive-provision-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let scratch = Self(root.canonicalize().unwrap());
        fs::create_dir(scratch.source()).unwrap();
        fs::create_dir(scratch.checkout()).unwrap();
        git(&scratch.source(), &["init", "-q", "-b", "main"]);
        scratch
    }

    fn source(&self) -> PathBuf {
        self.0.join("source")
    }

    fn checkout(&self) -> PathBuf {
        self.0.join("checkout")
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.source().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn copied(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.checkout().join(path)).ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(path: &Path, args: &[&str]) {
    let output = Host::Local
        .command("git", args, Some(path))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn patterns_follow_gitignore_anchoring_wildcards_and_folder_markers() {
    let patterns = IncludePatterns::parse(
        "# local secrets\n\n.env*\n/config/local.yml\nnode_modules/\n**/keys/*.pem\nnotes?.txt\na+b.txt\n",
    );
    for (path, is_folder, expected) in [
        (".env", false, true),
        ("apps/web/.env.local", false, true),
        ("env", false, false),
        ("config/local.yml", false, true),
        ("apps/config/local.yml", false, false),
        ("node_modules", true, true),
        ("apps/web/node_modules", true, true),
        ("node_modules", false, false),
        ("keys/dev.pem", false, true),
        ("deep/er/keys/dev.pem", false, true),
        ("keys/nested/dev.pem", false, false),
        ("notes1.txt", false, true),
        ("notes12.txt", false, false),
        ("a+b.txt", false, true),
        ("aab.txt", false, false),
    ] {
        assert_eq!(patterns.matches(path, is_folder), expected, "{path}");
    }
}

#[test]
fn comments_blank_lines_and_negations_name_nothing() {
    assert!(IncludePatterns::parse("# only a comment\n\n   \n!.env\n/\n").is_empty());
    assert!(!IncludePatterns::parse(".env").is_empty());
}

#[test]
fn copies_only_ignored_entries_the_file_names_and_never_overwrites() {
    let scratch = Scratch::new("copy");
    scratch.write(".gitignore", ".env\ncache/\nsecret.txt\nbuild.log\nlink\n");
    scratch.write("tracked.txt", "tracked");
    git(&scratch.source(), &["add", ".gitignore", "tracked.txt"]);
    git(&scratch.source(), &["commit", "-qm", "base"]);
    scratch.write(".env", "TOKEN=1");
    scratch.write("cache/deep/blob", "cached");
    scratch.write("secret.txt", "from source");
    scratch.write("build.log", "ignored but not listed");
    scratch.write("draft.txt", "untracked, not ignored");
    std::os::unix::fs::symlink(".env", scratch.source().join("link")).unwrap();
    scratch.write(
        INCLUDE_FILE,
        ".env\ncache/\nsecret.txt\nlink\ntracked.txt\ndraft.txt\n",
    );
    fs::write(scratch.checkout().join("secret.txt"), "already here").unwrap();

    let copied = copy_included(
        &scratch.source(),
        &scratch.checkout(),
        &Cancellation::default(),
    )
    .unwrap();

    assert_eq!(copied, 4);
    assert_eq!(scratch.copied(".env").as_deref(), Some("TOKEN=1"));
    assert_eq!(scratch.copied("cache/deep/blob").as_deref(), Some("cached"));
    assert_eq!(
        scratch.copied("secret.txt").as_deref(),
        Some("already here")
    );
    assert_eq!(
        fs::read_link(scratch.checkout().join("link")).unwrap(),
        Path::new(".env")
    );
    for absent in ["build.log", "draft.txt", "tracked.txt"] {
        assert_eq!(scratch.copied(absent), None, "{absent}");
    }
    // A retry after an interruption finds everything in place.
    copy_included(
        &scratch.source(),
        &scratch.checkout(),
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(scratch.copied(".env").as_deref(), Some("TOKEN=1"));
}

#[test]
fn a_repository_without_the_file_or_with_an_empty_one_copies_nothing() {
    let scratch = Scratch::new("absent");
    scratch.write(".gitignore", ".env\n");
    scratch.write(".env", "TOKEN=1");
    let copy = || {
        copy_included(
            &scratch.source(),
            &scratch.checkout(),
            &Cancellation::default(),
        )
    };
    assert_eq!(copy().unwrap(), 0);
    scratch.write(INCLUDE_FILE, "# nothing yet\n");
    assert_eq!(copy().unwrap(), 0);
    assert_eq!(scratch.copied(".env"), None);
}

#[test]
fn a_stopped_copy_reports_that_it_was_stopped() {
    let scratch = Scratch::new("stopped");
    scratch.write(".gitignore", ".env\n");
    scratch.write(".env", "TOKEN=1");
    scratch.write(INCLUDE_FILE, ".env\n");
    let stop = Cancellation::default();
    stop.cancel();
    let error = copy_included(&scratch.source(), &scratch.checkout(), &stop).unwrap_err();
    assert!(matches!(error, ProvisionError::Stopped), "{error}");
    assert_eq!(scratch.copied(".env"), None);
}

#[test]
fn setup_runs_in_the_checkout_and_a_failure_carries_its_command_and_log() {
    let scratch = Scratch::new("setup");
    let run = |command: &str| run_setup(command, &scratch.checkout(), &Cancellation::default());
    run("   ").unwrap();
    run("echo ready > marker").unwrap();
    assert_eq!(scratch.copied("marker").as_deref(), Some("ready\n"));
    let error = run("echo installing; echo 'lockfile is stale' >&2; exit 3").unwrap_err();
    let ProvisionError::Setup {
        command,
        outcome,
        output,
    } = &error
    else {
        panic!("{error}");
    };
    assert_eq!(
        command,
        "echo installing; echo 'lockfile is stale' >&2; exit 3"
    );
    assert_eq!(outcome, "exited with 3");
    assert!(output.contains("installing") && output.contains("lockfile is stale"));
    assert!(error.to_string().contains("exited with 3"), "{error}");
}

#[test]
fn a_setup_that_never_finishes_reports_its_last_output() {
    let scratch = Scratch::new("hung-setup");
    let error = setup_within(
        "echo 'fetching packages'; echo 'waiting for the registry' >&2; exec sleep 600",
        &scratch.checkout(),
        &Cancellation::default(),
        Duration::from_secs(3),
    )
    .unwrap_err();
    let ProvisionError::Setup {
        outcome, output, ..
    } = &error
    else {
        panic!("{error}");
    };
    assert_eq!(outcome, "timed out");
    assert!(output.contains("fetching packages"), "{output}");
    assert!(output.contains("waiting for the registry"), "{output}");
}

#[test]
fn a_folder_is_named_by_a_pattern_for_its_contents_and_anchored_paths_look_inside() {
    let patterns = IncludePatterns::parse(
        "cache/**\nbuild/*\nconfig/local/*.yml\n.vscode/settings.json\n**/x\n",
    );
    assert!(patterns.matches("cache", true));
    assert!(patterns.matches("build", true));
    assert!(!patterns.matches("config", true));
    assert!(!patterns.matches("cache", false));
    assert_eq!(patterns.inside("config"), ["config/local"]);
    assert_eq!(patterns.inside(".vscode"), [".vscode/settings.json"]);
    // A wildcard first reaches everywhere, which would mean walking every ignored folder.
    assert!(patterns.inside("node_modules").is_empty());
}

#[test]
fn anchored_patterns_reach_files_inside_ignored_folders() {
    let scratch = Scratch::new("nested");
    scratch.write(".gitignore", ".vscode/\nconfig/\ncache/\nnode_modules/\n");
    scratch.write("tracked.txt", "tracked");
    git(&scratch.source(), &["add", ".gitignore", "tracked.txt"]);
    git(&scratch.source(), &["commit", "-qm", "base"]);
    scratch.write(".vscode/settings.json", "{}");
    scratch.write(".vscode/launch.json", "launch");
    scratch.write("config/local/dev.yml", "dev");
    scratch.write("config/local/dev.json", "json");
    scratch.write("config/other.yml", "other");
    scratch.write("cache/deep/blob", "cached");
    scratch.write("node_modules/pkg/settings.json", "dependency");
    scratch.write(
        INCLUDE_FILE,
        ".vscode/settings.json\nconfig/local/*.yml\ncache/**\n**/settings.json\n",
    );

    let copied = copy_included(
        &scratch.source(),
        &scratch.checkout(),
        &Cancellation::default(),
    )
    .unwrap();

    assert_eq!(copied, 3);
    assert_eq!(
        scratch.copied(".vscode/settings.json").as_deref(),
        Some("{}")
    );
    assert_eq!(
        scratch.copied("config/local/dev.yml").as_deref(),
        Some("dev")
    );
    assert_eq!(scratch.copied("cache/deep/blob").as_deref(), Some("cached"));
    for absent in [
        ".vscode/launch.json",
        "config/local/dev.json",
        "config/other.yml",
        "node_modules/pkg/settings.json",
    ] {
        assert_eq!(scratch.copied(absent), None, "{absent}");
    }
}
