use super::*;
use crate::{commit_message::PROMPT_LIMIT, git::commit_context::Context};

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
    assert!(context.patch_within(usize::MAX).contains("staged text"));
    assert!(!context.patch_within(usize::MAX).contains("private"));
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
    assert!(context.patch_within(usize::MAX).contains("Binary files"));
    assert!(
        !context
            .patch_within(usize::MAX)
            .contains("secret-binary-payload")
    );
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
fn oversized_file_patch_is_omitted_by_name_and_smaller_patches_stay_complete() {
    let repo = Repo::new();
    repo.write("small.rs", "fn small() {}\n");
    repo.write("generated.lock", &"x\n".repeat(PROMPT_LIMIT));
    repo.git(&["add", "-A"]);
    let context = Context::read(&Host::Local, &repo.0).unwrap();
    let complete = context.patch_within(usize::MAX);
    assert!(complete.len() > PROMPT_LIMIT);
    assert!(complete.starts_with("diff --git a/generated.lock b/generated.lock\n"));
    let patch = context.patch_within(PROMPT_LIMIT);
    assert!(patch.len() <= PROMPT_LIMIT);
    assert!(patch.contains(" generated.lock | "));
    assert!(patch.contains("2 files changed"));
    assert!(patch.contains("\na/generated.lock b/generated.lock\n"));
    assert!(!patch.contains("+x\n"));
    assert!(patch.ends_with("+++ b/small.rs\n@@ -0,0 +1 @@\n+fn small() {}\n"));
    assert!(context.is_current(&Host::Local, &repo.0).unwrap());
}

#[test]
fn patch_beyond_the_read_limit_keeps_only_its_diffstat() {
    let repo = Repo::new();
    repo.write("small.rs", "fn small() {}\n");
    repo.write("huge.txt", &"x\n".repeat(crate::git::READ_LIMIT / 2));
    repo.git(&["add", "-A"]);
    let context = Context::read(&Host::Local, &repo.0).unwrap();
    let patch = context.patch_within(usize::MAX);
    assert!(patch.contains(" huge.txt | "));
    assert!(patch.contains(" small.rs | "));
    assert!(patch.contains("2 files changed"));
    assert!(!patch.contains("diff --git"));
}
