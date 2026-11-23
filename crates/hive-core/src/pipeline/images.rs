//! Images pasted into a task. Each is a PNG in Hive's data folder, and the task text holds a
//! Markdown reference to it by file name only: nothing that stores or shows the task (the
//! saved state, the run header, a pull request) carries a path into the user's home. An
//! agent gets absolute paths when its prompt is built.

use super::Library;
use crate::state::{Access, hive_dir};
use crate::terminal::paste::{PasteError, save_image_in};
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Folder of the images inside the data folder, and the prefix of a reference.
const FOLDER: &str = "task-images";
/// What stands for an image where its file cannot be reached.
pub(super) const PLACEHOLDER: &str = "[image]";

static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"!\[[^\]\n]*\]\(task-images/([A-Za-z0-9-]+\.png)\)")
        .expect("the reference pattern is a valid regular expression")
});
static FILE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9-]+\.png$").expect("the name pattern is a valid regular expression")
});

/// Where pasted task images are kept.
pub fn directory() -> PathBuf {
    hive_dir().join(FOLDER)
}

/// Blocking: saves clipboard image bytes as a PNG in `directory` and returns the Markdown
/// reference to put into the task.
pub fn save(bytes: &[u8], directory: &Path) -> Result<String, PasteError> {
    fs::create_dir_all(directory).map_err(|source| PasteError::Io {
        operation: "create the folder of the",
        path: directory.to_path_buf(),
        source,
    })?;
    let path = save_image_in(bytes, directory)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PasteError::InvalidPath(path.clone()))?;
    Ok(format!("![image]({FOLDER}/{name})"))
}

/// Every image reference in `text`: the bytes it spans and the file it names.
pub fn references(text: &str) -> Vec<(Range<usize>, &str)> {
    REFERENCE
        .captures_iter(text)
        .filter_map(|found| Some((found.get(0)?.range(), found.get(1)?.as_str())))
        .collect()
}

/// `prompt` as an agent needs it: each reference names the file in `directory`, and the
/// agent is told to look at them.
pub fn for_agent<'a>(prompt: &'a str, directory: &Path) -> Cow<'a, str> {
    let expanded = REFERENCE.replace_all(prompt, |found: &Captures| {
        format!("![image]({})", directory.join(&found[1]).display())
    });
    match expanded {
        Cow::Borrowed(unchanged) => Cow::Borrowed(unchanged),
        Cow::Owned(mut expanded) => {
            expanded.push_str(
                "\nThe task refers to image files by path. Open each of them before you start.\n",
            );
            Cow::Owned(expanded)
        }
    }
}

/// `text` for readers who cannot reach this machine's files, such as a pull request.
pub fn without(text: &str) -> Cow<'_, str> {
    REFERENCE.replace_all(text, PLACEHOLDER)
}

/// Blocking: removes the saved images that no draft or run of `library` refers to. Called
/// once at startup, before a form can hold a reference the library has not saved yet.
///
/// Does nothing for a Hive that only reads the data folder: the files are another's.
pub fn tidy(access: &Access, library: &Library) {
    if !access.can_write() {
        return;
    }
    let tasks = library
        .drafts
        .iter()
        .map(|draft| draft.task.as_str())
        .chain(library.runs.iter().map(|run| run.task()));
    let kept = tasks
        .flat_map(|task| references(task).into_iter().map(|(_, name)| name))
        .collect();
    tidy_in(&directory(), &kept);
}

fn tidy_in(directory: &Path, kept: &HashSet<&str>) {
    // No folder means no image was ever pasted.
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // Only files a reference can name are Hive's to delete.
        let Some(name) = name.to_str().filter(|name| FILE_NAME.is_match(name)) else {
            continue;
        };
        if kept.contains(name) {
            continue;
        }
        if let Err(error) = fs::remove_file(entry.path()) {
            tracing::warn!(path = ?entry.path(), %error, "could not remove an unused task image");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{directory, for_agent, references, save, tidy_in, without};
    use crate::terminal::paste::PasteError;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("hive-task-images-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    #[test]
    fn save_writes_a_png_and_returns_a_reference_references_finds() {
        let folder = folder("save");
        let reference = save(&png(), &folder).unwrap();
        let text = format!("See {reference} here");
        let found = references(&text);
        assert_eq!(found.len(), 1);
        let (range, name) = &found[0];
        assert_eq!(&text[range.clone()], reference);
        assert_eq!(
            image::guess_format(&fs::read(folder.join(name)).unwrap()).unwrap(),
            image::ImageFormat::Png
        );
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn save_rejects_corrupt_and_unsupported_bytes_without_leaving_a_file() {
        let folder = folder("reject");
        assert!(matches!(
            save(b"not an image", &folder),
            Err(PasteError::Decode(_))
        ));
        let mut gif = Vec::new();
        image::RgbaImage::new(1, 1)
            .write_to(&mut std::io::Cursor::new(&mut gif), image::ImageFormat::Gif)
            .unwrap();
        assert!(matches!(
            save(&gif, &folder),
            Err(PasteError::UnsupportedFormat)
        ));
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_reference_written_by_an_earlier_version_still_parses() {
        let text = "Fix it ![image](task-images/hive-clipboard-4242-0.png) please";
        assert_eq!(references(text), [(7..54, "hive-clipboard-4242-0.png")]);
    }

    #[test]
    fn other_markdown_images_and_paths_out_of_the_folder_are_not_references() {
        for text in [
            "![shot](https://example.com/a.png)",
            "![shot](task-images/../secret.png)",
            "![shot](task-images/nested/a.png)",
            "![shot](task-images/a.jpg)",
            "[link](task-images/a.png)",
            "![broken\n](task-images/a.png)",
        ] {
            assert!(references(text).is_empty(), "{text}");
        }
    }

    #[test]
    fn for_agent_expands_references_and_leaves_plain_prompts_borrowed() {
        let plain = "User task:\nFix the login";
        assert!(matches!(
            for_agent(plain, Path::new("/data/task-images")),
            std::borrow::Cow::Borrowed(text) if text == plain
        ));
        assert_eq!(
            for_agent(
                "Fix ![image](task-images/a-1.png)\n",
                Path::new("/data/task-images")
            ),
            "Fix ![image](/data/task-images/a-1.png)\n\nThe task refers to image files by path. Open each of them before you start.\n"
        );
    }

    #[test]
    fn without_replaces_references_and_keeps_other_text() {
        assert_eq!(
            without("A ![image](task-images/a-1.png) and ![x](https://e.com/b.png)"),
            "A [image] and ![x](https://e.com/b.png)"
        );
    }

    #[test]
    fn tidy_removes_only_unreferenced_images() {
        let folder = folder("tidy");
        fs::create_dir_all(&folder).unwrap();
        for name in ["kept-1.png", "unused-2.png", "notes.txt", "photo.jpg"] {
            fs::write(folder.join(name), b"x").unwrap();
        }
        tidy_in(&folder, &HashSet::from(["kept-1.png"]));
        let mut left: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["kept-1.png", "notes.txt", "photo.jpg"]);
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn tidy_without_a_folder_does_nothing() {
        tidy_in(&folder("missing"), &HashSet::new());
    }

    #[test]
    fn images_live_in_their_own_folder_of_the_data_folder() {
        assert!(directory().ends_with("task-images"));
    }
}
