//! Bounded image loading and static previews. All operations run off the UI thread.
use super::{FileError, read_bytes, resolve};
use crate::host::Host;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use std::path::{Path, PathBuf};

mod svg;

/// Maximum compressed image size, including SVG source.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// Maximum decoded width or height.
pub const MAX_IMAGE_SIDE: u32 = 8192;
/// Maximum decoded pixel count (128 MB of RGBA pixels).
pub const MAX_IMAGE_PIXELS: u64 = 32_000_000;

/// A supported document image format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// SVG with an optional editable source buffer.
    Svg,
    /// One of the supported raster codecs.
    Raster(ImageFormat),
}

/// Recognize the supported, case-insensitive filename extensions.
pub fn format(path: &Path) -> Option<Format> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "svg" => Format::Svg,
        "png" => Format::Raster(ImageFormat::Png),
        "jpg" | "jpeg" => Format::Raster(ImageFormat::Jpeg),
        "webp" => Format::Raster(ImageFormat::WebP),
        "gif" => Format::Raster(ImageFormat::Gif),
        "bmp" => Format::Raster(ImageFormat::Bmp),
        "tif" | "tiff" => Format::Raster(ImageFormat::Tiff),
        "ico" => Format::Raster(ImageFormat::Ico),
        _ => return None,
    })
}

/// A failed preview, retaining the affected path and decoder's explanation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid or unsupported raster data.
    #[error("Cannot preview {path}: {source}")]
    Raster {
        /// Affected document.
        path: PathBuf,
        /// Codec failure.
        #[source]
        source: image::ImageError,
    },
    /// Invalid SVG source.
    #[error("Cannot preview {path}: {source}")]
    Svg {
        /// Affected document.
        path: PathBuf,
        /// Parser failure.
        #[source]
        source: resvg::usvg::Error,
    },
    /// The preview would exceed a resource limit.
    #[error("Cannot preview {path}: {reason}")]
    Limit {
        /// Affected document.
        path: PathBuf,
        /// User-facing limit.
        reason: &'static str,
    },
}

/// A single, oriented frame in straight-alpha RGBA8, independent of GPUI.
pub struct Decoded {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    omitted_resources: bool,
}

impl Decoded {
    /// Decoded width and height in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether SVG resources were skipped because they were external or exceeded limits.
    pub fn omitted_resources(&self) -> bool {
        self.omitted_resources
    }

    /// Consume the image and return tightly packed RGBA8 bytes.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

/// Read a local regular image without bypassing the editor's worktree/path checks.
pub fn read(host: &Host, root: &Path, relative: &Path) -> Result<Vec<u8>, FileError> {
    let path = resolve(host, root, relative)?;
    read_bytes(
        &path,
        MAX_IMAGE_BYTES,
        "images larger than 20 MiB are not supported",
    )
}

/// Decode the first frame/page with EXIF orientation and resource limits.
/// SVGs fit `viewport` in device pixels; rasters retain their original size.
pub fn decode(
    path: &Path,
    format: Format,
    bytes: &[u8],
    viewport: (u32, u32),
) -> Result<Decoded, Error> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(limit(path, "images larger than 20 MiB are not supported"));
    }
    match format {
        Format::Svg => svg::decode(path, bytes, viewport),
        Format::Raster(format) => {
            let image = raster(bytes, format).map_err(|source| Error::Raster {
                path: path.into(),
                source,
            })?;
            Ok(Decoded {
                width: image.width(),
                height: image.height(),
                pixels: image.into_rgba8().into_raw(),
                omitted_resources: false,
            })
        }
    }
}

fn raster(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, image::ImageError> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS * 4);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
        return Err(image::ImageError::Limits(
            image::error::LimitError::from_kind(image::error::LimitErrorKind::DimensionError),
        ));
    }
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn limit(path: &Path, reason: &'static str) -> Error {
    Error::Limit {
        path: path.into(),
        reason,
    }
}

#[cfg(test)]
mod tests;
