# Hive performance audit, 2026-11-04

Interpretation and limitations: `wiki/performance-audit-2026-11-04.md`.

- `manifest.json`, `summary.json`, `git.csv`, compressed timing/resources and process samples:
  original release QA baseline. The manifest identifies the dirty source snapshot.
- `git-fixed.csv`: one warm-up and three runs on the same 100/1000-file fixtures.
- `signal-before.txt`: isolated regression test fails on the original SIGCHLD integration.
- `tests-fixed.txt`, `clippy-fixed.txt`, `core-tree-fixed.txt`, `release-bundle.txt`: verification.
- `fixed-manifest.json`: source hashes of the instrumented native QA snapshot.
- `fixed-summary.json`, `fixed-timing-primary.csv.gz`: valid 30-second visible/hidden phases.
- `fixed-ui-checks.csv.gz`: final native scroll/collapse/expand checks, all passed.
- `instrumentation/fixed-qa.patch`: temporary profiling, separate state, disabled usage,
  deterministic stream, 1000-file selection, visible/hidden phases and list assertions.
  It is for a disposable checkout only, never the installed application.

Fixture and stream generators are in `instrumentation/`. Their `/tmp/hive-perf-audit`
paths are intentional. Run `fixtures.py` once, then the `git-benchmark.rs` example in
an isolated copy of `hive-core`; it validates counts as well as recording durations.
The original repo-0 fixture later gained a large diff and history, so the fixed Git
benchmark covers 100 and 1000 only.

For native QA, apply the patch with `git apply --unidiff-zero` to a disposable copy
matching the `patch_base` hashes in the manifest (other concurrent workspace edits
are intentionally absent from the measured snapshot), then build
`cargo build --release -p hive-perf-fixed` with the temporary `gpui-pre` profiler feature,
and package it with a unique
bundle ID (`dev.hive.perf-fixed`). Its own state is `/tmp/hive-perf-fixed/state`, with
the synthetic projects registered. `main.rs` checks the 1000-file target, runs a
terminal stream, measures visible and hidden Git, then checks scrolling to the end
and collapsing/reopening Untracked before quitting. Logs/timings remain under `/tmp`.

The patch renames both workspace packages to `hive-perf-fixed` and `hive-core-perf-fixed`:
identical package IDs in a shared target directory can reuse another checkout's core
artifact as well as replace the `target/release/hive` convenience path. Use the separate
QA executable and verify both embedded state-directory and phase markers, or use a
separate target dir. Production package names remain unchanged.
An early fixed run selected `/tmp` as a worktree instead of Git's canonical
`/private/tmp` path, failed its list assertion, and was excluded from results. Another
run linked a production core artifact and failed the target guard before measurement;
it was also excluded. The 30-second primary phases used the correct QA state path.

The native UI tool disconnected during follow-up. macOS denied the fallback screen/
Accessibility APIs; the final native checks are internal assertions and frame data,
not a manual visual approval or a complete input-latency study. Timing percentiles
exclude two seconds at each phase boundary, since GPUI histograms flush once a second.
The final diagnostic script uses shorter phases to check list transitions; its frame
percentiles are not substituted for the primary 30-second measurements. A follow-up
which stopped producing frames after hide was excluded from timed layout assertions.
