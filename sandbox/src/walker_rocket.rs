//! A demo of what a projectile module uses (it is not the arena's launcher): a predicted spawn
//! ([`spawn_predicted`]) moved by its synced [`NetPose`], drawn as a [`SphereVisual`] (the own
//! rocket at the predicted present, others in the server's past), and an explosion shown and heard once.
//!
//! - `sandbox.walker_rocket <yaw> <pitch>`: the sender's walker launches a rocket (right mouse
//!   button).
//! - `assert.walker_rockets <count>`.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, AppInputExt, AppNetExt, AppNetStateExt, CommandArgs, CommandOutcome,
    CommandResult, DEFAULT_SOUND_REACH, EffectTime, HELD_ONLY, HideVisuals, InputAction,
    InputActionSystems, InputActions, InputKey, NetCommandSender, NetInputQueue, NetLocalClient,
    NetPose, NetReplaying, NetState, PhysicsControlSystems, PhysicsShape, RemoteInterpolation,
    RemoveNetEntity, RenderSmoothing, RetiredEntity, ShapeCast, SoundEvent, SoundEvents,
    SoundPosition, SphereVisual, VisualEffect, VisualEffects, cast_shape, run_command_line,
    spawn_predicted,
};
use serde::{Deserialize, Serialize};

use crate::walker::{Walker, WalkerAim, walker_of};
use crate::walker_combat::{EYE_HEIGHT, WalkerHealth, hurt_walker};

const ROCKET_SPEED: f32 = 12.0;
const ROCKET_RADIUS: f32 = 0.15;
const ROCKET_DAMAGE: u32 = 100;
const BLAST_RADIUS: f32 = 2.0;
/// A rocket that hits nothing explodes after this long (3 seconds).
const FLIGHT_TICKS: u32 = 192;
/// An exploded rocket stays this long, so every client hears of the explosion.
const LINGER_TICKS: u32 = 16;
const ROCKET_COLOR: Color = Color::srgb(1.0, 0.45, 0.1);

/// A rocket's synced state; its position is its `NetPose`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rocket {
    pub owner: Option<u32>,
    pub direction: Vec3,
    pub age: u32,
    /// Ticks since it exploded; 0 while it flies.
    pub exploded: u32,
}

impl NetState for Rocket {
    const ID: &'static str = "sandbox.rocket";
}

pub struct WalkerRocketPlugin;

impl Plugin for WalkerRocketPlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<Rocket>()
            .register_command(
                "sandbox.walker_rocket",
                "sandbox.walker_rocket <yaw> <pitch>",
                rocket_command,
            )
            .register_command(
                "assert.walker_rockets",
                "assert.walker_rockets <count>",
                assert_rockets_command,
            )
            .allow_client_command("sandbox.walker_rocket")
            .register_action(InputAction {
                id: "walk_rocket",
                command_line: HELD_ONLY,
                default_keys: &[InputKey::MouseRight],
            })
            .add_systems(
                FixedUpdate,
                (
                    send_rocket.after(InputActionSystems),
                    fly_rockets.in_set(PhysicsControlSystems),
                ),
            )
            .add_systems(Update, dress_rockets);
    }
}

/// Turns a new press of the rocket button into one command.
fn send_rocket(world: &mut World, mut held_before: Local<bool>) {
    if world.resource::<NetReplaying>().0 {
        return;
    }
    let held = world.resource::<InputActions>().is_held("walk_rocket");
    let pressed = held && !*held_before;
    *held_before = held;
    let local = world.resource::<NetLocalClient>().0;
    if !pressed || walker_of(world, local).is_none() {
        return;
    }
    let line = format!(
        "sandbox.walker_rocket {} 0",
        world.resource::<WalkerAim>().yaw
    );
    if world.resource::<NetInputQueue>().accepting {
        world.resource_mut::<NetInputQueue>().push(line);
    } else if let Err(error) = run_command_line(world, &line) {
        warn!("{error}");
    }
}

fn rocket_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let yaw: f32 = args.parse(0, "yaw")?;
    let pitch: f32 = args.parse(1, "pitch")?;
    if !(yaw.is_finite() && pitch.is_finite()) {
        return Err(args.failure("the numbers must be finite"));
    }
    let owner = world.resource::<NetCommandSender>().0;
    let Some(shooter) = walker_of(world, owner) else {
        return Err(args.failure("the sender has no walker"));
    };
    if world
        .get::<WalkerHealth>(shooter)
        .is_some_and(|health| health.down_ticks > 0)
    {
        return Err(args.failure("a downed walker cannot shoot"));
    }
    let eyes = world
        .get::<Transform>(shooter)
        .map_or(Vec3::ZERO, |transform| transform.translation)
        + Vec3::Y * EYE_HEIGHT;
    let direction = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Vec3::NEG_Z;
    spawn_predicted(
        world,
        "rocket",
        (
            Rocket {
                owner,
                direction,
                age: 0,
                exploded: 0,
            },
            NetPose::at(eyes),
        ),
    );
    Ok(CommandOutcome::Continue)
}

