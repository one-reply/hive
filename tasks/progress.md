# Editor implementation — 2026-11-17

All four implementation blocks in [plan.md](plan.md) are complete on
`editor-navigation-and-language-tools`, based on `develop` at
`c10f46a0ac32189258cfcd18932a5abddcf36ba3`.

- Navigation: viewport-safe blame, relative breadcrumbs, live symbols, bounded
  Back/Forward history, fuzzy file ranking and per-worktree recents.
- Editing: explicit installed Prettier/rustfmt through Local/SSH Host, revision
  guards and one Undo; editable selection context draft with no automatic submit.
- Language tools: bounded stdio lifecycle, capabilities, UTF-16 conversion,
  completion/hover/definitions/references/Problems and local fallback. Reviewed
  fingerprints track the complete diff and survive restart.
- Context: readonly Peek, two sticky scopes and virtualized excerpts for
  references, Problems and existing local content search.

Workspace checks passed: fmt, all-target clippy with warnings denied, 845 tests
(3 opt-in ignored), no GPUI in core, and the remote-helper feature build.
Native checks and release measurements are recorded in [verification.md](verification.md).

Real SSH and real TypeScript/rust-analyzer integration remain unverified here:
there is no configured SSH fixture or installed language-server binary. Nothing
was installed. The native LSP flows used a disposable stdio test peer. The PR
remains a draft pending those environment checks; there are no implementation
placeholders or automatic tool-install paths.
