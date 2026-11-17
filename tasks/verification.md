# Editor verification — 2026-11-17

Scope and exclusions: [plan.md](plan.md). Base: develop
`c10f46a0ac32189258cfcd18932a5abddcf36ba3`. macOS Apple Silicon.

## Automated gates

Initial feature checkpoint (before the review corrections below):

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

## Seven review corrections — 2026-11-17

All seven findings against `d815e10` were corrected before the requested merge.
No dependency was added.

| Finding | Correction | Regression evidence |
| --- | --- | --- |
| Completion corrupts member punctuation or duplicates the prefix | Every accepted item carries an explicit scalar-column edit; absent edits use the existing language word-boundary rules | Two real `EditorState::insert_completion` GPUI tests, including Unicode and a cursor inside a word; native `object.fo` → `object.foo`, `object.tot` → `object.total` |
| Full diagnostics queue kills a healthy session | Bounded event delivery waits for capacity, wakes when drained and observes cancellation while waiting | Deterministic pending/ready polling of a saturated queue, cancellation/closed receiver, 100-file fake-server burst followed by a successful response and graceful shutdown |
| LSP menu ignores typed prefix | Filter by filterText/label and the item's replacement prefix, then sort by sortText/label | Prefix/case/filterText/sortText tests, explicit zero-width insert versus replace tests; native menu excludes `alpha` while typing `tot` |
| Byte-path selection draft panics | Existing tagged lossless persisted-path serializer encodes both worktree and relative path | Non-UTF-8 SSH path round-trip without process/network access |
| Renamed document misses didOpen | `unlock_path` synchronizes after clearing path_busy | GPUI test holds recovery completion with another document on the ready session and no session events; removing the new sync call makes the test fail; native peer records didClose(old) then didOpen(new) |
| Alias root loses Rust edition | Resolve `pwd -P` on the document's host before matching Cargo manifest paths | Actual installed Cargo/rustfmt in an offline temporary edition-2021 package through canonical and symlink roots; async fn formats identically and disk source stays unchanged |
| History points to paths before rename/move | Successful Files move events remap both history stacks, including descendants, within the same Target | File/directory remapping and host/worktree isolation tests; native Back from the definition opens the renamed origin |

Final gates: fmt, workspace/all-target clippy `-D warnings`, **298 app + 6 integration
+ 552 core = 856 passed**, 3 existing opt-in tests ignored; no GPUI in core;
remote-helper feature check and debug build passed. The initial sandbox run blocked
existing local socket/process tests; the authorized full workspace run passed.
The temporary missing-sync mutation was restored before these gates.

Native checks used the existing disposable QA profile and a stdio peer, with no
real agent submission. Its wire log confirms the renamed URI opens without an
additional hover/completion/definition request. Real SSH and real TypeScript /
rust-analyzer checks remain unavailable; the earlier stress/performance limitations
still apply. The owner requested merge with those validation limits stated.

The completion adapter follows the [LSP completion contract](https://github.com/microsoft/language-server-protocol/blob/gh-pages/_specifications/lsp/3.18/language/completion.md)
and the installed GPUI insertion API; it does not add snippets or workspace edits.
