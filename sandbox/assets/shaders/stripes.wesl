// Stripes material (sandbox/src/stripes_material.rs): diagonal stripes in two colors that
// slide over simulation time. WESL file: WGSL plus `import`; Bevy 0.20 only resolves imports
// and `constants::` in .wesl files (a plain .wgsl file is passed to the GPU unprocessed).
// WebGL2 baseline: uniforms only, no storage buffers.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_view_bindings::globals;

// Must match `StripesSettings` in stripes_material.rs (same fields, same order).
struct StripesSettings {
    first_color: vec4<f32>,
    second_color: vec4<f32>,
    stripes_per_unit: f32,
    speed: f32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> settings: StripesSettings;

// Fixed direction of a simple light, so the faces of a shape read as 3D. The material
// ignores the scene's lights on purpose: it is a shader demo, not a lit surface.
const LIGHT_DIRECTION: vec3<f32> = vec3<f32>(0.4, 1.0, 0.3);

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let position = in.world_position.xyz;
    // Distance along the (1, 1, 1) diagonal, in stripe pairs, shifted by time.
    let along = (position.x + position.y + position.z) * settings.stripes_per_unit
        - globals.time * settings.speed;
    let in_first_stripe = fract(along) < 0.5;
    let color = select(settings.second_color, settings.first_color, in_first_stripe);

    let facing_light = max(dot(normalize(in.world_normal), normalize(LIGHT_DIRECTION)), 0.0);
    let brightness = 0.55 + 0.45 * facing_light;
    return vec4<f32>(color.rgb * brightness, color.a);
}
