//! GPU rasterization of chapbook display lists, via vello and wgpu.
//!
//! The second backend over the same paint-neutral ops the CPU renderer
//! consumes — which is the point: a contract with one implementation is a
//! data structure, not a seam. Nothing is re-shaped on the way here. A
//! `GlyphRun` names a face in the session's font database and carries
//! final glyph ids and positions, and vello's glyph API takes exactly
//! that, so the translation is a transcription rather than a rebuild.
//!
//! Rendering is offscreen: vello writes into a storage texture and the
//! result is copied back to RGBA bytes. A windowed shell would keep the
//! device and render to a surface instead; the scene building is the same
//! either way.

use std::collections::HashMap;
use std::sync::Arc;

use vello::kurbo::{Affine, Rect as KRect};
use vello::peniko::{
    Blob, Color, Fill, FontData, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
};
use vello::wgpu;
use vello::{AaConfig, Glyph, RenderParams, Renderer, RendererOptions, Scene};

use chapbook_core::{PixelFormat, Rgba, Rotation, Size};
use chapbook_paint::{DisplayList, DisplayOp, ImageStore};

/// A rasterized page in straight (non-premultiplied) RGBA8.
#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RenderedPage {
    /// Apply panel color policy — the same conversion the CPU backend
    /// gets, because it belongs to the target rather than the rasterizer.
    pub fn quantize(&mut self, format: PixelFormat) {
        chapbook_paint::quantize(&mut self.rgba, self.width, self.height, format);
    }

    /// Turn the page for a panel mounted in another orientation. Quarter
    /// turns swap the page's dimensions.
    pub fn rotate(self, rotation: Rotation) -> RenderedPage {
        let rgba = chapbook_paint::rotate(&self.rgba, self.width, self.height, rotation);
        let (width, height) = if rotation.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        };
        RenderedPage {
            width,
            height,
            rgba,
        }
    }

    /// The pixel at `(x, y)`, or `None` outside the page.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        self.rgba[i..i + 4].try_into().ok()
    }
}

#[derive(Debug)]
pub enum VelloError {
    /// No wgpu adapter at all — no GPU, and no software fallback either.
    NoAdapter,
    Device(String),
    Render(String),
    /// The page is larger than this device will allocate.
    TooLarge {
        width: u32,
        height: u32,
    },
}

impl std::fmt::Display for VelloError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VelloError::NoAdapter => write!(f, "no wgpu adapter available"),
            VelloError::Device(e) => write!(f, "wgpu device: {e}"),
            VelloError::Render(e) => write!(f, "vello render: {e}"),
            VelloError::TooLarge { width, height } => {
                write!(
                    f,
                    "page {width}x{height} exceeds the device's texture limit"
                )
            }
        }
    }
}

impl std::error::Error for VelloError {}

/// Face bytes by font id.
///
/// The paint seam hands out a `FontSystem`, and reading a face out of its
/// database copies the bytes, so every backend ends up keeping something
/// like this. Shared by the offscreen and windowed paths.
#[derive(Default)]
pub struct FontCache {
    faces: HashMap<cosmic_text::fontdb::ID, FontData>,
}

impl FontCache {
    fn data(
        &mut self,
        id: cosmic_text::fontdb::ID,
        fonts: &mut cosmic_text::FontSystem,
    ) -> Option<&FontData> {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.faces.entry(id) {
            let (bytes, index) = fonts
                .db_mut()
                .with_face_data(id, |data, index| (data.to_vec(), index))?;
            slot.insert(FontData::new(Blob::new(Arc::new(bytes)), index));
        }
        self.faces.get(&id)
    }
}

