<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="crates/hive/assets/icons/hive-dark.png">
    <img src="crates/hive/assets/icons/hive-light.png" width="112" alt="Hive logo">
  </picture>
</p>

# Hive

**A native desktop workspace for AI coding agents.**

Keep your projects, Git worktrees, agent terminals, and code reviews in one place.
Hive is written in Rust, with a GPU-rendered interface built on GPUI through
`gpui-kit`.

![Hive reviewing a Rust change in its own source](docs/images/hive-demo.png)

*Hive reviewing its own Rust source in an isolated local worktree.*

> **Early development · macOS / Apple Silicon**
>
> Hive is early software; features and interfaces are still evolving.
> Intel Macs are unsupported.
> Linux and Windows are planned.

## Download

Get the Apple Silicon ZIP and its `.sha256` file from
[GitHub Releases](https://github.com/vitaliits-stack/hive/releases).
While the first release is being verified, you can build from source below.

1. Download both files into the same folder and verify the archive:
   `shasum -a 256 -c Hive-0.1.0-macos-arm64.zip.sha256`.
2. Extract the ZIP and drag **Hive.app** to **Applications**.
3. Open Hive from Finder. This preview is **ad-hoc signed, without Developer ID
   or Apple notarization**. macOS may block it. After attempting to open it, use
   **System Settings → Privacy & Security → Open Anyway** only if you trust the
   download. See [Apple's instructions](https://support.apple.com/en-gb/102445).

Keep Gatekeeper enabled. Checksums detect corruption; they do not authenticate the
publisher. Managed Macs may prohibit opening an unnotarized app.

The release notes list the macOS versions actually tested. The deployment target
inside the executable is not a claim that every older macOS version is supported.
See the [verification record](docs/RELEASE_CHECKS.md) for completed and pending checks.

Packaged builds from **0.1.1** include signed in-app updates. Open **Settings → Updates**
or **Hive → Check for Updates…** to check manually. Hive also checks daily and asks
before installing and restarting. Finish active agent sessions before restarting.
Older builds need one manual installation of a version that includes the updater.

## What you can do today

| Workflow | What Hive provides |
| --- | --- |
| **Organize your work** | Add local projects, create and remove Git worktrees, reorder projects, and customize their names, colors, and icons. |
| **Run agents in terminals** | Use terminal tabs and split panes, with text selection, scrollback, and clipboard image paste. |
| **Find active agents** | See agents grouped by worktree in the sidebar and jump straight to their terminal panes. |
| **Review and commit** | Inspect unified or split diffs, stage files or hunks, compare branches, browse history, and commit, fetch, pull, or push. |
| **Generate commit messages** | Use an installed Codex or Claude Code CLI, or an API provider, to draft a message from staged changes. |
| **Track usage** | View Claude Code and Codex usage limits and reset times using your existing CLI sign-ins. |
| **Make it yours** | Switch between system, light, and dark themes; adjust UI and terminal font sizes; remap shortcuts. |
| **Keep Hive updated** | Check your installed version and install signed updates from Settings → Updates. |

Hive recognizes **Claude Code, Codex, OpenCode, Gemini CLI, Cursor Agent, Copilot CLI,
Qwen Code, Amp, Aider, and Droid** running in its local terminals. Install and sign
in to the agents you want to use separately, then launch them as you would in any
terminal.

Agent activity is inferred from terminal output. **Working / Idle** is an estimate;
it does not distinguish an approval prompt from an idle agent. Structured agent
sessions and approval controls are on the roadmap.

## Build and run

You need:

- A Mac with **Apple Silicon** and a native ARM64 Rust toolchain.
- **rustup** and Cargo; `rust-toolchain.toml` pins the tested Rust toolchain.
- **Xcode Command Line Tools** (or full Xcode) for the macOS SDK and native compiler.
- **Git** on your `PATH`.

If you do not already have Xcode or its Command Line Tools, install them first:

```sh
xcode-select --install
```

Then clone and run:

```sh
git clone https://github.com/vitaliits-stack/hive.git
cd hive
cargo run --locked -p hive
```

For a release build, use `cargo run --locked --release -p hive`.

### Your first workspace

1. Press **⌘O** to add a local repository.
2. Select its primary worktree, or create another from the project's **+** button.
3. Open a terminal with **⌘T** and launch your agent, for example `claude` or `codex`.
4. Review changes in the Git panel, stage what you want, and commit.

Projects, terminals, and Git tools work without configuring an AI API key in Hive.
Usage indicators require a supported Claude Code or Codex CLI sign-in.

### AI commit messages

Open **Settings → Workspace → Git** to choose a generator and customize its prompt.
CLI generation supports **Codex** and **Claude Code**. API generation supports
**OpenAI, Anthropic, OpenRouter, and Google AI**; add the corresponding key in
**Settings → Workspace → AI**, then select a model in Git settings.

Stage your changes and click the sparkle button in the commit message field.
Review the generated message before committing. The default prompt includes the
branch name, staged filenames, and staged patch and sends them to your selected
CLI or API provider.

### Build a macOS app bundle

Packaging additionally requires **Xcode 26 or newer** for the app icon compiler,
**Python 3**, and **cargo-about** for third-party notices. Install the pinned release
tools into a local build directory:

```sh
python3 scripts/install-release-tools.py target/release-tools
PATH="$PWD/target/release-tools:$PATH" bash scripts/bundle-macos.sh release
open target/release/bundle/Hive.app
```

This is the default output path; a custom Cargo target directory changes it.
The script creates an ARM64-only, locally ad-hoc-signed app. It does not produce a
notarized distribution release. Release mode also writes a ZIP and SHA-256 file.
See [Releasing](docs/RELEASING.md).

## Keyboard shortcuts

| Action | Default |
| --- | --- |
| Add project | ⌘O |
| New terminal | ⌘T |
| Split terminal right | ⌘D |
| Split terminal down | ⌘⇧D |
| Close terminal pane | ⌘W |
| Toggle project sidebar | ⌘⇧\ |
| Toggle Git panel | ⌘\ |
| Commit staged changes, while focused in the Git panel | ⌘Enter |
| Open settings | ⌘, |

Shortcuts can be changed in **Settings → Interface → Shortcuts**.

## Data and credentials

- Project references and settings are stored in `~/.hive/state.json`.
- Worktrees created by Hive live beside the repository in `<repo>-worktrees/`.
- API keys entered in Hive are stored in **macOS Keychain**, separately from settings.
- Usage indicators read existing CLI credentials and contact the provider's usage
  endpoint. They refresh every five minutes or on demand. These integrations rely
  on undocumented endpoints and may stop working when providers change them.
- Logs are written to `~/.hive/logs/hive.log`. For troubleshooting, run
  `RUST_LOG=hive_core=debug cargo run --locked -p hive`. Usage diagnostics include
  HTTP status and response size, not response bodies or credentials. Other logs can
  include local paths and Git errors; review and redact them before sharing.

## Roadmap

- Structured Claude Code and Codex sessions with event history and approval controls.
- Worktrees tied to agent sessions, with pull request and CI status.
- Agent hooks and desktop notifications.
- SSH host configuration in the app.
- Linux and Windows support.

These are planned capabilities, not features available in the current build.

## Contributing

Bug reports, reproducible examples, and focused improvements are welcome. For a
larger change, open an issue first to discuss the approach. Include your macOS
version, Hive commit, reproduction steps, and relevant redacted logs in bug reports.

The workspace has two crates:

| Crate | Responsibility |
| --- | --- |
| [`hive-core`](crates/hive-core) | Domain logic, Git and CLI adapters, HTTP clients, persistence, and the terminal model. No GPUI dependency. |
| [`hive`](crates/hive) | The GPUI app, state entities, views, and terminal rendering. |

Read [CONTRIBUTING.md](CONTRIBUTING.md) for engineering conventions and
[SECURITY.md](SECURITY.md) for private vulnerability reporting.

Before submitting a change:

```sh
cargo fmt --all
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo tree --locked -p hive-core | grep gpui
```

The last command must print nothing (`grep` exits with status 1 when there are no
matches). Verify UI changes in the running app and update the relevant documentation
when behavior or architecture changes.

## License

Hive is licensed under [Apache-2.0](LICENSE). Third-party dependencies and artwork
retain their own licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).
