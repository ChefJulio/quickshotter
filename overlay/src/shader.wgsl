// Overlay fullscreen shader.
//
// Renders:
// - Freeze mode: screenshot texture with dim overlay outside selection
// - Instant mode: semi-transparent dark overlay, clear inside selection
// - Selection border: 2px blue (#00aaff) rectangle

struct Uniforms {
  // Viewport size in physical pixels
  viewport: vec2<f32>,
  // Selection rectangle min corner (physical pixels). (-1,-1) = no selection.
  sel_min: vec2<f32>,
  // Selection rectangle max corner (physical pixels).
  sel_max: vec2<f32>,
  // 1.0 = has screenshot texture, 0.0 = transparent overlay
  has_texture: f32,
  // Dim factor for outside-selection area (0.0 = no dim, 0.55 = standard)
  dim_alpha: f32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var tex_sampler: sampler;

// Fullscreen triangle — covers clip space with 3 vertices, no vertex buffer.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
  var pos = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 3.0,  1.0),
  );
  return vec4<f32>(pos[idx], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
  let px = frag_pos.xy;
  let uv = px / u.viewport;

  // Determine if we have an active selection
  let has_sel = u.sel_min.x >= 0.0;

  // Inside selection rectangle?
  let inside = has_sel
    && px.x >= u.sel_min.x && px.x <= u.sel_max.x
    && px.y >= u.sel_min.y && px.y <= u.sel_max.y;

  // Selection border (2px wide)
  let bw = 2.0;
  let on_border = has_sel
    && px.x >= (u.sel_min.x - bw) && px.x <= (u.sel_max.x + bw)
    && px.y >= (u.sel_min.y - bw) && px.y <= (u.sel_max.y + bw)
    && !(px.x >= (u.sel_min.x + bw) && px.x <= (u.sel_max.x - bw)
      && px.y >= (u.sel_min.y + bw) && px.y <= (u.sel_max.y - bw));

  // Border color: #00aaff, pre-multiplied alpha
  if on_border {
    return vec4<f32>(0.0, 0.667, 1.0, 1.0);
  }

  // Freeze mode: sample texture
  if u.has_texture > 0.5 {
    var color = textureSample(tex, tex_sampler, uv);
    if !inside && has_sel {
      // Dim outside selection
      color = vec4<f32>(color.rgb * (1.0 - u.dim_alpha), 1.0);
    } else if !has_sel {
      // No selection yet: dim everything
      color = vec4<f32>(color.rgb * (1.0 - u.dim_alpha), 1.0);
    }
    // Pre-multiply alpha for compositor
    return vec4<f32>(color.rgb * color.a, color.a);
  }

  // Instant mode: transparent overlay
  if inside {
    // Clear inside selection
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
  }

  // Outside selection (or no selection): semi-transparent dark
  // Pre-multiplied: rgb * a, a
  let a = u.dim_alpha;
  return vec4<f32>(0.0, 0.0, 0.0, a);
}
