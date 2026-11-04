use super::*;
use image::{DynamicImage, Rgb, RgbImage};
use std::os::unix::fs::PermissionsExt;

fn encoded_image(format: ImageFormat) -> Vec<u8> {
    let image = DynamicImage::ImageRgb8(RgbImage::from_fn(3, 2, |x, y| {
        Rgb([x as u8 * 60, y as u8 * 100, 90])
    }));
    let mut encoded = Cursor::new(Vec::new());
    image.write_to(&mut encoded, format).unwrap();
    encoded.into_inner()
}

#[test]
fn images_become_private_pngs_without_resizing_or_pixel_changes() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::Tiff] {
        let bytes = encoded_image(format);
        let expected = image::load_from_memory(&bytes).unwrap().to_rgba8();
        let path = save_image(&bytes).unwrap();
        let saved = fs::read(&path).unwrap();
        assert_eq!(image::guess_format(&saved).unwrap(), ImageFormat::Png);
        assert_eq!(
            image::load_from_memory(&saved).unwrap().to_rgba8(),
            expected
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn repeated_pastes_do_not_overwrite_files() {
    let bytes = encoded_image(ImageFormat::Png);
    let first = save_image(&bytes).unwrap();
    let original = fs::read(&first).unwrap();
    let second = save_image(&bytes).unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(&first).unwrap(), original);
    fs::remove_file(first).unwrap();
    fs::remove_file(second).unwrap();
}

#[test]
fn corrupt_and_unsupported_images_are_rejected() {
    for bytes in [&b"not an image"[..], &b"\x89PNG\r\n\x1a\n"[..]] {
        assert!(matches!(save_image(bytes), Err(PasteError::Decode(_))));
    }
    assert!(matches!(
        save_image(b"GIF89a"),
        Err(PasteError::UnsupportedFormat)
    ));
}

#[test]
fn filesystem_error_includes_the_destination_and_keeps_existing_file() {
    let bytes = encoded_image(ImageFormat::Png);
    let file = save_image(&bytes).unwrap();
    let original = fs::read(&file).unwrap();
    let error = save_image_in(&bytes, &file).unwrap_err();
    match error {
        PasteError::Io {
            operation, path, ..
        } => {
            assert_eq!(operation, "create");
            assert_eq!(path.parent(), Some(file.as_path()));
        }
        other => panic!("expected a filesystem error, got {other}"),
    }
    assert_eq!(fs::read(&file).unwrap(), original);
    fs::remove_file(file).unwrap();
}

#[test]
fn quotes_paths_without_turning_them_into_commands() {
    assert_eq!(
        quoted_path(Path::new("/tmp/image.png")).unwrap(),
        "/tmp/image.png"
    );
    assert_eq!(
        quoted_path(Path::new("/tmp/it's a $(command).png")).unwrap(),
        "'/tmp/it'\\''s a $(command).png'"
    );
    for path in ["relative.png", "/tmp/image\n.png", "/tmp/\x1b[201~.png"] {
        assert!(matches!(
            quoted_path(Path::new(path)),
            Err(PasteError::InvalidPath(_))
        ));
    }
}

#[test]
fn preserves_text_paste_modes_and_does_not_submit_image_paths() {
    let text = "first\r\nsecond\nтретій";
    assert_eq!(
        text_bytes(text, TermMode::BRACKETED_PASTE),
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    );
    assert_eq!(
        text_bytes(text, TermMode::empty()),
        "first\rsecond\rтретій".as_bytes()
    );
    for mode in [TermMode::empty(), TermMode::BRACKETED_PASTE] {
        let bytes = text_bytes(&quoted_path(Path::new("/tmp/my image.png")).unwrap(), mode);
        assert!(!bytes.contains(&b'\r'));
        assert!(!bytes.contains(&b'\n'));
    }
}
