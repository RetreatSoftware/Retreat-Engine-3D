//! `--headless`: no window. Every camera that would draw to the (missing) primary window
//! draws to one offscreen image instead, which `snapshot` captures. Game code spawns its
//! cameras exactly as in a windowed run, so a headless snapshot shows the real scene.

use bevy::camera::{RenderTarget, ShadowLodOrigin};
use bevy::prelude::*;
use bevy::render::render_resource::{PollType, TextureFormat};
use bevy::render::renderer::RenderDevice;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::window::WindowRef;

use crate::SnapshotSource;

/// Size of the offscreen image, equal to Bevy's default window size so headless and
/// windowed snapshots frame the scene the same way.
pub const HEADLESS_RESOLUTION: UVec2 = UVec2::new(1280, 720);

/// Creates the offscreen image, makes it the [`SnapshotSource`] and redirects cameras to it.
/// Needs `DefaultPlugins` (for `Assets<Image>`) to be added first.
pub(crate) struct HeadlessRenderPlugin;

impl Plugin for HeadlessRenderPlugin {
    fn build(&self, app: &mut App) {
        let image = Image::new_target_texture(
            HEADLESS_RESOLUTION.x,
            HEADLESS_RESOLUTION.y,
            // 8-bit sRGB color, the format of a normal window surface.
            TextureFormat::Rgba8UnormSrgb,
            None,
        );
        let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        app.insert_resource(SnapshotSource::OffscreenImage(handle))
            .add_systems(PreUpdate, render_new_cameras_offscreen);
        app.sub_app_mut(RenderApp).add_systems(
            Render,
            wait_for_gpu_to_finish_frame.in_set(RenderSystems::Cleanup),
        );
    }
}

/// A window paces frames: presenting waits for the GPU. Headless has no such brake, so the
/// CPU would queue frames far faster than a slow GPU (the software renderer) draws them, and
/// a snapshot would arrive minutes late. Waiting here keeps the GPU at most one frame behind.
fn wait_for_gpu_to_finish_frame(device: Res<RenderDevice>) {
    if let Err(error) = device.poll(PollType::wait_indefinitely()) {
        error!("headless: waiting for the GPU to finish the frame failed: {error}");
    }
}

/// True for the target a camera gets by default (the primary window), which does not
/// exist in a headless run. Cameras with any other target are left alone.
fn targets_primary_window(target: &RenderTarget) -> bool {
    matches!(target, RenderTarget::Window(WindowRef::Primary))
}

/// Points each new window camera at the offscreen image. It also marks the camera as a
/// `ShadowLodOrigin`: Bevy picks the shadow detail origin from "cameras that render to a
/// window", and the marker keeps that choice identical to a windowed run.
fn render_new_cameras_offscreen(
    mut commands: Commands,
    mut new_cameras: Query<(Entity, &mut RenderTarget), Added<Camera>>,
    source: Res<SnapshotSource>,
) {
    let SnapshotSource::OffscreenImage(image) = source.as_ref() else {
        return;
    };
    for (camera, mut target) in &mut new_cameras {
        if targets_primary_window(&target) {
            *target = RenderTarget::Image(image.clone().into());
            commands.entity(camera).insert(ShadowLodOrigin);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_default_window_target_is_redirected() {
        assert!(targets_primary_window(&RenderTarget::Window(
            WindowRef::Primary
        )));
        let image_target = RenderTarget::Image(Handle::<Image>::default().into());
        assert!(!targets_primary_window(&image_target));
    }

    #[test]
    fn new_window_camera_renders_to_the_offscreen_image() {
        let image = Handle::<Image>::default();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(SnapshotSource::OffscreenImage(image.clone()))
            .add_systems(PreUpdate, render_new_cameras_offscreen);
        let camera = app.world_mut().spawn(Camera3d::default()).id();
        app.update();

        let target = app
            .world()
            .get::<RenderTarget>(camera)
            .expect("cameras have a RenderTarget");
        assert!(
            matches!(target, RenderTarget::Image(image_target) if image_target.handle == image),
            "camera must render to the offscreen image, got {target:?}"
        );
        assert!(app.world().get::<ShadowLodOrigin>(camera).is_some());
    }
}
