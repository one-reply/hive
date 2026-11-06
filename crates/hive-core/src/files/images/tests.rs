use super::*;
use crate::files::Snapshot;
use base64::Engine as _;
use image::ImageEncoder as _;
use std::fs;
use std::os::unix::fs::symlink;
use std::sync::atomic::{AtomicU64, Ordering};

const PNG: &[u8] = include_bytes!("fixtures/colors.png");
const SVG: &[u8] = include_bytes!("fixtures/colors.svg");
const SAMPLES: &[(&str, &[u8])] = &[
    ("colors.png", PNG),
    ("colors.jpeg", include_bytes!("fixtures/colors.jpeg")),
    ("colors.webp", include_bytes!("fixtures/colors.webp")),
    ("colors.bmp", include_bytes!("fixtures/colors.bmp")),
    ("colors.tiff", include_bytes!("fixtures/colors.tiff")),
    ("colors.ico", include_bytes!("fixtures/colors.ico")),
    ("two-frames.gif", include_bytes!("fixtures/two-frames.gif")),
    ("colors.svg", SVG),
];

fn preview(name: &str, bytes: &[u8]) -> Result<Decoded, Error> {
    let path = Path::new(name);
    decode(path, format(path).unwrap(), bytes, (32, 32))
}

#[test]
fn recognizes_supported_extensions_without_case_sensitivity() {
    for name in [
        "a.SVG", "a.PNG", "a.JPG", "a.JPEG", "a.WebP", "a.GIF", "a.BMP", "a.TIF", "a.TIFF", "a.ICO",
    ] {
        assert!(format(Path::new(name)).is_some(), "{name}");
    }
    for name in ["image", "image.rs", "image.heic", "image.svgz"] {
        assert!(format(Path::new(name)).is_none());
    }
}

#[test]
fn decodes_exported_files_in_every_supported_format() {
    for &(name, bytes) in SAMPLES {
        let image = preview(name, bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (width, height) = image.dimensions();
        assert!(width > 0 && height > 0);
        assert!(!image.omitted_resources());
        let pixels = image.into_pixels();
        assert_eq!(pixels.len(), width as usize * height as usize * 4);
        assert!(
            pixels[0] > 240 && pixels[1] < 15 && pixels[2] < 15,
            "{name}: {:?}",
            &pixels[..4]
        );
    }
}

#[test]
fn preserves_straight_alpha_and_primary_colors() {
    for (name, bytes, size) in [("a.png", PNG, 16), ("a.svg", SVG, 32)] {
        let image = preview(name, bytes).unwrap();
        assert_eq!(image.dimensions(), (size, size));
        let pixels = image.into_pixels();
        let pixel = |x: usize, y: usize| &pixels[(y * size as usize + x) * 4..][..4];
        assert_eq!(pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(size as usize - 2, 1), [0, 255, 0, 128]);
        assert_eq!(pixel(1, size as usize - 2), [0, 0, 255, 255]);
        assert_eq!(pixel(size as usize - 2, size as usize - 2), [0, 0, 0, 0]);
    }
}

#[test]
fn svg_rasterizes_at_viewport_resolution_without_stretching() {
    let source = br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><path fill="red" d="M0 0h16v8H0z"/></svg>"#;
    let image = decode(Path::new("a.svg"), Format::Svg, source, (600, 400)).unwrap();
    assert_eq!(image.dimensions(), (600, 300));
    assert!(
        image
            .into_pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [255, 0, 0, 255])
    );
    assert!(decode(Path::new("a.svg"), Format::Svg, source, (0, 100)).is_err());
}

#[test]
fn compressed_svg_cannot_bypass_the_source_byte_limit() {
    assert!(
        preview("a.svg", &[0x1f, 0x8b])
            .err()
            .unwrap()
            .to_string()
            .contains("compressed SVG")
    );
}

#[test]
fn gif_preview_uses_only_the_first_frame() {
    let image = preview("a.gif", include_bytes!("fixtures/two-frames.gif")).unwrap();
    assert_eq!(image.dimensions(), (1, 1));
    assert_eq!(image.into_pixels(), [255, 0, 0, 255]);
}

#[test]
fn applies_jpeg_exif_orientation() {
    let bytes = include_bytes!("fixtures/oriented.jpg");
    let unrotated = image::load_from_memory(bytes).unwrap().into_rgba8();
    let image = preview("a.jpg", bytes).unwrap();
    assert_eq!(
        image.into_pixels(),
        image::imageops::rotate180(&unrotated).into_raw()
    );
}

#[test]
fn reports_empty_malformed_and_truncated_images() {
    for &(name, bytes) in SAMPLES {
        for data in [
            &b""[..],
            &b"not an image"[..],
            &bytes[..bytes.len().min(12)],
        ] {
            assert!(preview(name, data).is_err(), "{name}");
        }
    }
    assert!(preview("a.svg", b"<svg><unclosed>").is_err());
}

