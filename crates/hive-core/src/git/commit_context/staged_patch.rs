//! Fits a staged diff into a prompt budget by dropping whole file patches, never part of one.
use std::borrow::Cow;

const FILE_HEADER: &str = "diff --git ";
// Labels only state facts: prompts treat the patch as untrusted data and ignore instructions in it.
const SUMMARY: &str =
    "The staged patch exceeds the prompt size limit. Diffstat of all staged changes:";
const OMITTED: &str = "Patches omitted for size:";
const INCLUDED: &str = "Remaining patches:";

/// The staged diff and its diffstat. The patch is absent when Git printed more than Hive reads.
pub(super) struct StagedPatch {
    stat: String,
    patch: Option<String>,
}

impl StagedPatch {
    /// Splits `git diff --stat --patch` output: the diffstat, a blank line, then the file patches.
    pub(super) fn from_stat_and_patch(mut output: String) -> Self {
        let start = if output.starts_with(FILE_HEADER) {
            0
        } else {
            // Every diffstat line is indented, so the first unindented header starts the patch.
            output
                .find(&format!("\n{FILE_HEADER}"))
                .map_or(output.len(), |newline| newline + 1)
        };
        let patch = output.split_off(start);
        Self {
            stat: trimmed(output),
            patch: Some(patch),
        }
    }

    /// A diff too large to read; only `git diff --stat` output is known.
    pub(super) fn from_stat(stat: String) -> Self {
        Self {
            stat: trimmed(stat),
            patch: None,
        }
    }

    /// The exact patch when it fits `budget` bytes; otherwise the diffstat, the files whose
    /// patches are left out, and every complete file patch that still fits. A diffstat that
    /// alone exceeds the budget is returned as is, for the caller's size check to reject.
    pub(super) fn within(&self, budget: usize) -> Cow<'_, str> {
        match &self.patch {
            Some(patch) if patch.len() <= budget => Cow::Borrowed(patch),
            Some(patch) => Cow::Owned(self.summarized(&files(patch), budget)),
            None => {
                tracing::info!("commit prompt carries only the diffstat of an unread staged patch");
                Cow::Owned(self.overview())
            }
        }
    }

    fn overview(&self) -> String {
        format!("{SUMMARY}\n{}\n", self.stat)
    }

    fn summarized(&self, files: &[&str], budget: usize) -> String {
        let included = fitting(files, budget.saturating_sub(self.frame_len()));
        let kept = included.iter().filter(|kept| **kept).count();
        tracing::info!(
            files = files.len(),
            omitted = files.len() - kept,
            "commit prompt omits staged patches that exceed its budget"
        );
        let mut output = self.overview();
        if kept < files.len() {
            output.push_str(&format!("\n{OMITTED}\n"));
            for (file, _) in files.iter().zip(&included).filter(|(_, kept)| !**kept) {
                output.push_str(label(file));
                output.push('\n');
            }
        }
        if kept > 0 {
            output.push_str(&format!("\n{INCLUDED}\n"));
            for (file, _) in files.iter().zip(&included).filter(|(_, kept)| **kept) {
                output.push_str(file);
            }
        }
        output
    }

    /// Bytes `summarized` writes besides file labels and patches, with both headings present.
    fn frame_len(&self) -> usize {
        // One newline ends the summary and the diffstat; each heading sits between two.
        const NEWLINES: usize = 6;
        SUMMARY.len() + self.stat.len() + OMITTED.len() + INCLUDED.len() + NEWLINES
    }
}

fn trimmed(mut stat: String) -> String {
    stat.truncate(stat.trim_end_matches('\n').len());
    stat
}

/// One slice per file, each starting at its `diff --git` header and ending before the next.
fn files(patch: &str) -> Vec<&str> {
    let mut starts = vec![0];
    starts.extend(
        patch
            .match_indices(&format!("\n{FILE_HEADER}"))
            .map(|(newline, _)| newline + 1),
    );
    starts.push(patch.len());
    starts
        .windows(2)
        .map(|bounds| &patch[bounds[0]..bounds[1]])
        .filter(|file| !file.is_empty())
        .collect()
}

/// The paths of a file's header line, as Git quoted them.
fn label(file: &str) -> &str {
    let header = file.lines().next().unwrap_or_default();
    header.strip_prefix(FILE_HEADER).unwrap_or(header)
}

