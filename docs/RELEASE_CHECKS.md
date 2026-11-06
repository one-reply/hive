# Initial preview verification

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
own source. The current README screenshots are documented below.

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

The current README reflects the terminal-agent workflow, native editor and Files
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
