# gpui-component

Hive builds the published gpui-component 0.9.0 source with a local Cargo patch. The original crate checksum is `45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb`. Its license is in the package directory. [Upstream source](https://github.com/longbridge/gpui-kit/tree/7591a35eced761e3613c01e71ff04c10129dae1a/crates/component)

The diff changes are confined to `src/diff/{document,mod,state}.rs`. The review needs the widget's measured list for smooth scrolling, stable keys for two versions of the same path, per-file loading/error notices, and the identity of the document containing a text selection. Previously highlighted documents retain their syntax presentation when another patch arrives.

The window's focused-input registry holds a weak reference so that closing the last editor releases its buffer and undo history. A context menu acquires its own reference for the duration of its actions. Recheck both patches on a gpui-kit upgrade.

# core-text

Hive uses the source and licenses from core-text 21.0.0, crate SHA-256 `a593227b66cbd4007b2a050dfdd9e1d1318311409c8d600dc82ba1b15ca9c130`. The backport applies the Create Rule in `CTFontCollection::get_descriptors`. Three elided lifetimes in `run.rs` are made explicit for Hive's toolchain. Remove this patch when the font stack can use a published version containing the fix. [Upstream fix](https://github.com/servo/core-foundation-rs/pull/746)
