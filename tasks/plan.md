# Editor navigation and language tools

Approved 2026-11-16. Goal: manual coding and agent review equally well supported.
Keep the existing GPUI editor, document ownership, recovery and atomic saves.
JS/TS/JSX/TSX and Rust; language servers and formatting on Local and SSH; use
installed tools only. Implementation checklist: [todo.md](todo.md).

## 1. Navigation
Fix blame geometry and stale fade placement. Replace the duplicated absolute path
with worktree + clickable breadcrumbs + current symbol. Share background parsing
between completion and document symbols; Cmd-Shift-O navigates symbols. Keep a
bounded in-memory back/forward history per project center. Improve inline Cmd-P
with deterministic fuzzy ranking, tree order and recent files.

## 2. Formatting and agent context
Explicit Format Document (Alt-Shift-F), Prettier/rustfmt on the document's host,
stdin/stdout only, project configuration and Rust edition, bounded/cancellable
execution. Apply only to the captured document revision as a single undo step.
Selection context draft: path, host/worktree, lines, exact buffer and user note;
existing safe local draft delivery, Copy & go on SSH, never submit.

## 3. Language services and reviewed revisions
One lazy stdio session per host/worktree/language family, Local and SSH. Typed
LSP messages, capabilities, synchronization, UTF-16 positions, cancellation,
bounded framing, shutdown and one recovery attempt. Completion, hover,
definitions, references and Problems. Preserve local fallback; omit completion
items requiring unsupported edits. Readonly external definitions.
Reviewed refers to the exact complete diff including full object IDs and source;
persist compact fingerprints via Store, revalidate after refresh/restart, never
treat missing, partial or binary previews as reviewed.

## 4. Context views
Peek Definition (Alt-F12), up to two sticky declaration lines with display-map
geometry, and virtualized readonly excerpts for references/diagnostics/existing
content search. Open an excerpt in the existing editor. Remote content search,
editable multibuffers, refactoring, auto-import, quick fixes, format-on-save and
tool installation are excluded.

## Verification
Hermetic core/state tests for every decision and failure path, compatibility
fixtures for persisted additions, fake language-server tests, native UI smoke,
release performance comparison. Run fmt, workspace/all-target clippy with
warnings denied, workspace tests and verify hive-core has no GPUI. Update the
Ukrainian wiki and ADRs with implementation and limitations. Branches/PRs target
develop; no direct changes to main or develop.
