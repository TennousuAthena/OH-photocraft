use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;

use egui::epaint::mutex::RwLock;
use egui_wgpu::{RenderState, Renderer, RendererOptions, ScreenDescriptor, SurfaceConfig};
use raw_window_handle::{OhosDisplayHandle, OhosNdkWindowHandle};

pub struct GpuSurface {
    surface: wgpu::Surface<'static>,
    window: NonNull<c_void>,
    config: wgpu::SurfaceConfiguration,
    pub state: RenderState,
    pub size: [u32; 2],
}

impl GpuSurface {
    /// # Safety
    /// The caller retains an OHNativeWindow until this object is dropped, and drives it only
    /// from this renderer's thread. The bridge joins every surface destruction before release.
    pub unsafe fn new(
        window: NonNull<c_void>,
        width: u32,
        height: u32,
        previous: Option<RenderState>,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 || width > 32768 || height > 32768 {
            return Err("invalid native surface size".into());
        }
        let instance = previous
            .as_ref()
            .map(|state| state.instance.clone())
            .unwrap_or_else(|| {
                wgpu::Instance::new(wgpu::InstanceDescriptor {
                    backends: wgpu::Backends::GL,
                    ..wgpu::InstanceDescriptor::new_without_display_handle()
                })
            });
        // SAFETY: The C++ bridge retains the window for the surface's entire lifetime.
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(OhosDisplayHandle::new().into()),
                raw_window_handle: OhosNdkWindowHandle::new(window).into(),
            })
        }
        .map_err(|e| format!("OHOS GLES surface: {e}"))?;
        let (state, mut config) = if let Some(state) = previous {
            // Keep the device and renderer: egui and PhotoCraft still hold texture
            // handles after the OS destroys and recreates the native surface.
            let capabilities = surface.get_capabilities(&state.adapter);
            if !capabilities.formats.contains(&state.target_format) {
                return Err(
                    "recreated OHOS surface does not support the existing renderer format".into(),
                );
            }
            let mut config = surface
                .get_default_config(&state.adapter, width, height)
                .ok_or("existing adapter cannot render to the recreated OHOS surface")?;
            config.format = state.target_format;
            (state, config)
        } else {
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                    ..Default::default()
                }))
                .map_err(|e| format!("OHOS GLES adapter: {e}"))?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("PhotoCraft OHOS"),
                    required_limits:
                        wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                    ..Default::default()
                }))
                .map_err(|e| format!("OHOS GLES device: {e}"))?;
            let mut config = surface
                .get_default_config(&adapter, width, height)
                .ok_or("no OHOS surface configuration")?;
            let capabilities = surface.get_capabilities(&adapter);
            if let Some(format) = capabilities
                .formats
                .iter()
                .copied()
                .find(|format| !format.is_srgb())
            {
                config.format = format;
            }
            let renderer = Renderer::new(&device, config.format, RendererOptions::default());
            let state = RenderState {
                adapter,
                available_adapters: Vec::new(),
                instance,
                device,
                queue,
                target_format: config.format,
                renderer: Arc::new(RwLock::new(renderer)),
                surface_config: SurfaceConfig::HIGH_THROUGHPUT,
            };
            (state, config)
        };
        let limit = state.device.limits().max_texture_dimension_2d;
        if width > limit || height > limit {
            return Err(format!(
                "OHOS surface exceeds the device's {limit}-pixel texture limit"
            ));
        }
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        surface.configure(&state.device, &config);
        Ok(Self {
            surface,
            window,
            config,
            state,
            size: [width, height],
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        let limit = self
            .state
            .device
            .limits()
            .max_texture_dimension_2d
            .min(32768);
        if width > limit || height > limit {
            return Err("invalid surface dimensions".into());
        }
        self.size = [width, height];
        if width != 0 && height != 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.state.device, &self.config);
        }
        Ok(())
    }

    fn recreate_surface(&mut self) -> Result<(), String> {
        // SAFETY: new's lifetime contract requires the bridge to retain this
        // native window until GpuSurface is dropped, including recovery frames.
        let surface = unsafe {
            self.state
                .instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(OhosDisplayHandle::new().into()),
                    raw_window_handle: OhosNdkWindowHandle::new(self.window).into(),
                })
        }
        .map_err(|e| format!("recreate OHOS GLES surface: {e}"))?;
        if !surface
            .get_capabilities(&self.state.adapter)
            .formats
            .contains(&self.state.target_format)
        {
            return Err("recreated OHOS surface does not support the renderer format".into());
        }
        // Replacing drops the old EGL swapchain before configure creates the
        // replacement for the same native window. The GPU state stays intact.
        self.surface = surface;
        self.surface.configure(&self.state.device, &self.config);
        Ok(())
    }

    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        mut output: egui::FullOutput,
    ) -> Result<bool, String> {
        // Empty the egui delta object before any GPU call: its Drop asserts that
        // every delta was consumed, including while a GPU panic is unwinding.
        let mut updates = std::mem::take(&mut output.textures_delta.set);
        let frees = std::mem::take(&mut output.textures_delta.free);
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = ScreenDescriptor {
            size_in_pixels: self.size,
            pixels_per_point: output.pixels_per_point,
        };
        let mut renderer = self.state.renderer.write();
        for (id, deltas) in updates.drain() {
            for delta in &deltas {
                renderer.update_texture(&self.state.device, &self.state.queue, id, delta);
            }
        }
        if self.size.contains(&0) {
            flush_and_free(&self.state, &mut renderer, frees);
            ctx.request_repaint();
            return Ok(false);
        }
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                flush_and_free(&self.state, &mut renderer, frees);
                ctx.request_repaint();
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                flush_and_free(&self.state, &mut renderer, frees);
                drop(renderer);
                ctx.request_repaint();
                self.recreate_surface()?;
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                flush_and_free(&self.state, &mut renderer, frees);
                ctx.request_repaint();
                self.surface.configure(&self.state.device, &self.config);
                return Ok(false);
            }
            other => {
                flush_and_free(&self.state, &mut renderer, frees);
                ctx.request_repaint();
                return Err(format!("OHOS surface acquisition: {other:?}"));
            }
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("PhotoCraft frame"),
                });
        let mut buffers = renderer.update_buffers(
            &self.state.device,
            &self.state.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("PhotoCraft egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.10,
                            g: 0.10,
                            b: 0.10,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        buffers.push(encoder.finish());
        self.state.queue.submit(buffers);
        // Free after submission, while wgpu retains anything still in flight.
        for id in frees {
            renderer.free_texture(&id);
        }
        self.state.queue.present(texture);
        Ok(true)
    }
}

fn flush_and_free(
    state: &RenderState,
    renderer: &mut Renderer,
    frees: impl IntoIterator<Item = egui::TextureId>,
) {
    // update_texture enqueues writes. Submit those before destroying resources,
    // even when there is no presentable surface texture this frame.
    state
        .queue
        .submit(std::iter::empty::<wgpu::CommandBuffer>());
    for id in frees {
        renderer.free_texture(&id);
    }
}