/// Translate display ops into a vello scene.
///
/// The interesting half, and deliberately free-standing: the offscreen
/// renderer and a windowed shell build the same scene and differ only in
/// where they send it.
pub fn build_scene(
    dl: &DisplayList,
    cache: &mut FontCache,
    fonts: &mut cosmic_text::FontSystem,
    images: &ImageStore,
    scale: f32,
) -> Scene {
    let mut scene = Scene::new();
    // Device pixels are a transform here, not a re-layout: vello is a
    // vector renderer, so scale applies to the whole scene.
    let to_device = Affine::scale(scale as f64);

    for op in &dl.ops {
        match op {
            DisplayOp::FillRect { rect, color } => {
                scene.fill(
                    Fill::NonZero,
                    to_device,
                    peniko_color(*color),
                    None,
                    &KRect::new(
                        rect.min_x() as f64,
                        rect.min_y() as f64,
                        rect.max_x() as f64,
                        rect.max_y() as f64,
                    ),
                );
            }
            DisplayOp::Image { resource, dest } => {
                let Some(stored) = images.get(*resource) else {
                    continue;
                };
                let image = ImageData {
                    data: Blob::new(Arc::new(stored.rgba.clone())),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::Alpha,
                    width: stored.width,
                    height: stored.height,
                };
                let placement = Affine::translate((dest.min_x() as f64, dest.min_y() as f64))
                    * Affine::scale_non_uniform(
                        dest.size.w as f64 / stored.width.max(1) as f64,
                        dest.size.h as f64 / stored.height.max(1) as f64,
                    );
                scene.draw_image(&ImageBrush::new(image), to_device * placement);
            }
            DisplayOp::GlyphRun {
                font,
                font_size,
                font_weight: _,
                color,
                origin,
                glyphs,
            } => {
                let Some(font_data) = cache.data(*font, fonts) else {
                    continue;
                };
                // Glyph positions are baseline-relative, exactly as the
                // run carries them; the run origin is the baseline.
                let run = to_device * Affine::translate((origin.x as f64, origin.y as f64));
                scene
                    .draw_glyphs(font_data)
                    .font_size(*font_size)
                    .brush(peniko_color(*color))
                    .transform(run)
                    .draw(
                        Fill::NonZero,
                        glyphs.iter().map(|g| Glyph {
                            id: u32::from(g.id),
                            x: g.x,
                            y: g.y,
                        }),
                    );
            }
        }
    }
    scene
}

/// Holds the GPU device and the caches that outlive one page.
pub struct VelloRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    fonts: FontCache,
    max_texture: u32,
}

