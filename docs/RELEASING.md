# Releasing Hive

Hive currently ships macOS Apple Silicon builds only. Local builds default to an
ad-hoc-signed preview. The tag release workflow requires Developer ID signing and
Apple notarization; missing credentials or a rejected submission stop the release.
Previously created ad-hoc drafts are not upgraded by this workflow change. Do not
describe an artifact as notarized until its ticket and Gatekeeper assessment pass,
or claim minimum macOS support solely from the binary's deployment target.

## Build

Use an ARM64 Mac, rustup, Xcode 26 or newer selected with `xcode-select`, Python 3,
and Git. The Rust toolchain and release-tool versions are pinned in the repository.

```sh
python3 scripts/install-release-tools.py target/release-tools
python3 scripts/install-sparkle.py target/sparkle
export PATH="$PWD/target/release-tools:$PATH"
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 -m unittest discover -s scripts -p 'test_*.py'
cargo deny --locked check licenses advisories sources
bash scripts/bundle-macos.sh release
```

Also check that `cargo tree --locked -p hive-core | grep -i gpui` produces no matches.
The packager validates icons, arm64 architecture, bundle metadata, included notices,
the code signature, ZIP integrity, and checksum. Artifacts are written to
`target/release/bundle/` (or the configured Cargo target directory).

## Developer ID and notarization

