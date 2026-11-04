# Contributing to Hive

Hive is an early macOS Apple Silicon application. Focused fixes and reproducible
bug reports are welcome. Open an issue before a larger feature or architecture change.

## Development

Install native ARM64 Rust via rustup, Xcode Command Line Tools, and Git. The pinned
toolchain is in `rust-toolchain.toml`; Cargo installs it through rustup when needed.

```sh
cargo run --locked -p hive
cargo fmt --all
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo tree --locked -p hive-core | grep gpui
```

The last command must print nothing (`grep` exits 1 when it finds no matches).
UI changes also need testing in the running application. Full app packaging requires
Xcode 26 or newer and Python 3; see [Releasing](docs/RELEASING.md).

## Architecture and conventions

- `hive-core` owns domain logic, Git/CLI/HTTP adapters, persistence and the PTY model.
  It must never depend on GPUI. Use plain types and `thiserror` errors.
- `hive` owns GPUI state entities and views. Dependencies point from views to state
  to core. Views render and dispatch; they do not read files or spawn processes.
- Run blocking work on the background executor. Never hold an entity lease across
  a call that can re-enter the same entity. Retain tasks and subscriptions you need alive.
- Persistent mutations go through `Store`; Git and agent commands use `Host::command`.
- Reuse existing components and patterns. Add dependencies or abstractions only for
  a current need. Declare dependencies centrally with minimal features.
- Treat files, process output, PTY bytes and HTTP responses as untrusted. Preserve
  errors, avoid panics, and never log keys, tokens or complete provider responses.
- Add a regression test for a logic bug. Keep public core APIs documented. Use theme
  values for UI sizes/colors and verify keyboard access and visible focus.

Keep pull requests focused and describe the user-visible change and how you tested it.
Maintainer-only agent instructions and working notes are intentionally not distributed.
Report security issues [privately](SECURITY.md), not in a public issue.

## License

By submitting a contribution, you agree it may be distributed under Hive's
[Apache-2.0 license](LICENSE). Only contribute material you have the right to license;
retain the licenses and attribution of third-party material.
