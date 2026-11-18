//! The sandbox's custom shader: diagonal stripes in two colors that slide over time.
//! Applied with `shader.apply <name> stripes`. The shader is `assets/shaders/stripes.wesl`.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use retreat_engine::AppShaderMaterialExt;

/// The name `shader.apply` uses for this material.
pub const STRIPES_MATERIAL_NAME: &str = "stripes";

/// Registers the stripes material with the engine.
pub struct StripesMaterialPlugin;

impl Plugin for StripesMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.register_shader_material(STRIPES_MATERIAL_NAME, StripesMaterial::sandbox_default);
    }
}

/// Material asset for the stripes shader. `AsBindGroup` turns the `#[uniform(0)]` field into
/// binding 0 of the material bind group, which the shader reads as `settings`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct StripesMaterial {
    #[uniform(0)]
    pub settings: StripesSettings,
}

/// The uniform block. Field order and types must match `StripesSettings` in `stripes.wesl`.
/// Uniform buffers (not storage buffers) keep it within WebGL2 limits.
#[derive(ShaderType, Debug, Clone, Copy)]
pub struct StripesSettings {
    pub first_color: LinearRgba,
    pub second_color: LinearRgba,
    /// How many stripe pairs fit in one world unit, measured along the (1, 1, 1) diagonal.
    pub stripes_per_unit: f32,
    /// Stripe pairs that slide past a point per second of simulation time.
    pub speed: f32,
}

impl StripesMaterial {
    /// The look `shader.apply ... stripes` gives: warm yellow on dark purple.
    pub fn sandbox_default() -> Self {
        Self {
            settings: StripesSettings {
                first_color: LinearRgba::from(Color::srgb(1.0, 0.8, 0.2)),
                second_color: LinearRgba::from(Color::srgb(0.25, 0.1, 0.35)),
                stripes_per_unit: 2.0,
                speed: 0.5,
            },
        }
    }
}

impl Material for StripesMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/stripes.wesl".into()
    }
}
