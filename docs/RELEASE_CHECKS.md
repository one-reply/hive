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
| GitHub-hosted secret and dependency audits | Pending runner allocation; passed locally |
| Anonymous repository and release download | Pending public visibility |

The QA build differs only in package identity, the state directory and Keychain
service names. It protects the maintainer's existing data and credentials; it is
not the distributed executable and does not replace the downloaded-release gate.
The public screenshot uses this build and a disposable synthetic repository.

Do not promote the draft until the remaining installation checks in
[RELEASING.md](RELEASING.md) have been recorded. Only macOS 27.2 has been exercised
locally through the UI; macOS 26.7.2 has automated coverage. No support claim is made
for macOS 11 from the Mach-O deployment target.
