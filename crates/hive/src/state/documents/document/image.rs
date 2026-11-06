use super::*;
use hive_core::files::images::{self, Format};

const SVG_RENDER_DELAY: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SvgMode {
    Preview,
    Code,
}

pub(crate) enum Content {
    Text,
    Raster(ImagePreview),
    Svg {
        mode: SvgMode,
        preview: ImagePreview,
        source_error: Option<String>,
    },
}

impl Content {
    pub fn new(path: &std::path::Path, recovered: bool) -> Self {
        match images::format(path) {
            Some(Format::Svg) => Self::Svg {
                mode: if recovered {
                    SvgMode::Code
                } else {
                    SvgMode::Preview
                },
                preview: ImagePreview::default(),
                source_error: None,
            },
            Some(Format::Raster(_)) if !recovered => Self::Raster(ImagePreview::default()),
            // A recovered text draft must remain editable even after an extension change.
            _ => Self::Text,
        }
    }

    pub fn preview(&self) -> Option<&ImagePreview> {
        match self {
            Self::Text => None,
            Self::Raster(preview) | Self::Svg { preview, .. } => Some(preview),
        }
    }

    fn preview_mut(&mut self) -> Option<&mut ImagePreview> {
        match self {
            Self::Text => None,
            Self::Raster(preview) | Self::Svg { preview, .. } => Some(preview),
        }
    }

    pub fn shows_image(&self) -> bool {
        matches!(
            self,
            Self::Raster(_)
                | Self::Svg {
                    mode: SvgMode::Preview,
                    ..
                }
        )
    }

    pub fn is_raster(&self) -> bool {
        matches!(self, Self::Raster(_))
    }
}

pub(crate) enum PreviewState {
    Loading,
    Ready {
        image: Arc<RenderImage>,
        omitted_resources: bool,
    },
    Failed(String),
}

pub(crate) struct ImagePreview {
    pub state: PreviewState,
    disk: Option<Arc<[u8]>>,
    input: Option<Arc<[u8]>>,
    generation: u64,
    viewport: (u32, u32),
    task: Option<Task<()>>,
}

impl Default for ImagePreview {
    fn default() -> Self {
        Self {
            state: PreviewState::Loading,
            disk: None,
            input: None,
            generation: 0,
            viewport: (0, 0),
            task: None,
        }
    }
}

impl ImagePreview {
    fn grow_viewport(&mut self, viewport: (u32, u32)) -> bool {
        // Shared documents can appear in differently sized panels; never resize them back and forth.
        let viewport = (
            self.viewport.0.max(viewport.0),
            self.viewport.1.max(viewport.1),
        );
        let changed = self.viewport != viewport;
        self.viewport = viewport;
        changed
    }

    fn invalidate(&mut self) -> Option<Arc<RenderImage>> {
        self.generation += 1;
        self.task = None;
        self.input = None;
        match std::mem::replace(&mut self.state, PreviewState::Loading) {
            PreviewState::Ready { image, .. } => Some(image),
            _ => None,
        }
    }

    fn clear(&mut self, cx: &mut App) {
        if let Some(image) = self.invalidate() {
            // The current window is temporarily absent from App while its entities update.
            cx.defer(move |cx| cx.drop_image(image, None));
        }
    }

    fn complete(
        &mut self,
        generation: u64,
        result: Result<(Arc<RenderImage>, bool), String>,
    ) -> bool {
        if self.generation != generation {
            return false;
        }
        self.state = match result {
            Ok((image, omitted_resources)) => PreviewState::Ready {
                image,
                omitted_resources,
            },
            Err(error) => {
                tracing::warn!(%error, "image preview failed");
                PreviewState::Failed(error)
            }
        };
        true
    }
}

