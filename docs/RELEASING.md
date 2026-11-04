# Releasing Hive

Hive currently ships macOS Apple Silicon builds only. The initial release is an
ad-hoc-signed preview without Developer ID or notarization. Do not describe it as
an Apple-verified application or publish a minimum supported macOS version solely
from the binary's deployment target.

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
the ad-hoc signature, ZIP integrity, and checksum. Artifacts are written to
`target/release/bundle/` (or the configured Cargo target directory).

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
   runs security and build checks, and creates a **draft prerelease** with the ZIP
   and its checksum, plus a signed `appcast.xml`. Only the final release job has
   repository write permission and access to the update-signing secret.
2. Download that draft ZIP through a browser, rather than copying the local `.app`.
   Verify the checksum, extract it, and install it in Applications. Use a separate
   test account or Mac so existing projects, credentials and running agents are safe.
3. With Gatekeeper enabled, record the actual first-launch behavior. If offered,
   use Privacy & Security → Open Anyway following [Apple's guidance](https://support.apple.com/en-gb/102445).
   Do not advise disabling Gatekeeper globally. A managed-device restriction is not
   a packaging bug; document it.
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
  --download-url-prefix "https://github.com/vitaliits-stack/hive/releases/download/v<version>/" \
  --link "https://github.com/vitaliits-stack/hive/releases/tag/v<version>" <release-directory>
target/sparkle/bin/sign_update --account dev.hive.Hive --verify <release-directory>/appcast.xml
```

Upload the generated signed `appcast.xml` alongside the ZIP and checksum. Do not
edit that XML after signing. Sparkle's Ed25519 signature is independent of Apple
Developer ID/notarization; the preview still uses ad-hoc Apple code signing.

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
as a prerelease. Keep release notes explicit about ad-hoc signing and tested systems.
If a published build is faulty, mark it as withdrawn and publish a corrected version;
do not silently replace a ZIP under an existing checksum or move a published tag.
