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

#[test]
fn file_references_follow_the_receiving_terminal_directory() {
    let root = Path::new("/repo");
    for (path, directory, expected) in [
        ("src/main.rs", "/repo", "src/main.rs"),
        ("src/main.rs", "/repo/src", "main.rs"),
        ("README.md", "/repo/src/deep", "../../README.md"),
        ("src/main.rs", "/repo/tests", "../src/main.rs"),
        ("src/main.rs", "/elsewhere", "../repo/src/main.rs"),
        ("src", "/repo/src", "."),
        ("-flag.txt", "/repo", "./-flag.txt"),
    ] {
        assert_eq!(
            file_reference(root, Path::new(path), Path::new(directory)).unwrap(),
            expected
        );
    }
}

#[test]
fn file_references_quote_unicode_spaces_and_shell_syntax_without_submitting() {
    let path = "docs/мій 'file' $(echo bad);.md";
    let reference =
        file_reference(Path::new("/repo"), Path::new(path), Path::new("/repo")).unwrap();
    assert_eq!(shlex::split(&reference).unwrap(), [path]);
    for mode in [TermMode::empty(), TermMode::BRACKETED_PASTE] {
        let bytes = text_bytes(&format!("{reference} "), mode);
        assert!(!bytes.contains(&b'\r'));
        assert!(!bytes.contains(&b'\n'));
        if mode == TermMode::BRACKETED_PASTE {
            assert!(bytes.starts_with(b"\x1b[200~"));
            assert!(bytes.ends_with(b"\x1b[201~"));
        }
    }
}

#[test]
fn file_references_reject_traversal_absolute_and_control_character_paths() {
    for path in [
        "",
        ".",
        "../secret",
        "src/../../secret",
        "/etc/passwd",
        "line\nfile",
        "a\rfile",
        "a\tfile",
        "\x1b[201~bad",
    ] {
        assert!(
            file_reference(Path::new("/repo"), Path::new(path), Path::new("/repo")).is_err(),
            "{path:?}"
        );
    }
    for invalid in ["relative", "/repo/../other", "/repo\n"] {
        assert!(file_reference(Path::new(invalid), Path::new("file"), Path::new("/repo")).is_err());
        assert!(file_reference(Path::new("/repo"), Path::new("file"), Path::new(invalid)).is_err());
    }
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let invalid = Path::new(OsStr::from_bytes(b"bad\xff"));
    assert!(file_reference(Path::new("/repo"), invalid, Path::new("/repo")).is_err());
}

#[test]
fn multiple_file_references_form_one_ordered_quoted_paste_without_enter() {
    let paths = [
        PathBuf::from("src/z.rs"),
        "docs/my notes.md".into(),
        "нотатки.md".into(),
    ];
    let text = file_references(Path::new("/repo"), &paths, Path::new("/repo/src")).unwrap();
    assert_eq!(
        shlex::split(&text).unwrap(),
        ["z.rs", "../docs/my notes.md", "../нотатки.md"]
    );
    assert!(text.ends_with(' '));
    for mode in [TermMode::empty(), TermMode::BRACKETED_PASTE] {
        let bytes = text_bytes(&text, mode);
        assert!(!bytes.contains(&b'\r'));
        assert!(!bytes.contains(&b'\n'));
        if mode == TermMode::BRACKETED_PASTE {
            assert_eq!(bytes, format!("\x1b[200~{text}\x1b[201~").into_bytes());
        }
    }
}

#[test]
fn empty_or_partially_invalid_file_groups_fail_as_a_whole() {
    assert!(matches!(
        file_references(Path::new("/repo"), &[], Path::new("/repo")),
        Err(PasteError::EmptySelection)
    ));
    for invalid in ["../outside", "line\nfile", "/absolute"] {
        let paths = [
            PathBuf::from("good.rs"),
            invalid.into(),
            "also-good.rs".into(),
        ];
        assert!(file_references(Path::new("/repo"), &paths, Path::new("/repo")).is_err());
    }
}
