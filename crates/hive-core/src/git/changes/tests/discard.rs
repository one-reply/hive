use super::*;

fn discard(repo: &Repo, kind: Discard, paths: &[&str]) -> Result<Option<Synced>, GitError> {
    repo.action(Action::Discard {
        kind,
        paths: paths.iter().map(|path| (*path).into()).collect(),
    })
}

#[test]
fn bulk_actions_only_touch_the_confirmed_paths_and_preserve_the_index() {
    let repo = Repo::new();
    repo.write("partial", "base");
    repo.write("missing", "restore me");
    repo.write(".gitignore", "*.ignored\n");
    repo.commit();
    repo.write("partial", "staged");
    repo.git(&["add", "partial"]);
    repo.write("partial", "unstaged");
    std::fs::remove_file(repo.0.join("missing")).unwrap();
    let names = ["keep", "keep.ignored", ":(glob)*", "-new\nfile"];
    for path in names {
        repo.write(path, "new");
    }
    discard(&repo, Discard::Restore, &["partial", "missing"]).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.0.join("partial")).unwrap(),
        "staged"
    );
    assert_eq!(repo.git(&["show", ":partial"]), "staged");
    assert_eq!(
        std::fs::read_to_string(repo.0.join("missing")).unwrap(),
        "restore me"
    );
    assert!(names.iter().all(|path| repo.0.join(path).exists()));

    discard(&repo, Discard::Delete, &names[2..]).unwrap();
    assert!(names[..2].iter().all(|path| repo.0.join(path).exists()));
    assert!(names[2..].iter().all(|path| !repo.0.join(path).exists()));
}

#[test]
fn a_changed_group_aborts_the_whole_selection_before_mutation() {
    let repo = Repo::new();
    repo.write("tracked", "base");
    repo.commit();
    repo.write("tracked", "edit");
    repo.write("untracked", "new");
    assert!(discard(&repo, Discard::Restore, &["tracked", "untracked"]).is_err());
    assert!(discard(&repo, Discard::Delete, &["untracked", "tracked"]).is_err());
    assert_eq!(
        std::fs::read_to_string(repo.0.join("tracked")).unwrap(),
        "edit"
    );
    assert!(repo.0.join("untracked").exists());
}

#[test]
fn restore_refuses_to_overwrite_ignored_files_blocking_checkout() {
    let repo = Repo::new();
    repo.write("entry", "tracked");
    repo.write(".gitignore", "*.ignored\n");
    repo.commit();
    std::fs::remove_file(repo.0.join("entry")).unwrap();
    std::fs::create_dir(repo.0.join("entry")).unwrap();
    repo.write("entry/new.ignored", "keep");
    assert!(discard(&repo, Discard::Restore, &["entry"]).is_err());
    assert!(repo.0.join("entry/new.ignored").exists());
}

#[test]
fn clean_does_not_descend_into_a_replacement_directory() {
    let repo = Repo::new();
    std::fs::create_dir(repo.0.join("new")).unwrap();
    repo.write("new/arrived", "keep");
    repo.write("other", "delete");
    super::super::discard::delete_untracked(&Host::Local, &repo.0, &["new".into(), "other".into()])
        .unwrap();
    assert!(repo.0.join("new/arrived").exists());
    assert!(!repo.0.join("other").exists());
}