#[test]
fn enforces_encoded_and_decoded_limits_before_allocating_pixels() {
    assert!(preview("a.png", &vec![0; MAX_IMAGE_BYTES + 1]).is_err());
    // BMP dimensions are available without allocating or providing the pixel data.
    for (width, height) in [(8193u32, 1u32), (1, 8193), (6000, 6000)] {
        let mut bytes = include_bytes!("fixtures/colors.bmp").to_vec();
        bytes[18..22].copy_from_slice(&width.to_le_bytes());
        bytes[22..26].copy_from_slice(&height.to_le_bytes());
        assert!(preview("a.bmp", &bytes).is_err());
    }
}

#[test]
fn svg_omits_external_and_oversized_embedded_images() {
    let f = Fixture::new();
    fs::write(f.0.join("secret.png"), PNG).unwrap();
    for href in [
        f.0.join("secret.png").display().to_string(),
        "https://example.invalid/image.png".into(),
    ] {
        let source = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><image href="{href}" width="16" height="16"/></svg>"#
        );
        let image = preview("a.svg", source.as_bytes()).unwrap();
        assert!(image.omitted_resources());
        assert!(image.into_pixels().iter().all(|v| *v == 0));
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(PNG);
    let source = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><image href="data:image/png;base64,{encoded}" width="16" height="16"/></svg>"#
    );
    let image = preview("a.svg", source.as_bytes()).unwrap();
    assert!(!image.omitted_resources());
    assert_eq!(&image.into_pixels()[..4], &[255, 0, 0, 255]);
    let mut oversized = Vec::new();
    let width = MAX_IMAGE_SIDE + 1;
    image::codecs::png::PngEncoder::new(&mut oversized)
        .write_image(
            &vec![0; width as usize * 4],
            width,
            1,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(oversized);
    let source = source.replace(
        &base64::engine::general_purpose::STANDARD.encode(PNG),
        &encoded,
    );
    assert!(
        preview("a.svg", source.as_bytes())
            .unwrap()
            .omitted_resources()
    );
}

#[test]
fn large_svg_can_preview_without_becoming_editable() {
    let mut source = SVG.to_vec();
    source.extend(vec![b' '; crate::files::MAX_FILE_BYTES]);
    assert!(preview("a.svg", &source).is_ok());
    assert!(Snapshot::parse(Path::new("a.svg"), &source).is_err());
}

#[test]
fn image_reads_keep_worktree_and_regular_file_guards() {
    let f = Fixture::new();
    fs::write(f.0.join("a.png"), PNG).unwrap();
    assert_eq!(read(&Host::Local, &f.0, Path::new("a.png")).unwrap(), PNG);
    symlink(f.0.join("a.png"), f.0.join("link.png")).unwrap();
    for path in ["link.png", "../a.png", ".git/a.png", "missing.png", ""] {
        assert!(read(&Host::Local, &f.0, Path::new(path)).is_err(), "{path}");
    }
    assert!(
        read(
            &Host::Ssh {
                target: "example".into()
            },
            &f.0,
            Path::new("a.png")
        )
        .is_err()
    );
    let file = fs::File::create(f.0.join("large.png")).unwrap();
    file.set_len(MAX_IMAGE_BYTES as u64 + 1).unwrap();
    assert!(read(&Host::Local, &f.0, Path::new("large.png")).is_err());
}

#[test]
fn svg_source_preserves_save_conflicts_and_recovery_compatibility() {
    use crate::files::{self, recovery};
    let f = Fixture::new();
    let path = Path::new("source.svg");
    fs::write(f.0.join(path), SVG).unwrap();
    let base = files::read(&Host::Local, &f.0, path).unwrap();
    let text = base.text().replace("red", "blue");
    let draft = recovery::Draft {
        host: Host::Local,
        project: f.0.clone(),
        root: f.0.clone(),
        path: path.into(),
        base: base.clone(),
        text: text.clone(),
    };
    let directory = f.0.join("recovery");
    let key = recovery::key(&Host::Local, &f.0, path);
    recovery::write(&directory, &key, &draft).unwrap();
    let (records, errors) = recovery::load(&directory);
    assert!(errors.is_empty());
    assert_eq!(records[0].draft.text, text);
    assert_eq!(records[0].draft.base, base);
    let saved = files::save(&Host::Local, &f.0, path, &base, &text).unwrap();
    assert!(saved.matches(&text));
    assert!(matches!(
        files::save(&Host::Local, &f.0, path, &base, "stale"),
        Err(FileError::Conflict(_))
    ));
    assert_eq!(files::read(&Host::Local, &f.0, path).unwrap(), saved);
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hive-image-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
