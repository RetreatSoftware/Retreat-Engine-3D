//! The sandbox's starting scene: a ground plane, one cube, a light and a camera. The ground and
//! the cube are fixed (immovable) physics bodies, so boxes from `physics.spawn_box` land on them.

use bevy::prelude::*;
use retreat_engine_api::{GameCamera, PhysicsBody, PhysicsBodyKind, PhysicsShape};

/// Spawns the starting scene once, at startup.
pub struct SandboxScenePlugin;

impl Plugin for SandboxScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_sandbox_scene);
    }
}

/// Marks the demo cube so systems and tests can find it.
#[derive(Component)]
pub struct SandboxCube;

/// Marks the ground plane.
#[derive(Component)]
pub struct SandboxGround;

const GROUND_SIZE: f32 = 8.0;
/// The ground collider is a slab this thick whose top face is the visible plane (y = 0).
const GROUND_COLLIDER_THICKNESS: f32 = 1.0;
const CUBE_SIZE: f32 = 1.0;

fn spawn_sandbox_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Name::new("sandbox_ground"),
        SandboxGround,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(GROUND_SIZE, GROUND_SIZE))),
        MeshMaterial3d(materials.add(Color::srgb(0.30, 0.50, 0.30))),
        ground_physics_body(),
    ));

    commands.spawn((
        Name::new("sandbox_cube"),
        SandboxCube,
        Mesh3d(meshes.add(Cuboid::new(CUBE_SIZE, CUBE_SIZE, CUBE_SIZE))),
        MeshMaterial3d(materials.add(Color::srgb(0.49, 0.56, 1.0))),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::splat(CUBE_SIZE / 2.0),
            },
            shape_offset: Vec3::ZERO,
        },
        // Lift the cube by half its size so it rests on the ground instead of sinking into it.
        Transform::from_xyz(0.0, CUBE_SIZE / 2.0, 0.0),
    ));

    commands.spawn((
        Name::new("sandbox_light"),
        PointLight {
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0),
    ));

    commands.spawn((
        Name::new("sandbox_camera"),
        // The engine creates the real camera from this description (retreat_engine_api::GameCamera).
        GameCamera::looking_at(Vec3::new(-2.5, 4.5, 9.0), Vec3::ZERO),
    ));
}

/// A slab under the ground plane: same width as the plane, top face at y = 0. A plane mesh has
/// no thickness, and a zero-thickness collider lets fast bodies tunnel through.
fn ground_physics_body() -> PhysicsBody {
    let half_thickness = GROUND_COLLIDER_THICKNESS / 2.0;
    PhysicsBody {
        kind: PhysicsBodyKind::Fixed,
        shape: PhysicsShape::Cuboid {
            half_extents: Vec3::new(GROUND_SIZE / 2.0, half_thickness, GROUND_SIZE / 2.0),
        },
        shape_offset: Vec3::new(0.0, -half_thickness, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an app with only what the scene needs (no window, no GPU), runs one frame.
    fn app_after_first_frame() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins(SandboxScenePlugin);
        app.update();
        app
    }

    #[test]
    fn scene_spawns_one_cube_resting_on_the_ground() {
        let mut app = app_after_first_frame();
        let mut cubes = app
            .world_mut()
            .query_filtered::<&Transform, With<SandboxCube>>();
        let transforms: Vec<&Transform> = cubes.iter(app.world()).collect();
        assert_eq!(transforms.len(), 1, "expected exactly one SandboxCube");
        assert_eq!(
            transforms[0].translation.y,
            CUBE_SIZE / 2.0,
            "cube bottom must touch y = 0"
        );
    }

    #[test]
    fn scene_spawns_ground_light_and_camera() {
        let mut app = app_after_first_frame();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<(), With<SandboxGround>>()
                .iter(world)
                .count(),
            1
        );
        assert_eq!(
            world
                .query_filtered::<(), With<PointLight>>()
                .iter(world)
                .count(),
            1
        );
        assert_eq!(
            world
                .query_filtered::<(), With<Camera3d>>()
                .iter(world)
                .count(),
            1
        );
    }

    /// Runs `scripts/verify/commands_smoke.cmds` on the scene without a window, so CI checks
    /// the script too. `include_str!` embeds the file at compile time.
    #[test]
    fn commands_smoke_script_passes() {
        use retreat_engine::{CommandScript, RetreatCommandsPlugin};

        let text = include_str!("../../scripts/verify/commands_smoke.cmds");
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins(SandboxScenePlugin)
            .add_plugins(RetreatCommandsPlugin {
                seed: 0,
                command_script: Some(CommandScript::from_text("commands_smoke.cmds", text)),
            });
        let max_updates = 200;
        for _ in 0..max_updates {
            app.update();
            if let Some(exit) = app.should_exit() {
                let failure = app
                    .world()
                    .resource::<CommandScript>()
                    .failure()
                    .map(String::from);
                assert_eq!(exit, AppExit::Success, "script failed: {failure:?}");
                return;
            }
        }
        panic!("commands_smoke.cmds did not quit within {max_updates} updates");
    }
}
