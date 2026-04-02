//! GPU-based renderers for overlay modes.
//!
//! Two variants:
//! - FreezeRenderer: opaque window with screenshot texture, dimmed outside selection
//! - LiveGpuRenderer: transparent window with semi-transparent dark overlay,
//!   clear cutout inside selection. Used on macOS where layered windows aren't available.

use std::sync::Arc;
use wgpu::util::DeviceExt;

use crate::gpu::{GpuContext, Uniforms};
use crate::interaction::Interaction;

/// Per-capture render state for freeze mode.
pub struct FreezeRenderer {
  pub surface: wgpu::Surface<'static>,
  pub surface_config: wgpu::SurfaceConfiguration,
  pub pipeline: wgpu::RenderPipeline,
  pub bind_group: wgpu::BindGroup,
  pub uniform_buf: wgpu::Buffer,
  pub viewport_w: f32,
  pub viewport_h: f32,
  /// Cached selection to skip GPU work when nothing changed.
  last_selection: Option<(f32, f32, f32, f32)>,
}

impl FreezeRenderer {
  pub fn new(
    gpu: &GpuContext,
    window: Arc<winit::window::Window>,
    screenshot_rgba: &[u8],
    screenshot_width: u32,
    screenshot_height: u32,
  ) -> Result<Self, String> {
    let size = window.inner_size();
    let viewport_w = size.width as f32;
    let viewport_h = size.height as f32;

    let surface = gpu
      .instance
      .create_surface(window)
      .map_err(|e| format!("Failed to create surface: {e}"))?;

    let caps = surface.get_capabilities(&gpu.adapter);

    let format = caps
      .formats
      .iter()
      .copied()
      .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb)
      .or_else(|| caps.formats.first().copied())
      .ok_or_else(|| "No surface format available".to_string())?;

    // Opaque is fine — freeze mode window is not transparent
    let alpha_mode = caps.alpha_modes[0];

    let surface_config = wgpu::SurfaceConfiguration {
      usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
      format,
      width: size.width.max(1),
      height: size.height.max(1),
      present_mode: wgpu::PresentMode::Fifo,
      desired_maximum_frame_latency: 1,
      alpha_mode,
      view_formats: vec![],
    };
    surface.configure(&gpu.device, &surface_config);

    // Upload screenshot texture
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
      label: Some("screenshot_tex"),
      size: wgpu::Extent3d {
        width: screenshot_width,
        height: screenshot_height,
        depth_or_array_layers: 1,
      },
      mip_level_count: 1,
      sample_count: 1,
      dimension: wgpu::TextureDimension::D2,
      format: wgpu::TextureFormat::Rgba8UnormSrgb,
      usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
      view_formats: &[],
    });
    gpu.queue.write_texture(
      wgpu::TexelCopyTextureInfo {
        texture: &texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
      },
      screenshot_rgba,
      wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(4 * screenshot_width),
        rows_per_image: Some(screenshot_height),
      },
      wgpu::Extent3d {
        width: screenshot_width,
        height: screenshot_height,
        depth_or_array_layers: 1,
      },
    );

    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let dim_alpha: f32 = 0.45;
    let uniforms = Uniforms::new(viewport_w, viewport_h, true, dim_alpha);
    let uniform_buf = gpu
      .device
      .create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniform_buf"),
        contents: bytemuck::cast_slice(&[uniforms]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
      });

    let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
      label: Some("overlay_bg"),
      layout: &gpu.bind_group_layout,
      entries: &[
        wgpu::BindGroupEntry {
          binding: 0,
          resource: uniform_buf.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
          binding: 1,
          resource: wgpu::BindingResource::TextureView(&texture_view),
        },
        wgpu::BindGroupEntry {
          binding: 2,
          resource: wgpu::BindingResource::Sampler(&gpu.sampler),
        },
      ],
    });

    let pipeline = gpu
      .device
      .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("freeze_pipeline"),
        layout: Some(&gpu.pipeline_layout),
        vertex: wgpu::VertexState {
          module: &gpu.shader,
          entry_point: Some("vs_main"),
          buffers: &[],
          compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
          module: &gpu.shader,
          entry_point: Some("fs_main"),
          targets: &[Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
          })],
          compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
      });

    Ok(Self {
      surface,
      surface_config,
      pipeline,
      bind_group,
      uniform_buf,
      viewport_w,
      viewport_h,
      // Sentinel ensures first frame always renders
      last_selection: Some((-2.0, -2.0, -2.0, -2.0)),
    })
  }

  /// Render one frame with current interaction state.
  /// Skips GPU work entirely when the selection hasn't changed.
  pub fn render(&mut self, gpu: &GpuContext, interaction: &Interaction) {
    let current_sel = interaction.selection();

    // Skip if selection hasn't changed since last frame
    if current_sel == self.last_selection {
      return;
    }
    self.last_selection = current_sel;

    let mut uniforms = Uniforms::new(self.viewport_w, self.viewport_h, true, 0.45);
    if let Some((x1, y1, x2, y2)) = current_sel {
      uniforms.sel_min = [x1, y1];
      uniforms.sel_max = [x2, y2];
    }
    gpu
      .queue
      .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[uniforms]));

    let frame = match self.surface.get_current_texture() {
      Ok(f) => f,
      Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
        self.surface.configure(&gpu.device, &self.surface_config);
        return;
      }
      Err(_) => return,
    };

    let view = frame
      .texture
      .create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = gpu
      .device
      .create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("freeze_encoder"),
      });

    {
      let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("freeze_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
          view: &view,
          resolve_target: None,
          ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            store: wgpu::StoreOp::Store,
          },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
      });
      pass.set_pipeline(&self.pipeline);
      pass.set_bind_group(0, &self.bind_group, &[]);
      pass.draw(0..3, 0..1);
    }

    gpu.queue.submit(std::iter::once(encoder.finish()));
    frame.present();
  }
}