/// Moves every rocket in name order, explodes it on what it meets, and removes it later.
fn fly_rockets(world: &mut World) {
    let mut query =
        world.query_filtered::<(Entity, &Name, &Rocket, &NetPose), Without<RetiredEntity>>();
    let mut rockets: Vec<(String, Entity, Rocket, NetPose)> = query
        .iter(world)
        .map(|(entity, name, rocket, pose)| (name.to_string(), entity, *rocket, *pose))
        .collect();
    rockets.sort_by(|left, right| left.0.cmp(&right.0));
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    for (name, entity, mut rocket, mut pose) in rockets {
        if rocket.exploded > 0 {
            rocket.exploded += 1;
            if rocket.exploded > LINGER_TICKS {
                world.entity_mut(entity).insert(RemoveNetEntity);
            }
        } else {
            rocket.age += 1;
            let step = ROCKET_SPEED * dt;
            let own = walker_of(world, rocket.owner);
            let hit = cast_shape(
                world,
                &ShapeCast {
                    shape: PhysicsShape::Ball {
                        radius: ROCKET_RADIUS,
                    },
                    origin: pose.translation,
                    rotation: Quat::IDENTITY,
                    direction: rocket.direction,
                    max_distance: step,
                    exclude: own.into_iter().collect(),
                },
            );
            pose.translation += rocket.direction * hit.as_ref().map_or(step, |hit| hit.distance);
            if hit.is_some() || rocket.age >= FLIGHT_TICKS {
                rocket.exploded = 1;
                explode(world, &name, &rocket, pose.translation);
            }
        }
        world.entity_mut(entity).insert((rocket, pose));
    }
}

fn explode(world: &mut World, name: &str, rocket: &Rocket, center: Vec3) {
    let mut walkers: Vec<(String, Entity, f32)> = world
        .query_filtered::<(Entity, &Name, &Transform, &Walker), Without<RetiredEntity>>()
        .iter(world)
        .map(|(entity, name, transform, _)| {
            let distance = transform.translation.distance(center);
            (name.to_string(), entity, distance)
        })
        .filter(|(_, _, distance)| *distance <= BLAST_RADIUS)
        .collect();
    walkers.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, walker, _) in walkers {
        hurt_walker(world, walker, ROCKET_DAMAGE);
    }
    let local = world.resource::<NetLocalClient>().0;
    // Another player's rocket is drawn in the past, so its explosion waits for the view too.
    let time = if local.is_some() && rocket.owner != local {
        EffectTime::RemoteView
    } else {
        EffectTime::Present
    };
    world.resource_mut::<VisualEffects>().play(VisualEffect {
        id: format!("{name}/boom"),
        position: center,
        radius: BLAST_RADIUS,
        color: ROCKET_COLOR,
        glow: 2.0,
        ticks: 24,
        time,
    });
    world.resource_mut::<SoundEvents>().play(SoundEvent {
        id: format!("{name}/boom"),
        path: "sounds/tone.wav".to_string(),
        volume: 0.2,
        at: Some(SoundPosition {
            position: center,
            reach: DEFAULT_SOUND_REACH,
        }),
        time,
    });
}

/// Looks, on each machine. Safe to run again on a re-added rocket.
fn dress_rockets(
    rockets: Query<(Entity, Ref<Rocket>)>,
    local: Res<NetLocalClient>,
    mut commands: Commands,
) {
    for (entity, rocket) in &rockets {
        if rocket.is_added() {
            commands.entity(entity).insert(SphereVisual {
                radius: ROCKET_RADIUS,
                color: ROCKET_COLOR,
                glow: 1.5,
            });
            if local.0.is_some() && rocket.owner != local.0 {
                commands.entity(entity).insert(RemoteInterpolation);
            } else {
                commands.entity(entity).insert(RenderSmoothing::default());
            }
        }
        // The own rocket vanishes into its explosion; another's goes when the view passes it.
        if rocket.is_changed() && (local.0.is_none() || rocket.owner == local.0) {
            if rocket.exploded > 0 {
                commands.entity(entity).insert(HideVisuals);
            } else {
                commands.entity(entity).remove::<HideVisuals>();
            }
        }
    }
}

fn assert_rockets_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected: usize = args.parse(0, "count")?;
    let count = world
        .query_filtered::<&Rocket, Without<RetiredEntity>>()
        .iter(world)
        .count();
    if count == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("there are {count} rockets, expected {expected}")))
    }
}
