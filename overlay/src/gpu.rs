//! One-time wgpu initialization.
//!
//! Called once at daemon startup. The returned `GpuContext` is reused
//! across captures — only the surface and texture change per-window.

use wgpu::util::DeviceExt;

/// Persistent GPU state shared across captures.
pub struct GpuContext {
  pub instance: wgpu::Instance,
  pub adapter: wgpu::Adapter,
  pub device: wgpu::Device,
  pub queue: wgpu::Queue,
  pub shader: wgpu::ShaderModule,
  pub pipeline_layout: wgpu::PipelineLayout,
  pub bind_group_layout: wgpu::BindGroupLayout,
  pub sampler: wgpu::Sampler,
}

impl GpuContext {
  /// Initialize wgpu: adapter, device, shader, pipeline layout.
  /// This is the expensive part (~200-500ms first time) that we pre-warm.
  pub fn init() -> Result<Self, String> {
    // Windows: use Vulkan — D3D12 only supports Opaque alpha mode,
    // which prevents transparent compositing with the desktop.
    // Vulkan supports PreMultiplied alpha via DWM on Windows 10+.
    // macOS: use Metal (the only native GPU API).
    let backends = if cfg!(target_os = "macos") {
      wgpu::Backends::METAL
    } else {
      wgpu::Backends::VULKAN
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
      backends,
      ..Default::default()
    });

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
      power_preference: wgpu::PowerPreference::HighPerformance,
      compatible_surface: None,
      force_fallback_adapter: false,
    }))
    .map_err(|e| format!("No suitable GPU adapter found: {e}"))?;

    let (device, queue) = pollster::block_on(adapter.request_device(
      &wgpu::DeviceDescriptor {
        label: Some("overlay_device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
      },
    ))
    .map_err(|e| format!("Failed to create device: {e}"))?;

    let shader: wgpu::ShaderModule = device.create_shader_module(wgpu::ShaderModuleDescriptor {
      label: Some("overlay_shader"),
      source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
      label: Some("overlay_bgl"),
      entries: &[
        // Uniforms
        wgpu::BindGroupLayoutEntry {
          binding: 0,
          visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
          ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
          },
          count: None,
        },
        // Screenshot texture
        wgpu::BindGroupLayoutEntry {
          binding: 1,
          visibility: wgpu::ShaderStages::FRAGMENT,
          ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
          },
          count: None,
        },
        // Sampler
        wgpu::BindGroupLayoutEntry {
          binding: 2,
          visibility: wgpu::ShaderStages::FRAGMENT,
          ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
          count: None,
        },
      ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
      label: Some("overlay_pl"),
      bind_group_layouts: &[&bind_group_layout],
      push_constant_ranges: &[],
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
      mag_filter: wgpu::FilterMode::Linear,
      min_filter: wgpu::FilterMode::Linear,
      ..Default::default()
    });

    Ok(Self {
      instance,
      adapter,
      device,
      queue,
      shader,
      pipeline_layout,
      bind_group_layout,
      sampler,
    })
  }

  /// Create a 1x1 transparent placeholder texture (used when no screenshot is loaded).
  pub fn create_placeholder_texture(&self) -> wgpu::Texture {
    self.device.create_texture_with_data(
      &self.queue,
      &wgpu::TextureDescriptor {
        label: Some("placeholder_tex"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
      },
      wgpu::util::TextureDataOrder::LayerMajor,
      &[0u8; 4],
    )
  }
}

/// Uniform buffer layout matching shader.wgsl `Uniforms` struct.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
  pub viewport: [f32; 2],
  pub sel_min: [f32; 2],
  pub sel_max: [f32; 2],
  pub has_texture: f32,
  pub dim_alpha: f32,
}

impl Uniforms {
  pub fn new(viewport_w: f32, viewport_h: f32, has_texture: bool, dim_alpha: f32) -> Self {
    Self {
      viewport: [viewport_w, viewport_h],
      sel_min: [-1.0, -1.0], // no selection
      sel_max: [-1.0, -1.0],
      has_texture: if has_texture { 1.0 } else { 0.0 },
      dim_alpha,
    }
  }
}
