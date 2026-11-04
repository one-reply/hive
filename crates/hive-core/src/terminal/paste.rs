//! Clipboard images become private temporary PNGs; terminals receive only their quoted paths.

use super::TermMode;
use crate::host::quote;
use image::{ImageFormat, ImageReader};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Cursor, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_IMAGE: AtomicU64 = AtomicU64::new(0);

/// A clipboard image could not be decoded, saved, or safely referenced in a terminal.
#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    /// The clipboard bytes are not a valid supported image.
    #[error("could not decode clipboard image: {0}")]
    Decode(#[from] image::ImageError),
    /// Only screenshot and common photo formats are accepted.
    #[error("unsupported clipboard image format; use PNG, JPEG, or TIFF")]
    UnsupportedFormat,
    /// Terminal paths must be absolute UTF-8 without control characters.
    #[error("cannot paste this temporary image path: {0}")]
    InvalidPath(PathBuf),
    /// An image could not be encoded to the destination file.
    #[error("could not encode clipboard image at {path}: {source}")]
    Encode {
        /// Destination PNG path.
        path: PathBuf,
        /// Encoder failure.
        #[source]
        source: image::ImageError,
    },
    /// A filesystem operation failed.
    #[error("could not {operation} clipboard image at {path}: {source}")]
    Io {
        /// Failed filesystem operation.
        operation: &'static str,
        /// Affected PNG path.
        path: PathBuf,
        /// Underlying filesystem failure.
        #[source]
        source: io::Error,
    },
}

/// Decode PNG/JPEG/TIFF with the decoder's default limits and save a unique temporary PNG.
///
/// This blocks and must run off the UI thread. Files survive the terminal session;
/// the operating system may remove them when it cleans its temporary directory.
pub fn save_image(bytes: &[u8]) -> Result<PathBuf, PasteError> {
    save_image_in(bytes, &std::env::temp_dir())
}

fn save_image_in(bytes: &[u8], directory: &Path) -> Result<PathBuf, PasteError> {
    let format = image::guess_format(bytes)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Tiff
    ) {
        return Err(PasteError::UnsupportedFormat);
    }
    let image = ImageReader::with_format(Cursor::new(bytes), format).decode()?;
    let (path, file) = create_image_file(directory)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        image
            .write_to(&mut writer, ImageFormat::Png)
            .map_err(|source| PasteError::Encode {
                path: path.clone(),
                source,
            })?;
        writer.flush().map_err(|source| PasteError::Io {
            operation: "flush",
            path: path.clone(),
            source,
        })
    })();
    if let Err(error) = result {
        if let Err(cleanup) = fs::remove_file(&path) {
            tracing::warn!(?path, %cleanup, "could not remove incomplete clipboard image");
        }
        return Err(error);
    }
    Ok(path)
}

fn create_image_file(directory: &Path) -> Result<(PathBuf, File), PasteError> {
    loop {
        let sequence = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "hive-clipboard-{}-{sequence}.png",
            std::process::id()
        ));
        quoted_path(&path)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(PasteError::Io {
                    operation: "create",
                    path,
                    source,
                });
            }
        }
    }
}

/// Quote one absolute path for shell input without allowing terminal control characters.
pub fn quoted_path(path: &Path) -> Result<String, PasteError> {
    let text = path
        .to_str()
        .filter(|text| path.is_absolute() && !text.chars().any(char::is_control))
        .ok_or_else(|| PasteError::InvalidPath(path.to_path_buf()))?;
    Ok(quote(text))
}

/// Encode text using the terminal's paste mode, without adding a submit key.
pub fn text_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests;
