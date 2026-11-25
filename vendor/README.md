# gpui-component

Hive builds the published gpui-component 0.9.0 source with a local Cargo patch. The original crate checksum is `45bbfdba0efa392190f2fd446ec311325302eb047d5ce45dc1bcf189cb18f5eb`. Its license is in the package directory. [Upstream source](https://github.com/longbridge/gpui-kit/tree/7591a35eced761e3613c01e71ff04c10129dae1a/crates/component)

Hive's changes are confined to `src/diff/{document,mod,state}.rs`. The review needs the widget's measured list for smooth scrolling, stable keys for two versions of the same path, per-file loading/error notices, and the identity of the document containing a text selection. Previously highlighted documents retain their syntax presentation when another patch arrives. Recheck these APIs on a gpui-kit upgrade and remove the patch once upstream provides them.