impl Document {
    pub fn resize_image(&mut self, size: Size<Pixels>, scale: f32, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Svg { .. }) {
            return;
        }
        let viewport = device_viewport(size, scale);
        if let Some(preview) = self.content.preview_mut()
            && preview.grow_viewport(viewport)
        {
            preview.clear(cx);
            self.render_image(cx);
        }
    }

    pub fn set_svg_mode(&mut self, mode: SvgMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Content::Svg { mode: current, .. } = &mut self.content {
            if *current == mode || (mode == SvgMode::Code && self.base.is_none()) {
                return;
            }
            *current = mode;
            if mode == SvgMode::Code {
                self.editor.read(cx).focus_handle(cx).focus(window, cx);
            } else {
                self.render_image(cx);
            }
            cx.notify();
        }
    }

    pub(super) fn invalidate_image(&mut self, cx: &mut App) {
        if let Some(preview) = self.content.preview_mut() {
            preview.clear(cx);
        }
    }

    pub(super) fn refresh_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.loading = true;
        let (target, path) = (self.target.clone(), self.path.clone());
        let svg = matches!(self.content, Content::Svg { .. });
        self._read = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = images::read(&target.host, &target.path, &path)?;
                    let source = svg.then(|| Snapshot::parse(&path, &bytes));
                    Ok::<_, files::FileError>((Arc::<[u8]>::from(bytes), source))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok((bytes, source)) => {
                        this.missing = false;
                        if matches!(this.error, Some(DocumentError::Read(_))) {
                            this.error = None;
                            cx.notify();
                        }
                        if let Some(source) = source {
                            match source {
                                Ok(snapshot) => {
                                    if let Content::Svg { source_error, .. } = &mut this.content
                                        && source_error.take().is_some()
                                    {
                                        cx.notify();
                                    }
                                    this.accept_snapshot(snapshot, window, cx);
                                }
                                Err(error) => {
                                    let message = error.to_string();
                                    if let Content::Svg { source_error, .. } = &mut this.content
                                        && source_error.as_ref() != Some(&message)
                                    {
                                        tracing::warn!(%error, "SVG source is not editable");
                                        *source_error = Some(message);
                                        cx.notify();
                                    }
                                    if !this.dirty {
                                        this.base = None;
                                    }
                                }
                            }
                        }
                        if let Some(preview) = this.content.preview_mut() {
                            preview.disk = Some(bytes);
                        }
                        this.render_image(cx);
                    }
                    Err(error) => {
                        this.read_failed(error, cx);
                        if !this.dirty {
                            this.invalidate_image(cx);
                            if let Some(preview) = this.content.preview_mut() {
                                preview.disk = None;
                                preview.state =
                                    PreviewState::Failed("Image is unavailable on disk".into());
                            }
                            cx.notify();
                        }
                    }
                }
            });
        }));
    }

    pub(super) fn render_image(&mut self, cx: &mut Context<Self>) {
        if !self.content.shows_image() || self.path_busy {
            return;
        }
        let Some(format) = images::format(&self.path) else {
            return;
        };
        let source = if format == Format::Svg && self.base.is_some() {
            Some(Arc::<[u8]>::from(self.text(cx).into_bytes()))
        } else {
            self.content.preview().and_then(|p| p.disk.clone())
        };
        let Some(source) = source else {
            return;
        };
        let Some(preview) = self.content.preview_mut() else {
            return;
        };
        let viewport = preview.viewport;
        if format == Format::Svg && (viewport.0 == 0 || viewport.1 == 0) {
            return;
        }
        if preview.input.as_ref() == Some(&source) {
            return;
        }
        preview.clear(cx);
        preview.input = Some(source.clone());
        let generation = preview.generation;
        let path = self.path.clone();
        preview.task = Some(cx.spawn(async move |this, cx| {
            if format == Format::Svg {
                cx.background_executor().timer(SVG_RENDER_DELAY).await;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let decoded = images::decode(&path, format, &source, viewport)
                        .map_err(|e| e.to_string())?;
                    let (width, height) = decoded.dimensions();
                    let omitted_resources = decoded.omitted_resources();
                    let mut pixels = decoded.into_pixels();
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    let buffer = ::image::RgbaImage::from_raw(width, height, pixels)
                        .expect("decoded image guarantees exactly width * height RGBA pixels");
                    let image = Arc::new(RenderImage::new(vec![::image::Frame::new(buffer)]));
                    Ok::<_, String>((image, omitted_resources))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(preview) = this.content.preview_mut() else {
                    return;
                };
                if preview.complete(generation, result) {
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
}

fn device_viewport(size: Size<Pixels>, scale: f32) -> (u32, u32) {
    (
        ((f32::from(size.width) * scale).ceil() as u32).min(images::MAX_IMAGE_SIDE),
        ((f32::from(size.height) * scale).ceil() as u32).min(images::MAX_IMAGE_SIDE),
    )
}

#[cfg(test)]
mod tests {
    use super::{Content, ImagePreview, PreviewState, SvgMode, device_viewport};
    use gpui_kit::{RenderImage, px, size};
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn svg_viewport_tracks_retina_pixels_and_fractional_bounds() {
        assert_eq!(device_viewport(size(px(100.25), px(40.)), 2.), (201, 80));
        assert_eq!(device_viewport(size(px(0.), px(0.)), 2.), (0, 0));
        assert_eq!(device_viewport(size(px(9000.), px(100.)), 2.), (8192, 200));
    }

    #[test]
    fn shared_svg_panels_do_not_restart_rendering_at_each_others_size() {
        let mut preview = ImagePreview::default();
        assert!(preview.grow_viewport((1000, 500)));
        assert!(preview.grow_viewport((500, 1000)));
        assert_eq!(preview.viewport, (1000, 1000));
        assert!(!preview.grow_viewport((1000, 500)));
        assert!(!preview.grow_viewport((500, 1000)));
        assert!(!preview.grow_viewport((0, 0)));
        assert!(preview.grow_viewport((1200, 1000)));
    }

    #[test]
    fn new_images_preview_but_recovered_svg_stays_in_code() {
        assert!(Content::new(Path::new("a.PNG"), false).shows_image());
        assert!(Content::new(Path::new("a.svg"), false).shows_image());
        assert!(matches!(
            Content::new(Path::new("a.svg"), true),
            Content::Svg {
                mode: SvgMode::Code,
                ..
            }
        ));
        assert!(matches!(
            Content::new(Path::new("renamed.png"), true),
            Content::Text
        ));
        assert!(matches!(
            Content::new(Path::new("a.rs"), false),
            Content::Text
        ));
    }

    #[test]
    fn edits_moves_and_deletion_invalidate_old_render_results() {
        let mut preview = ImagePreview::default();
        let original = preview.generation;
        preview.input = Some(Arc::from(&b"original SVG"[..]));
        preview.invalidate();
        assert!(preview.input.is_none());
        assert!(!preview.complete(original, Err("stale failure".into())));
        assert!(matches!(preview.state, PreviewState::Loading));
        assert!(preview.complete(preview.generation, Err("current failure".into())));
        assert!(
            matches!(&preview.state, PreviewState::Failed(error) if error == "current failure")
        );
    }

    #[test]
    fn invalidation_releases_the_previous_rendered_image() {
        let mut preview = ImagePreview::default();
        let image = Arc::new(RenderImage::new(vec![::image::Frame::new(
            ::image::RgbaImage::new(1, 1),
        )]));
        let weak = Arc::downgrade(&image);
        preview.complete(preview.generation, Ok((image, false)));
        drop(preview.invalidate());
        assert!(weak.upgrade().is_none());
    }
}
