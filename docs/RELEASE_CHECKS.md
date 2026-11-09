# Release verification

## Hive 1.0.4 — November 9, 2026

The [release notes](releases/v1.0.4.md) describe the changes and verification limits.
The published release records the exact CI run, source revision, final archive
checksum, Developer ID, notarization, and downloaded-asset checks.

- Formatting, Clippy with warnings denied, 533 Rust tests (three existing opt-in
  tests ignored), 26 Python tests, and the core/GPUI boundary passed locally.
- cargo-deny passed licenses, advisories, and sources with the existing documented
  maintenance exceptions. Gitleaks found no leaks in the changed and new source files.
- Regression tests cover the agent notification setting's default and round trip,
  notification title and body, agent placement from captured process tables, and
  selection autoscroll through history and on alternate screens.

Not reverified: posting and clicking a macOS notification in the packaged app, the
default alert style on a fresh installation, a paid multi-agent turn, clean-account
browser/Finder installation, Keychain allow/deny, the full Sparkle install/relaunch
cycle, or runtime on minimum macOS 12.0.

## Hive 1.0.3 — November 9, 2026

The [release notes](releases/v1.0.3.md) describe the changes and verification limits.
The published release records the exact CI run, source revision, final archive
checksum, Developer ID, notarization, and downloaded-asset checks.

- Formatting, Clippy with warnings denied, 521 Rust tests (three existing opt-in
  tests ignored), 26 Python tests, and the core/GPUI boundary passed locally.
- Gitleaks passed on all Git refs and a clean snapshot of tracked and new source
  files. cargo-deny passed licenses, advisories, and sources with the existing
  documented maintenance exceptions.
- Regression tests cover attention labels and search context, empty results,
  unavailable usage with cached data, recovery, and keyboard dispatch across
  normal, empty, and failed diffs.
- Native checks in isolated development profiles covered notification status
  colors, wrapping, search, arrow navigation, Enter to the correct session, and
  clearing unread status; usage hover cards and accessibility descriptions; and
  Command-W closing empty, failed, refreshed, and ordinary diffs and an editor.
- The native app was built and exercised on macOS 27.2 (26B123), Apple Silicon.
  The installed app and active user sessions were not replaced.

Not reverified: a paid multi-agent turn, clean-account browser/Finder installation,
Keychain allow/deny, the full Sparkle install/relaunch cycle, or runtime on minimum
macOS 12.0. SSH notification context, missing usage data, and usage recovery have
automated coverage rather than new live-session coverage.

## README refresh — November 9, 2026

All three `docs/images/hive-*.png` files are new, unretouched captures of the running
1.0.3 development app in full screen (3456×2168). They show the Rust editor and
Files tree, the staged diff and Git panel, and the New worktree agent picker.
Full screen excludes the macOS sharing/control indicator. Each capture was checked
for control overlays, private projects, and account details before publication.

A separate app identity and `HIVE_DEV_DATA_DIR` isolated saved state. The demo used
`git archive 774d865` of Hive's public source with disposable `attention-details`
and `compact-limits` worktrees; the latter contained the current status-bar change.
No agent prompts were submitted and no demo changes were pushed. These captures
verify the UI presentation, not a clean-account installation of the signed release.

## Hive 0.1.11 — November 8, 2026

