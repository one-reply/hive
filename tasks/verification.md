# Editor verification — 2026-11-17

Scope and exclusions: [plan.md](plan.md). Base: develop
`c10f46a0ac32189258cfcd18932a5abddcf36ba3`. macOS Apple Silicon.

## Automated gates

All passed on the final implementation:

- `cargo fmt --all` and `git diff --check`.
- `cargo clippy --workspace --all-targets --offline -- -D warnings`.
- `cargo test --workspace --offline`: **292 app + 6 integration + 547 core = 845 passed**,
  3 existing opt-in tests ignored. Integration tests ran with permission for their
  local test sockets; no real agent or SSH endpoint was contacted.
- `cargo tree -p hive-core --offline`: no GPUI dependency.
- `cargo check -p hive-core --no-default-features --features remote-helper --bin hive-remote --offline`.

New coverage includes JS/TS/Rust declarations; fuzzy ordering, Unicode and large
file lists; recent-target isolation; bounded history and deleted paths; formatter
stdin, quoting, nonzero exit, invalid output, Rust edition and stale revision;
selection metadata; malformed/truncated/oversized LSP frames, UTF-16 surrogate
boundaries and reversed ranges, canonical URI rebasing, request cancellation,
shutdown and one restart; capability/fallback and stale diagnostic decisions;
bounded diagnostics; exact diff/source/host fingerprints, changes outside visible
hunks and previous state/config compatibility; excerpt merge/order/limits and row
geometry/cache behavior; sticky display-row selection.

## Native verification

Used a disposable project, profile and `Hive Editor QA.app` with a launcher that
sets `HIVE_DEV_DATA_DIR`. Its source fixtures and Python stdio test peer remained
outside the repository. No real agent message was sent.

Observed successfully:

- Relative breadcrumbs remain readable; symbol picker jumps; fuzzy Files results.
- Real local rustfmt changes the buffer; one Undo restores the original and clean state.
- Exact Unicode selection/path/line metadata in editable agent draft; Cancel.
- LSP test-peer startup/retry, diagnostics, multi-file references and Problems.
- Peek displays the definition, rejects pasted edits, opens a normal editor,
  Back returns to the origin, and Escape closes it from the origin editor.
- Two nested Sticky Scroll scopes, readable line numbers/indentation after scroll.
- References/Problems/local content results render grouped context; keyboard
  selection opens the appropriate editor. Excerpt rows have accessibility labels.
- Reviewed survives relaunch, clears after an external content edit plus refresh,
  and persists again after the new revision is acknowledged.

## Release microbenchmarks

An uncommitted temporary Rust example compared the base search module from
`git show c10f46a0:crates/hive-core/src/files/search.rs` and current code in the same
release executable. Five warmups, 50 iterations, `std::hint::black_box`; mean elapsed
time, not a statistical latency percentile. The temporary example was removed.

| Workload | Base | Current |
| --- | ---: | ---: |
| 20k paths, broad `component1` query | 17.75 ms | 27.10 ms |
| 20k paths, fuzzy `cpt19` query | 0.63 ms (no matches) | 11.85 ms |
| 20k paths, absent `no-match-xyz` query | 0.64 ms | 6.88 ms |
| 1k declarations, completion index / completion + outline | 3.53 ms | 5.76 ms |
| Group 5k excerpt hits across 500 files | — | 3.82 ms |

Paths were `src/feature{i % 100}/component{i}.tsx`, `i = 0..20000`. The declaration
fixture used 1,000 exported TypeScript functions; parsing happened before timing
both index builders. Excerpt paths were `/work/file{i % 500}.ts` with line `i`.
This measures CPU work only. Fuzzy ranking adds cost, especially to absent queries;
search and declaration parsing/indexing run in the background. The implementation
uses one row/score map, avoiding the duplicate-map cost observed during development.
These figures do **not** establish release UI frame rate or typing latency.

## Limits and follow-up checks

- No configured SSH fixture. Local/SSH use the shared Host command path, but real
  remote formatting, login-shell discovery and remote process cleanup need a live smoke.
- Prettier and typescript-language-server were absent. rust-analyzer was only a
  rustup shim without an installed binary. No tools were installed. Fake peers
  verify protocol mechanics; they cannot prove every real server behavior.
- TS server discovery checks worktree-root/ancestor `node_modules/.bin`, then PATH.
  A server installed only in a nested package must also be available from that lookup.
- Diagnostics without a server-supplied version cannot be proven fresh against an
  earlier server computation; supplied versions and current synced text are checked.
- Excerpts are readonly snapshots. Known document/Git changes mark them stale;
  Refresh is required for external changes that produce no observed notification.
  References Refresh uses the origin editor's current cursor.
- Back/Forward removes known deleted paths; an externally deleted closed file can
  open the existing read-error state. Alias paths restored as different Targets can
  still yield duplicate tabs; LSP URI rebasing handles aliases within one target.
- Native smoke covered normal nested scrolling. Exhaustive wrap/fold/resize and
  release frame-time stress remain unmeasured. No signed release was produced.

Refactoring, auto-import, quick fixes, format-on-save, remote content search,
editable multibuffers and automatic installations remain outside the agreed scope.
