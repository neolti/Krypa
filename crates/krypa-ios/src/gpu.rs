//! wgpu on a `CAMetalLayer` owned by the Swift view, and egui-wgpu painting into it. Apple
//! targets only. Errors are plain strings: the shell can only log them.

use std::ffi::c_void;

use egui::FullOutput;
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};

use crate::geometry::Viewport;

pub struct Gpu {
    // Field order is drop order: everything that uses the layer goes before the instance.
    renderer: Renderer,
    queue: wgpu::Queue,
    device: wgpu::Device,
    config: wgpu::SurfaceConfiguration,
    surface: wgpu::Surface<'static>,
    _instance: wgpu::Instance,
}

impl Gpu {
    /// Creates the surface on `layer` and the device behind it.
    ///
    /// # Safety
    /// `layer` must point to a live `CAMetalLayer` that outlives the returned value and is
    /// not drawn to by anything else.
    pub unsafe fn new(layer: *mut c_void, viewport: &Viewport) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        // SAFETY: the caller guarantees `layer` is a live `CAMetalLayer` that outlives the
        // surface; `surface` is stored next to `_instance` and dropped before the caller
        // releases the layer (`krypa_view_destroy` runs before the layer goes away).
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(layer))
        }
        .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .map_err(|e| format!("request_adapter: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("krypa"),
            // The simulator reports lower limits than wgpu's defaults (for example 15 inter-stage
            // variables, not 16); egui needs nothing special, so ask for what the adapter has.
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| format!("request_device: {e}"))?;

        let [w, h] = viewport.size_px();
        let mut config = surface
            .get_default_config(&adapter, w, h)
            .ok_or("the surface supports no format for this adapter")?;
        // egui blends in gamma space and expects a non-sRGB target (eframe does the same).
        let caps = surface.get_capabilities(&adapter);
        if let Some(format) = caps.formats.iter().copied().find(|f| !f.is_srgb()) {
            config.format = format;
        }
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

        let renderer = Renderer::new(&device, config.format, RendererOptions::default());
        Ok(Self {
            renderer,
            queue,
            device,
            config,
            surface,
            _instance: instance,
        })
    }

    /// Reconfigures the surface for a new pixel size.
    pub fn resize(&mut self, viewport: &Viewport) {
        let [w, h] = viewport.size_px();
        if (w, h) != (self.config.width, self.config.height) {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Draws one egui frame. Returns `Ok(false)` if no frame was presented (no drawable
    /// right now, or the surface had to be reconfigured): the caller should try again soon.
    pub fn paint(
        &mut self,
        full: &mut FullOutput,
        ctx: &egui::Context,
        viewport: &Viewport,
    ) -> Result<bool, String> {
        for (id, deltas) in std::mem::take(&mut full.textures_delta.set) {
            for delta in &deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, id, delta);
            }
        }
        let jobs = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: viewport.size_px(),
            pixels_per_point: full.pixels_per_point,
        };

        let drawn = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder =
                    self.device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("krypa"),
                        });
                let extra = self.renderer.update_buffers(
                    &self.device,
                    &self.queue,
                    &mut encoder,
                    &jobs,
                    &screen,
                );
                {
                    let mut pass = encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("krypa"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                            multiview_mask: None,
                        })
                        .forget_lifetime();
                    self.renderer.render(&mut pass, &jobs, &screen);
                }
                self.queue
                    .submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
                self.queue.present(frame);
                true
            }
            // No drawable this tick (busy, or the app is not visible): try again next tick.
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => false,
            // The layer changed under us (size, background/foreground): rebuild and retry.
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                false
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface validation error".to_owned());
            }
        };
        for id in std::mem::take(&mut full.textures_delta.free) {
            self.renderer.free_texture(&id);
        }
        Ok(drawn)
    }
}