Published [preview release](https://github.com/one-reply/hive/releases/tag/v0.1.11)
from tag commit `749c72926db4fd94a1fd0333c5e20888abca6aed`, whose tree matches
build commit `332a83bdb145f5474b337e9a23aaf5e23666a040` and successful
[PR CI](https://github.com/one-reply/hive/actions/runs/42487237728).

- Local fmt, Clippy with warnings denied, 463 Rust tests (3 existing opt-in tests
  ignored), 15 Python tests, cargo-deny, core/GPUI boundary and Gitleaks passed.
- Both bundled static Linux helpers came from the exact build commit's CI.
- Built on macOS 27.2 (26B123), Apple Silicon. Developer ID team Lumorial Inc
  (`CFB6956H48`), hardened runtime, Apple notarization, stapled ticket,
  recursive codesign and Gatekeeper assessment passed.
- Final ZIP checksum and Sparkle archive/feed signatures verified; GitHub's
  uploaded asset digest matches the local ZIP. Local Xcode signing was used
  because hosted Apple API credentials are unavailable.
- Isolated startup logged 0.1.11 and handled missing CLI credentials. Terminal
  fixture comparisons and CPU measurements are recorded in the release notes.

Still unverified: browser/Finder installation in a clean macOS account, Keychain
allow/deny there, full Sparkle install/relaunch, minimum macOS 12.0 runtime, and
full native hover/keyboard/resize/selection coverage. Existing installations and
active sessions were not replaced. These limits also apply to claims in the README.

The records below describe earlier builds and retain their original verification status.

## Initial preview verification

Verification performed on 2026-11-04, macOS 27.2 (26B123), Apple Silicon.

| Check | Result |
| --- | --- |
| Formatting and Clippy with warnings denied | Passed |
| Workspace tests | 165 passed; two opt-in tests excluded from the default run |
| Native Keychain round trip | Passed separately, using a unique test service with cleanup |
| `hive-core` without GPUI | Passed |
| Gitleaks, historical refs and current source | Passed with the documented source-hash exception |
| RustSec, license and source checks | Passed with four documented upstream maintenance exceptions |
| ARM64 app, icon catalog, notices, ad-hoc signature, ZIP and checksum | Passed locally |
| Demo UI: shell input, Git discovery, diff, staging and commit | Passed in an isolated QA build |
| Demo UI: missing CLI credentials and unconfigured API providers | Correct empty/error states |
| Demo UI: theme persistence and quit | Preference written; QA application exited |
| Worktree create/remove and PTY child cleanup | Covered by automated core tests; GUI worktree flow not signed off |
| Installed agent CLI generation | Not run; would use the maintainer's real provider sign-in |
| Browser-downloaded release on a clean macOS account, Gatekeeper and Finder launch | Pending |
| GitHub-hosted formatting, Clippy, tests and release build | Passed on macOS 26.7.2 ARM64; 165 tests passed |
| Downloaded CI artifact checksum, archive, signature and system library dependencies | Passed |
| Exact-tag local build and uploaded draft assets | Passed; GitHub asset digests match the verified ZIP and checksum |
| GitHub-hosted secret and dependency audits | Runner allocation failed; passed locally on the exact release tag |
| Anonymous repository and release download | Pending public visibility |

The QA build differs only in package identity, the state directory and Keychain
service names. It protects the maintainer's existing data and credentials; it is
not the distributed executable and does not replace the downloaded-release gate.
The original public screenshot used this build and a disposable clone of Hive's
own source. The November 6 screenshots are documented below; the November 9 record supersedes them.

The `v0.1.0` tag points to `91e6c9a06e809ce108b91a4f08145a484615c0d1`.
Its [tag workflow](https://github.com/vitaliits-stack/hive/actions/runs/41894907955)
could not acquire GitHub-hosted ARM64 runners after repeated attempts; neither job
started. This is an infrastructure failure, not a failed test. The initial draft is
built locally from a clean checkout of that exact tag. Repeat CI with
`workflow_dispatch` on `v0.1.0` before promotion; this verifies the tag without
recreating or replacing the manually prepared draft.

Do not promote the draft until the remaining installation checks in
[RELEASING.md](RELEASING.md) have been recorded. Only macOS 27.2 has been exercised
locally through the UI; macOS 26.7.2 has automated coverage. No support claim is made
for macOS 11 from the Mach-O deployment target.

## README refresh — November 6, 2026

The November 6 README reflected the terminal-agent workflow, native editor and Files
navigator, review feedback, PR/CI cards, supported usage providers, and restoration
limits. The public Releases API returned no published releases on this date, so
the download section directs readers to a source build.

`docs/images/hive-editor.png`, `hive-review.png`, and `hive-create-worktree.png` are
system captures of the running development app on macOS. They show the editor and
Files tree, a unified diff with the Git panel, and the Create & Start agent picker.
Capture with `screencapture -x -o -a -l <window-id> -t png <output>` to exclude the
cursor, window shadow, and attached overlays. Capture the workspace in full-screen
mode to omit the macOS sharing indicator and rounded window corners; crop the
agent-picker capture around the dialog. Inspect every image for control overlays
and corner artifacts before publishing. The two workspace images are 3456×2168;
the dialog crop is 1080×950 and is displayed at half size in the README.
The app was built from the November 6 checkout at `b263fd2`, including the pending
Git-settings model-hint change later committed as `5faf3d5`; those settings are not
shown. A separate app identity and `HIVE_DEV_DATA_DIR` isolated its saved workspace.
The sample repository came from `git archive b263fd2`, with disposable worktrees
and a small documentation-only change to `terminal/activity.rs`. No agent prompts
were submitted and no sample changes were pushed.

Validation: source build, formatting, Clippy with warnings denied, 311 workspace
tests passed (three opt-in tests ignored), and no GPUI in the `hive-core` dependency
tree. The tests required local socket/process access outside the restricted
sandbox. These screenshots and checks do not replace the release installation,
notarization, real-agent, or notification verification gates above.
