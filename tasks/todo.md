# Approved editor implementation

Scope: [plan.md](plan.md). Evidence and remaining environment checks:
[verification.md](verification.md). Checked items mean the stated checks passed,
not that every platform/server combination has been exercised.

- [x] 1.1 Blame viewport/fade guards, geometry tests and native smoke.
- [x] 1.2 Relative breadcrumbs, folder reveal and symbol navigation; native smoke.
- [x] 1.3 JS/TS/Rust symbols from the shared background parse, unsaved provenance tests.
- [x] 1.4 Bounded project Back/Forward, ordering and known-deleted-path tests; native Back.
- [x] 1.5 Fuzzy ranking, Unicode offsets, recents isolation and 20k-file test/measurement.
- [x] Checkpoint 1: navigation native flows and workspace gates.
- [x] 2.1 Local/SSH formatter commands, configuration/edition, single Undo and stale guards; fake process/state tests and real local rustfmt smoke.
- [x] 2.2 Exact selection draft and target metadata, existing no-submit delivery; native draft/cancel.
- [x] Checkpoint 2: formatting/draft native smoke and workspace gates.
- [x] 3.1 Local/SSH LSP lifecycle, bounded protocol, cancellation/shutdown/recovery; fake-server tests.
- [x] 3.2 Completion/hover/definitions/references/Problems, Unicode/version/fallback tests and test-peer native flows.
- [x] 3.3 Exact Reviewed revision and compatible persistence; mutation/restart tests and native checks.
- [x] Checkpoint 3: automated protocol/state checks and workspace gates.
- [x] 4.1 Readonly Peek, Escape/Open/history; native checks.
- [x] 4.2 Two sticky scopes, display-map geometry and compatible setting; pure tests and native nested-scope scroll.
- [x] 4.3 Virtual excerpts, bounded lazy reads, refresh/stale status; ordering/cache tests and native references/Problems/search.
- [x] Self-review, release microbenchmarks, wiki and final fmt/clippy/tests/dependency gates.
- [ ] Real installed TypeScript server / rust-analyzer and live SSH formatter/LSP smoke (unavailable environment; no installation authorized).
- [ ] Exhaustive native wrap/fold/resize stress and release UI frame-time measurement (beyond the completed smoke/microbenchmarks).
- [x] Commit `60ccdba` and [draft PR #92](https://github.com/one-reply/hive/pull/92) against develop.
