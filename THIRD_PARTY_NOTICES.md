# Third-party notices

Hive's own code and application mark are Apache-2.0 licensed. Third-party resources
retain the terms below. Product names and logos belong to their respective owners;
their use identifies integrations and does not imply endorsement or grant trademark rights.

## Bundled artwork

| Resource | Source | License |
| --- | --- | --- |
| `openai.svg`, `gemini.svg`, `copilot.svg` | [Simple Icons 14.15.0](https://github.com/simple-icons/simple-icons/tree/14.15.0/icons): `openai.svg`, `googlegemini.svg`, `githubcopilot.svg`; path data retained, SVG wrapper adapted | [CC0-1.0](licenses/simple-icons-CC0.txt) |
| `cursor.svg` | [Simple Icons](https://github.com/simple-icons/simple-icons/blob/aac2d0877b8a8afee1792c342fac0664ed294488/icons/cursor.svg); path data retained, SVG wrapper adapted | [CC0-1.0](licenses/simple-icons-CC0.txt) |
| `claude.svg`, `qwen.svg`, `anthropic.svg`, `opencode.svg`, `openrouter.svg`, `google-ai.svg` | [Lobe Icons](https://github.com/lobehub/lobe-icons/tree/ea434b96e75813517b377825e01c0df61dee8836/packages/static-svg/icons), same filenames except `google-ai.svg` is `google.svg`; unmodified SVGs | [MIT, LobeHub](licenses/lobe-icons-MIT.txt) |
| GPUI Kit's embedded icon catalog | `gpui-kit-assets` 0.9.0, Lucide and Feather-derived icons | [ISC and MIT notices](licenses/gpui-kit-assets-Lucide.txt) |

`agent-terminal.svg` is Hive's own geometric terminal glyph. The Hive app mark and
its exported PNG/ICNS files are project artwork; its editable vector source is included.
The app uses system/user-installed fonts, not a bundled proprietary font.

## Syntax queries

The JSX query fragments in `crates/hive/src/ui/syntax.rs` are adapted from
[tree-sitter-javascript 0.23.1](https://github.com/tree-sitter/tree-sitter-javascript/tree/v0.23.1/queries).
Its [MIT copyright and license](licenses/tree-sitter-javascript-MIT.txt) are included.
Other queries and grammars are provided by the dependencies listed in `Cargo.lock`.

## Rust dependencies

The macOS bundle includes [Sparkle 2.10.1](https://github.com/sparkle-project/Sparkle/releases/tag/2.10.1)
for software updates. Its MIT license and bundled dependency notices are preserved in
`Contents/Resources/licenses/Sparkle.txt` and in the framework's resources.

The app bundle includes `Contents/Resources/THIRD_PARTY_LICENSES.html`, generated
from the locked Apple Silicon dependency graph by cargo-about. Licensing policy is
checked by cargo-deny. The `licenses/` directory and this notice are also bundled.

Some crate packages omit their original copyright texts. Their
[supplemental upstream notices](licenses/upstream/README.md) are included with
immutable source links and checksums; these supplement any generic license terms
in the generated inventory. Upstream notices for Objective-C bindings also explain
their derivation from Apple's SDKs. The app does not redistribute the SDK itself.

For MPL-2.0 dependencies, the corresponding unmodified crate source archives are
included in `Contents/Resources/licenses/`, including their licenses. This makes
their covered source available alongside the executable. Hive's license does not
replace their MPL terms.