Hive uses Xcode's cloud-managed **Developer ID Application** signing workflow.
Choose the Apple Developer team authorized to publish Hive: its organization name
will appear in the signature. The Xcode account must have access to cloud-managed
Developer ID certificates for that team. A local Apple Development certificate
alone does not grant that permission. See [Apple's cloud signing documentation](https://developer.apple.com/help/account/certificates/cloud-managed-certificates/).

For a local release, sign in to the authorized team in Xcode → Settings → Accounts:

```sh
export HIVE_APPLE_TEAM_ID='YOURTEAMID'
PATH="$PWD/target/release-tools:$PATH" bash scripts/bundle-macos.sh release
```

The ten-character team ID is available in the Apple Developer account. Without any
Apple variables, the packager produces a local ad-hoc preview. Partial configuration
fails instead of silently producing a preview. To sign an already built disposable
copy, run `python3 scripts/sign_macos.py /absolute/path/to/Hive.app` with the same
variable. The script uses `-allowProvisioningUpdates`, allowing Xcode to manage the
certificates and provisioning resources required for that team's distribution.

The script wraps the existing build in an `.xcarchive`; it does not rebuild Rust.
Xcode signs the app and nested Sparkle code with Developer ID and uploads the result
to Apple's notary service. The script waits up to 30 minutes for notarized export,
then requires ticket validation, recursive signature verification and Gatekeeper
assessment. The final ZIP is created **after** stapling; SHA-256 and Sparkle
signatures are generated from those final bytes. No Developer ID private key is
exported into GitHub.

If Apple is still processing or export fails, the script preserves the archive and
`upload.log` at the printed path. On a local Mac, a pending submission can be
resumed without uploading another copy. Hosted runners are ephemeral: their archive
and raw logs are not published as artifacts, so this recovery path is local only.
Inspect the error before retrying:

```sh
xcodebuild -exportNotarizedApp -archivePath /path/to/Hive.xcarchive \
  -exportPath /path/to/notarized -allowProvisioningUpdates
xcrun stapler validate /path/to/notarized/Hive.app
codesign --verify --deep --strict /path/to/notarized/Hive.app
spctl --assess --type execute --verbose=2 /path/to/notarized/Hive.app
```

Use the exported app to create a new ZIP, checksum and Sparkle appcast; never reuse
the checksum or update signatures from the ad-hoc input.

For unattended GitHub releases, an authorized maintainer must configure:

| Setting | Value |
| --- | --- |
| Variable `APPLE_TEAM_ID` | Authorized ten-character Apple Developer team ID |
| Secret `APPLE_API_KEY_BASE64` | Base64 of a private App Store Connect `.p8` API key authorized for this team's cloud signing |
| Secret `APPLE_API_KEY_ID` | The API key's ID |
| Secret `APPLE_API_ISSUER_ID` | The team API key's issuer UUID |

The API key must have the team's cloud-managed Developer ID permissions; a
notarization-only key is not sufficient for cloud signing. Review its actual access
with the organization before storing it in GitHub. A successful local Xcode export
does not establish that a separately configured API key works on a hosted runner.
The release job fails if credentials are missing; it never falls back to ad-hoc.

The workflow writes the API key to a permission-restricted temporary file and removes
it on exit. PR/build jobs never receive it. For local API authentication, set
`HIVE_APPLE_API_KEY` to that file's path, plus `HIVE_APPLE_API_KEY_ID` and
`HIVE_APPLE_API_ISSUER_ID`; otherwise Xcode uses its signed-in account. Keep keys
outside the checkout and never paste their contents into chat or command arguments.
The existing `SPARKLE_PRIVATE_KEY` separately signs the final update archive and feed.
See [Apple's automation workflow](https://developer.apple.com/videos/play/wwdc2021/10204/)
and [notarization workflow](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow).

Before publishing, download the final draft ZIP through a browser and test first
launch in a clean macOS profile, along with terminal, Keychain and Sparkle updates.
Do not move an existing protected tag or replace already published archive bytes;
use a new version when changing the signing identity or notarization state.

The `licenses/` directory covers bundled artwork and supplemental dependency notices.
cargo-about generates the dependency license inventory from `Cargo.lock`; MPL-covered
crate sources are included alongside the executable. Recheck notices on dependency updates.

Four cargo-deny exceptions currently cover upstream maintenance advisories for
`instant`, `paste`, `rustybuzz`, and `ttf-parser`. Each has an explicit reason in
`deny.toml`; revisit them when upgrading GPUI. These do not suppress new advisories.
`tree-sitter-graphql` declares `license-file` instead of an SPDX field; its packaged
MIT license is verified by the tools, which may still print a metadata warning.

## Before the first public release

Scan all branches and tags with Gitleaks, including archives and encoded content:

```sh
gitleaks git --log-opts='--all' --redact=100 --max-archive-depth=3 --max-decode-depth=2
```

Scan the current source snapshot separately with `gitleaks dir`, excluding local
build output and ignored maintainer files. CI runs this on a clean checkout.
Review screenshots, compressed diagnostics, Git author identities, issues,
release assets and Actions logs. Never commit scan reports containing private data.
The narrow Gitleaks exception covers verified source-file hashes in the historical
performance manifests, not arbitrary keys or whole directories.

Maintainer agent instructions and working notes are local-only and ignored. Keep a
private backup before any history cleanup. Rewriting history changes commit IDs;
coordinate with collaborators, use an explicit force-with-lease, and have existing
clones re-clone or rebase without merging the old history back.

A force push does not purge GitHub's cached commits. Before changing visibility,
check whether removed material can still be retrieved by its old commit IDs.
If that material must remain private, use GitHub Support's cleanup process or
publish the cleaned history in a fresh repository while keeping the original private. See
[GitHub's removal guidance](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository).

## Draft release and installation checks

1. Update the workspace version, document changes, and wait for CI on the intended
   commit. Push a matching `v<version>` tag. CI verifies the tag/version match,
   runs security and build checks, signs and notarizes the app, and creates a
   **draft prerelease** with the stapled ZIP, checksum and signed `appcast.xml`.
   Only the final release job has repository write permission and access to the
   Apple credentials and update-signing secret.
2. Download that draft ZIP through a browser, rather than copying the local `.app`.
   Verify the checksum, extract it, and install it in Applications. Use a separate
   test account or Mac so existing projects, credentials and running agents are safe.
3. With Gatekeeper enabled, record the actual first-launch behavior. A notarized
   release must pass `xcrun stapler validate Hive.app` and `spctl --assess --type
   execute --verbose=2 Hive.app`, and open without an unidentified-developer override.
   Only legacy/local ad-hoc previews may need Privacy & Security → Open Anyway
   following [Apple's guidance](https://support.apple.com/en-gb/102445). Do not use
   that override as evidence that notarization worked or disable Gatekeeper globally.
   A managed-device restriction is separate; document it.
4. Test launch through Finder, terminal input and installed CLI discovery, worktree
   creation/removal, diff/staging/commit in a disposable repository, settings after
   restart, Keychain access/denial, and child-process cleanup on quit.
5. Test a clean account without CLI sign-ins or API keys. Project, terminal and Git
   workflows must work; usage and AI actions must explain missing credentials.
6. Record exact macOS versions, build commit and completed checks in release notes.
   Use only synthetic project data in public screenshots. Do not mark skipped tests
   as passed. Keep the draft private if an installation gate is unresolved.

If hosted runners cannot start, a maintainer may build the exact tag locally with
all the same checks and create a private draft for installation testing. Record the
build provenance and failed runner allocation in its notes. Run `workflow_dispatch`
on that tag when runners recover; the release job only runs on tag pushes, so this
does not replace draft assets. CI and installation checks must pass before promotion.

## In-app updates

Settings → Updates and Hive → Check for Updates use Sparkle 2. The packaged app
checks daily and requires the user's confirmation to install; silent installation
is disabled because agent sessions may be running. `cargo run` has no embedded
framework and shows a link to download the packaged application instead.

The framework and signing tools are pinned and checksum-verified by
`scripts/install-sparkle.py`. The bundle includes Sparkle's complete license notices.
`objc2` and `objc2-foundation`, already present through GPUI, provide the native API
bridge; no Sparkle build dependency or updater code is added to `hive-core`.

The public key in `packaging/macos/Info.plist` verifies both update archives and
appcasts. The private key lives in the maintainer's Keychain under account
`dev.hive.Hive`, and in the repository Actions secret `SPARKLE_PRIVATE_KEY` for
the tag-only release job. Keep a secure backup of this key: replacing it in a new
build does not make already installed copies trust the replacement. Never print
the private key or pass it in command-line arguments.

For a local signed draft, put only the version's ZIP and checksum in a fresh directory:

```sh
target/sparkle/bin/generate_appcast --account dev.hive.Hive --maximum-deltas 0 \
  --download-url-prefix "https://github.com/one-reply/hive/releases/download/v<version>/" \
  --link "https://github.com/one-reply/hive/releases/tag/v<version>" <release-directory>
target/sparkle/bin/sign_update --account dev.hive.Hive --verify <release-directory>/appcast.xml
```

Upload the generated signed `appcast.xml` alongside the ZIP and checksum. Do not
edit that XML after signing. Sparkle's Ed25519 signature is independent of Apple
Developer ID/notarization; both must be verified for a distribution release.

GitHub Pages uses **GitHub Actions** as its source. `Publish update feed` runs when
a release is published, unpublished or deleted, or through `workflow_dispatch`.
It copies the highest numeric `vMAJOR.MINOR.PATCH` release's signed appcast without
changing its bytes. Published previews are included; drafts, incomplete uploads
and non-numeric tags are excluded. Until the first signed release is published,
the signed empty feed reports no updates. Downloads are public and clients need
no GitHub token. The workflow has no access to the private signing key.

Before raising the minimum macOS version, extend the feed to retain updates for
older supported systems; the current feed advertises one release. To withdraw an
update, unpublish/delete its release and rerun the feed workflow if needed. Existing
installations are never automatically downgraded.

Test an update from a disposable older bundle: check, download, verify, install,
restart, and check again. Confirm that normal quit cleanup runs. Also test an
unreachable feed and a tampered signature; neither may replace the installed app.

## Publish

After the audit and installation gates pass, make the repository public and enable
secret scanning, push protection, Dependabot alerts and Private Vulnerability
Reporting. Protect `main` with PRs and required `Security and licenses` and
`macOS checks and bundle` checks, block force pushes/deletion, and require zero
additional approvals while there is one maintainer. Verify settings after the
visibility change, since availability varies between private and public repositories.

Verify the repository and download links without signing in, then publish the draft
as a prerelease. Keep release notes explicit about the actual signing/notarization
status and tested systems.
If a published build is faulty, mark it as withdrawn and publish a corrected version;
do not silently replace a ZIP under an existing checksum or move a published tag.
