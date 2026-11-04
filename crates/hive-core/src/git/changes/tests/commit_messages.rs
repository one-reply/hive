use super::*;
use crate::git::commit_context::Context;

#[test]
fn staged_context_excludes_unstaged_and_untracked_and_detects_new_index() {
    let repo = Repo::new();
    repo.write("file", "original\n");
    repo.commit();
    repo.write("file", "staged text\n");
    repo.git(&["add", "file"]);
    repo.write("file", "unstaged private text\n");
    repo.write("untracked", "untracked private text");
    let context = Context::read(&Host::Local, &repo.0).unwrap();
    assert!(context.patch().contains("staged text"));
    assert!(!context.patch().contains("private"));
    assert!(!context.files().contains("untracked"));
    repo.write("file", "another working edit");
    assert!(context.is_current(&Host::Local, &repo.0).unwrap());
    repo.git(&["add", "file"]);
    assert!(!context.is_current(&Host::Local, &repo.0).unwrap());
}

#[test]
fn binary_renames_deletions_and_unusual_paths_have_context_without_binary_payload() {
    let repo = Repo::new();
    repo.write("old name", "old\n");
    repo.write("delete", "gone\n");
    repo.commit();
    repo.git(&["mv", "old name", "new\nім’я"]);
    repo.git(&["rm", "delete"]);
    std::fs::write(repo.0.join("binary"), b"\0secret-binary-payload").unwrap();
    repo.git(&["add", "binary"]);
    let context = Context::read(&Host::Local, &repo.0).unwrap();
    assert!(context.files().contains("Renamed"));
    assert!(context.files().contains("Deleted"));
    assert!(context.files().contains("new\\nім’я"));
    assert!(context.patch().contains("Binary files"));
    assert!(!context.patch().contains("secret-binary-payload"));
    std::fs::write(repo.0.join("binary"), b"\0changed-binary-payload").unwrap();
    repo.git(&["add", "binary"]);
    assert!(!context.is_current(&Host::Local, &repo.0).unwrap());
}

#[test]
fn unborn_detached_empty_conflicted_and_changed_head_are_handled() {
    let repo = Repo::new();
    assert!(Context::read(&Host::Local, &repo.0).is_err());
    repo.write("file", "initial\n");
    repo.git(&["add", "file"]);
    let context = Context::read(&Host::Local, &repo.0).unwrap();
    assert_eq!(context.branch(), "main");
    repo.git(&["commit", "-m", "first"]);
    assert!(!context.is_current(&Host::Local, &repo.0).unwrap());
    repo.git(&["checkout", "--detach"]);
    repo.write("file", "detached edit\n");
    repo.git(&["add", "file"]);
    assert_eq!(
        Context::read(&Host::Local, &repo.0).unwrap().branch(),
        "detached HEAD"
    );
    repo.git(&["reset", "--hard"]);
    repo.git(&["checkout", "main"]);
    repo.git(&["checkout", "-b", "other"]);
    repo.write("file", "other\n");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("file", "main\n");
    repo.commit();
    assert!(run(&Host::Local, &repo.0, &["merge", "other"]).is_err());
    assert!(Context::read(&Host::Local, &repo.0).is_err());
}

#[test]
fn oversized_patch_is_not_silently_truncated() {
    let repo = Repo::new();
    repo.write("large", &"x\n".repeat(crate::commit_message::PROMPT_LIMIT));
    repo.git(&["add", "large"]);
    assert!(matches!(
        Context::read(&Host::Local, &repo.0),
        Err(GitError::TooLarge(_))
    ));
}
