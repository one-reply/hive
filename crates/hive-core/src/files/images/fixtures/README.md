# Image preview fixtures

Related behavior: `../tests.rs` and `wiki/modules/editor.md`.

- `colors.png`: authored 16×16 RGBA quadrants (opaque red/blue, half-alpha green, transparent).
- `colors.jpeg`, `colors.bmp`, `colors.tiff`, `colors.ico`: macOS ImageIO exports of that PNG using `sips`.
- `colors.webp`: a 16×16 opaque red image exported once with `image` 0.25's lossless WebP encoder.
- `colors.svg`: the same quadrants as SVG geometry, with no fonts or external resources.
- `two-frames.gif`: a minimal GIF89a with two 1×1 frames, red then blue.
- `oriented.jpg`: the 180° EXIF example from `gpui-pre` 0.3.11, `examples/image/exif-orientation-rotate-180.jpg` (Apache-2.0).

These are checked-in encoded files; tests do not invoke exporters, networking, or installed image applications.