/// Marks the files to keep, smallest first so that the most files stay complete. Every file
/// starts out omitted, costing its label line; keeping one replaces that line with its patch.
fn fitting(files: &[&str], budget: usize) -> Vec<bool> {
    let label_len = |file: &str| label(file).len() + 1;
    let mut used: usize = files.iter().map(|file| label_len(file)).sum();
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by_key(|&ix| files[ix].len());
    let mut included = vec![false; files.len()];
    for ix in order {
        let swapped = used - label_len(files[ix]) + files[ix].len();
        if swapped <= budget {
            used = swapped;
            included[ix] = true;
        }
    }
    included
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `git diff --cached --no-renames --stat=1000 --stat-graph-width=10 --patch`
    /// (Git 2.54): binary, deleted, mode-changed and quoted paths.
    const CAPTURED: &str = include_str!("../fixtures/diff-cached-stat-patch.txt");

    fn file(path: &str, lines: usize) -> String {
        format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{}",
            format!("+{path}\n").repeat(lines)
        )
    }

    #[test]
    fn captured_output_splits_into_diffstat_and_one_patch_per_file() {
        let staged = StagedPatch::from_stat_and_patch(CAPTURED.into());
        assert!(staged.stat.starts_with(" binary "));
        assert!(
            staged
                .stat
                .ends_with("6 files changed, 4 insertions(+), 3 deletions(-)")
        );
        let patch = staged.patch.as_deref().unwrap();
        assert!(patch.starts_with("diff --git a/binary b/binary\n"));
        assert_eq!(format!("{}\n\n{patch}", staged.stat), CAPTURED);
        let files = files(patch);
        assert_eq!(files.concat(), patch);
        let labels: Vec<_> = files.iter().map(|file| label(file)).collect();
        assert_eq!(
            labels,
            [
                "a/binary b/binary",
                "a/deep/dir/space name.rs b/deep/dir/space name.rs",
                "a/delete.txt b/delete.txt",
                "a/keep.txt b/keep.txt",
                r#""a/new\n\321\226\320\274\342\200\231\321\217" "b/new\n\321\226\320\274\342\200\231\321\217""#,
                "a/old name b/old name",
            ]
        );
    }

    #[test]
    fn patch_within_the_budget_is_returned_unchanged() {
        let staged = StagedPatch::from_stat_and_patch(CAPTURED.into());
        let patch = staged.patch.clone().unwrap();
        assert_eq!(staged.within(patch.len()), patch);
        assert!(matches!(staged.within(usize::MAX), Cow::Borrowed(_)));
    }

    #[test]
    fn oversized_file_is_named_and_smaller_patches_stay_complete() {
        let (small, large, other) = (file("small", 2), file("large", 400), file("other", 3));
        let staged =
            StagedPatch::from_stat_and_patch(format!(" stat line\n\n{small}{large}{other}"));
        let budget = large.len();
        let output = staged.within(budget);
        assert!(output.len() <= budget);
        assert_eq!(
            output,
            format!(
                "{SUMMARY}\n stat line\n\n{OMITTED}\na/large b/large\n\n{INCLUDED}\n{small}{other}"
            )
        );
    }

    #[test]
    fn more_small_files_are_kept_instead_of_one_larger_file() {
        let patches = [file("b", 150), file("a", 40), file("c", 41)];
        let staged = StagedPatch::from_stat_and_patch(format!(" stat\n\n{}", patches.concat()));
        // Exactly enough for the largest file alone, which the two smaller ones use instead.
        let budget = staged.frame_len() + patches[0].len() + 2 * "a/a b/a\n".len();
        assert!(patches.concat().len() > budget);
        let output = staged.within(budget);
        assert!(output.len() <= budget);
        assert!(output.contains(&format!("{OMITTED}\na/b b/b\n")));
        assert!(output.ends_with(&format!("{INCLUDED}\n{}{}", patches[1], patches[2])));
    }

    #[test]
    fn budget_without_room_for_any_patch_lists_every_file_as_omitted() {
        let staged = StagedPatch::from_stat_and_patch(CAPTURED.into());
        let labels: usize = files(staged.patch.as_deref().unwrap())
            .iter()
            .map(|file| label(file).len() + 1)
            .sum();
        let budget = staged.frame_len() + labels;
        let output = staged.within(budget);
        assert!(output.len() <= budget);
        assert!(output.starts_with(&format!("{SUMMARY}\n binary ")));
        assert!(output.contains(&format!("{OMITTED}\na/binary b/binary\n")));
        assert!(output.ends_with("a/old name b/old name\n"));
        assert!(!output.contains(INCLUDED));
        assert!(!output.contains(FILE_HEADER));
    }

    #[test]
    fn unread_patch_yields_only_the_diffstat() {
        let staged = StagedPatch::from_stat(" huge | 9 +++\n 1 file changed\n".into());
        assert_eq!(
            staged.within(usize::MAX),
            format!("{SUMMARY}\n huge | 9 +++\n 1 file changed\n")
        );
    }

    #[test]
    fn diffstat_larger_than_the_budget_is_left_for_the_caller_to_reject() {
        let staged = StagedPatch::from_stat_and_patch(CAPTURED.into());
        assert!(staged.within(10).len() > 10);
    }

    #[test]
    fn empty_and_headerless_output_do_not_panic() {
        assert_eq!(
            StagedPatch::from_stat_and_patch(String::new()).within(0),
            ""
        );
        let staged = StagedPatch::from_stat_and_patch(" stat only\n".into());
        assert_eq!(staged.within(usize::MAX), "");
        assert_eq!(files("no header"), ["no header"]);
        assert_eq!(fitting(&["no header"], 0), [false]);
    }
}
