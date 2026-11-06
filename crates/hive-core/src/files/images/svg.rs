use super::*;
use resvg::{tiny_skia, usvg};
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

pub(super) fn decode(path: &Path, bytes: &[u8], viewport: (u32, u32)) -> Result<Decoded, Error> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return Err(limit(path, "the preview viewport is empty"));
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(limit(
            path,
            "compressed SVG is not supported; use an uncompressed .svg file",
        ));
    }
    static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        // macOS does not ship the default Times New Roman on every installation.
        fonts.set_serif_family("Times");
        fonts.set_sans_serif_family("Helvetica");
        fonts.set_monospace_family("Menlo");
        Arc::new(fonts)
    });
    let omitted = Arc::new(AtomicBool::new(false));
    let embedded_pixels = Arc::new(AtomicU64::new(0));
    let select_font = usvg::FontResolver::default_font_selector();
    let options = usvg::Options {
        font_resolver: usvg::FontResolver {
            select_font: Box::new(move |font, database| {
                if database.is_empty() {
                    *database = FONTS.clone();
                }
                select_font(font, database)
            }),
            ..Default::default()
        },
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_string: Box::new({
                let omitted = omitted.clone();
                move |_, _| {
                    omitted.store(true, Ordering::Relaxed);
                    None
                }
            }),
            resolve_data: Box::new({
                let omitted = omitted.clone();
                move |_, data, _| {
                    let result = embedded(data, &embedded_pixels);
                    if result.is_none() {
                        omitted.store(true, Ordering::Relaxed);
                    }
                    result
                }
            }),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|source| Error::Svg {
        path: path.into(),
        source,
    })?;
    let size = tree.size();
    let scale = (viewport.0 as f32 / size.width())
        .min(viewport.1 as f32 / size.height())
        .min(MAX_IMAGE_SIDE as f32 / size.width())
        .min(MAX_IMAGE_SIDE as f32 / size.height())
        .min((MAX_IMAGE_PIXELS as f32 / (size.width() * size.height())).sqrt());
    let width = (size.width() * scale).floor().max(1.) as u32;
    let height = (size.height() * scale).floor().max(1.) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| limit(path, "could not allocate the SVG preview"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.take();
    // tiny-skia returns premultiplied alpha; the cross-crate contract is straight RGBA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    Ok(Decoded {
        width,
        height,
        pixels,
        omitted_resources: omitted.load(Ordering::Relaxed),
    })
}

fn embedded(data: Arc<Vec<u8>>, budget: &AtomicU64) -> Option<usvg::ImageKind> {
    if data.len() > MAX_IMAGE_BYTES {
        return None;
    }
    // Validate embedded rasters before resvg's own decoder allocates their pixels.
    // Nested SVG resources are omitted: their recursive allocations have no shared budget.
    let format = image::guess_format(&data).ok()?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return None;
    }
    let image = raster(&data, format).ok()?;
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if budget
        .fetch_add(pixels, Ordering::Relaxed)
        .saturating_add(pixels)
        > MAX_IMAGE_PIXELS
    {
        return None;
    }
    Some(match format {
        ImageFormat::Png => usvg::ImageKind::PNG(data),
        ImageFormat::Jpeg => usvg::ImageKind::JPEG(data),
        ImageFormat::Gif => usvg::ImageKind::GIF(data),
        ImageFormat::WebP => usvg::ImageKind::WEBP(data),
        _ => return None,
    })
}