/// GPU-based live overlay renderer for macOS.
/// Uses a transparent wgpu surface with the shader's `has_texture=0` path:
/// semi-transparent dark overlay outside selection, clear inside.
pub struct LiveGpuRenderer {
  pub surface: wgpu::Surface<'static>,
  pub surface_config: wgpu::SurfaceConfiguration,
  pub pipeline: wgpu::RenderPipeline,
  pub bind_group: wgpu::BindGroup,
  pub uniform_buf: wgpu::Buffer,
  pub viewport_w: f32,
  pub viewport_h: f32,
  pub dim_alpha: f32,
  last_selection: Option<(f32, f32, f32, f32)>,
}

impl LiveGpuRenderer {
  pub fn new(
    gpu: &GpuContext,
    window: Arc<winit::window::Window>,
    dim_alpha: f32,
  ) -> Result<Self, String> {
    let size = window.inner_size();
    let viewport_w = size.width as f32;
    let viewport_h = size.height as f32;

    let surface = gpu
      .instance
      .create_surface(window)
      .map_err(|e| format!("Failed to create surface: {e}"))?;

    let caps = surface.get_capabilities(&gpu.adapter);

    let format = caps
      .formats
      .iter()
      .copied()
      .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb)
      .or_else(|| caps.formats.first().copied())
      .ok_or_else(|| "No surface format available".to_string())?;

    // Need PreMultiplied alpha for transparent compositing on macOS
    let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
      wgpu::CompositeAlphaMode::PreMultiplied
    } else {
      caps.alpha_modes[0]
    };

    let surface_config = wgpu::SurfaceConfiguration {
      usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
      format,
      width: size.width.max(1),
      height: size.height.max(1),
      present_mode: wgpu::PresentMode::Fifo,
      desired_maximum_frame_latency: 1,
      alpha_mode,
      view_formats: vec![],
    };
    surface.configure(&gpu.device, &surface_config);

    // 1x1 placeholder texture (shader needs a texture binding even if unused)
    let placeholder = gpu.create_placeholder_texture();
    let texture_view = placeholder.create_view(&wgpu::TextureViewDescriptor::default());

    let uniforms = Uniforms::new(viewport_w, viewport_h, false, dim_alpha);
    let uniform_buf = gpu
      .device
      .create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("live_uniform_buf"),
        contents: bytemuck::cast_slice(&[uniforms]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
      });

    let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
      label: Some("live_bg"),
      layout: &gpu.bind_group_layout,
      entries: &[
        wgpu::BindGroupEntry {
          binding: 0,
          resource: uniform_buf.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
          binding: 1,
          resource: wgpu::BindingResource::TextureView(&texture_view),
        },
        wgpu::BindGroupEntry {
          binding: 2,
          resource: wgpu::BindingResource::Sampler(&gpu.sampler),
        },
      ],
    });

    let pipeline = gpu
      .device
      .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("live_pipeline"),
        layout: Some(&gpu.pipeline_layout),
        vertex: wgpu::VertexState {
          module: &gpu.shader,
          entry_point: Some("vs_main"),
          buffers: &[],
          compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
          module: &gpu.shader,
          entry_point: Some("fs_main"),
          targets: &[Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
          })],
          compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
      });

    Ok(Self {
      surface,
      surface_config,
      pipeline,
      bind_group,
      uniform_buf,
      viewport_w,
      viewport_h,
      dim_alpha,
      // Sentinel value: Some((-2,-2,-2,-2)) ensures the first frame always renders
      // (it will never match the real selection which starts as None).
      last_selection: Some((-2.0, -2.0, -2.0, -2.0)),
    })
  }

  pub fn render(&mut self, gpu: &GpuContext, interaction: &Interaction) {
    let current_sel = interaction.selection();

    if current_sel == self.last_selection {
      return;
    }
    self.last_selection = current_sel;

    let mut uniforms = Uniforms::new(self.viewport_w, self.viewport_h, false, self.dim_alpha);
    if let Some((x1, y1, x2, y2)) = current_sel {
      uniforms.sel_min = [x1, y1];
      uniforms.sel_max = [x2, y2];
    }
    gpu
      .queue
      .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[uniforms]));

    let frame = match self.surface.get_current_texture() {
      Ok(f) => f,
      Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
        self.surface.configure(&gpu.device, &self.surface_config);
        return;
      }
      Err(_) => return,
    };

    let view = frame
      .texture
      .create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder = gpu
      .device
      .create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("live_encoder"),
      });

    {
      let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("live_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
          view: &view,
          resolve_target: None,
          ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
          },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
      });
      pass.set_pipeline(&self.pipeline);
      pass.set_bind_group(0, &self.bind_group, &[]);
      pass.draw(0..3, 0..1);
    }

    gpu.queue.submit(std::iter::once(encoder.finish()));
    frame.present();
  }
}