impl VelloRenderer {
    /// Acquire a device and compile vello's pipelines. Prefers real
    /// hardware and falls back to whatever adapter exists (lavapipe and
    /// friends), so this works on a machine with no GPU at all.
    pub fn new() -> Result<Self, VelloError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .or_else(|_| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                force_fallback_adapter: true,
                compatible_surface: None,
            }))
        })
        .map_err(|_| VelloError::NoAdapter)?;

        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("chapbook-vello"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| VelloError::Device(e.to_string()))?;

        let renderer = Renderer::new(
            &device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: vello::AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|e| VelloError::Render(e.to_string()))?;

        Ok(VelloRenderer {
            device,
            queue,
            renderer,
            fonts: FontCache::default(),
            max_texture: limits.max_texture_dimension_2d,
        })
    }

    /// Rasterize a display list at `scale` device pixels per CSS px.
    ///
    /// `fonts` is the session's font database — glyph runs name faces in
    /// it — and `images` resolves image ops. Both come from
    /// `Session::paint_resources`.
    pub fn render(
        &mut self,
        dl: &DisplayList,
        fonts: &mut cosmic_text::FontSystem,
        images: &ImageStore,
        scale: f32,
    ) -> Result<RenderedPage, VelloError> {
        let width = (dl.size.w * scale).round().max(1.0) as u32;
        let height = (dl.size.h * scale).round().max(1.0) as u32;
        if width > self.max_texture || height > self.max_texture {
            return Err(VelloError::TooLarge { width, height });
        }

        let scene = self.build_scene(dl, fonts, images, scale);
        self.rasterize(&scene, width, height)
    }

    /// The scene for one page, using this renderer's font cache.
    pub fn build_scene(
        &mut self,
        dl: &DisplayList,
        fonts: &mut cosmic_text::FontSystem,
        images: &ImageStore,
        scale: f32,
    ) -> Scene {
        build_scene(dl, &mut self.fonts, fonts, images, scale)
    }

    fn rasterize(
        &mut self,
        scene: &Scene,
        width: u32,
        height: u32,
    ) -> Result<RenderedPage, VelloError> {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chapbook-page"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &view,
                &RenderParams {
                    // The display list's first op is the page ground, so
                    // the base color only shows through where the page
                    // deliberately doesn't paint.
                    base_color: Color::TRANSPARENT,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| VelloError::Render(e.to_string()))?;

        self.read_back(&target, width, height)
    }

    /// Copy the rendered texture back to host memory. Buffer rows are
    /// padded to wgpu's 256-byte alignment, so the copy back out is
    /// row-by-row rather than one memcpy.
    fn read_back(
        &mut self,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Result<RenderedPage, VelloError> {
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;

        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chapbook-readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("chapbook-readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| VelloError::Device(e.to_string()))?;
        rx.recv()
            .map_err(|e| VelloError::Device(e.to_string()))?
            .map_err(|e| VelloError::Device(e.to_string()))?;

        let mapped = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            rgba.extend_from_slice(&mapped[start..start + unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();

        Ok(RenderedPage {
            width,
            height,
            rgba,
        })
    }
}

/// Page size in device pixels, for callers sizing a target themselves.
pub fn device_size(size: Size, scale: f32) -> (u32, u32) {
    (
        (size.w * scale).round().max(1.0) as u32,
        (size.h * scale).round().max(1.0) as u32,
    )
}

fn peniko_color(color: Rgba) -> Color {
    Color::from_rgba8(color.r, color.g, color.b, color.a)
}

/// A vello renderer bound to a window's surface.
///
/// The offscreen path above renders into a texture and copies the pixels
/// back, which is right for tests and wrong for a viewer: a windowed shell
/// would be paying for a GPU render, a readback, and a CPU blit. This
/// renders into vello's intermediate target and blits that to the
/// swapchain, so pixels never leave the device.
///
/// Panel policy is deliberately absent here. `PixelFormat` dithering and
/// `Rotation` are properties of an e-ink target, and applying them on this
/// path means a compute pass or a scene transform rather than the row
/// operations in chapbook-paint — worth doing when a GPU e-ink shell
/// exists, not before.
pub struct VelloWindow {
    context: vello::util::RenderContext,
    surface: vello::util::RenderSurface<'static>,
    renderer: Renderer,
    fonts: FontCache,
}

impl VelloWindow {
    /// Bind to a window. `target` is anything wgpu accepts as a surface
    /// target for the `'static` lifetime — an `Arc<Window>`, in practice.
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, VelloError> {
        let mut context = vello::util::RenderContext::new();
        let surface = pollster::block_on(context.create_surface(
            target,
            width.max(1),
            height.max(1),
            wgpu::PresentMode::AutoVsync,
        ))
        .map_err(|e| VelloError::Device(e.to_string()))?;

        let device = &context.devices[surface.dev_id];
        let renderer = Renderer::new(
            &device.device,
            RendererOptions {
                use_cpu: false,
                antialiasing_support: vello::AaSupport::area_only(),
                num_init_threads: None,
                pipeline_cache: None,
            },
        )
        .map_err(|e| VelloError::Render(e.to_string()))?;

        Ok(VelloWindow {
            context,
            surface,
            renderer,
            fonts: FontCache::default(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.context
            .resize_surface(&mut self.surface, width.max(1), height.max(1));
    }

    /// Draw a page and present it.
    pub fn present(
        &mut self,
        dl: &DisplayList,
        fonts: &mut cosmic_text::FontSystem,
        images: &ImageStore,
        scale: f32,
        background: Rgba,
    ) -> Result<(), VelloError> {
        let scene = build_scene(dl, &mut self.fonts, fonts, images, scale);
        let device = &self.context.devices[self.surface.dev_id];

        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            // The swapchain needs rebuilding (a resize we haven't been
            // told about yet, or a lost device). Reconfigure and let the
            // shell ask for another frame rather than failing the read.
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                self.context.configure_surface(&self.surface);
                return Ok(());
            }
            // Nothing to draw into right now; not an error.
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
            other => {
                return Err(VelloError::Render(format!(
                    "surface unavailable: {other:?}"
                )))
            }
        };
        self.renderer
            .render_to_texture(
                &device.device,
                &device.queue,
                &scene,
                &self.surface.target_view,
                &RenderParams {
                    // Letterboxing: the page rarely fills the window
                    // exactly, and the ground the display list paints only
                    // covers the page.
                    base_color: peniko_color(background),
                    width: self.surface.config.width,
                    height: self.surface.config.height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(|e| VelloError::Render(e.to_string()))?;

        let mut encoder = device
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("chapbook-blit"),
            });
        self.surface.blitter.copy(
            &device.device,
            &mut encoder,
            &self.surface.target_view,
            &frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        device.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }
}
