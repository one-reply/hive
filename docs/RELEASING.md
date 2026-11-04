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
export PATH="$PWD/target/release-tools:$PATH"
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
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
   and its checksum. Only the final release job has repository write permission.
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
