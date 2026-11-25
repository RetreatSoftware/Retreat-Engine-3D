//! Step and slope handling for character capsules (contract
//! `retreat_engine_api/src/character_motion.rs`). One system, after `PhysicsControlSystems` and
//! before velocity requests: it turns the velocity a module asked for into one that follows the
//! ground, and lifts or lowers the body over steps. It uses the engine's own casts, so it gives
//! the same result on the server, on a client and in a replay.
//!
//! The casts use a capsule [`SKIN`] thinner than the body, so a body resting against something
//! does not start the cast already touching it.
//!
//! Commands: `physics.set_stepping <name> <step_height> <slope_degrees>`,
//! `assert.grounded <name> <true|false>`.

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use retreat_engine_api::{
    AppCommandExt, CharacterGround, CharacterStepping, CommandArgs, CommandOutcome, CommandResult,
    PhysicsBody, PhysicsBodyKind, PhysicsShape, PhysicsVelocitySet, RayCast, RetiredEntity,
    ShapeCast, cast_ray, cast_shape,
};

use crate::entity_names::find_unique_entity_by_name;
use crate::physics::{PhysicsProxy, sync_body_readouts};

/// How much thinner than the body the probing capsule is.
const SKIN: f32 = 0.02;
/// How far below its feet a character that was in the air looks for ground.
const LANDING_PROBE: f32 = 0.03;
/// Upward speed above which an airborne or jumping character is left alone.
const RISING_SPEED: f32 = 0.5;
/// How far past a step's edge the surface check looks.
const EDGE_REACH: f32 = 0.03;
/// Shortest forward distance tried on top of a step.
const MIN_STEP_FORWARD: f32 = 0.1;

pub(crate) fn add_character_stepping_commands(app: &mut App) {
    app.register_command(
        "physics.set_stepping",
        "physics.set_stepping <name> <step_height> <slope_degrees>",
        set_stepping_command,
    )
    .register_command(
        "assert.grounded",
        "assert.grounded <name> <true|false>",
        assert_grounded_command,
    );
}

/// One character to move this tick.
struct Walker {
    name: String,
    entity: Entity,
    proxy: Entity,
    settings: CharacterStepping,
    ground: CharacterGround,
    half_height: f32,
    radius: f32,
}

/// The body as the physics world has it.
#[derive(Clone, Copy)]
struct Motion {
    position: Vec3,
    velocity: Vec3,
}

/// What the tick does with a character.
struct Outcome {
    position: Vec3,
    velocity: Vec3,
    ground: CharacterGround,
}

pub(crate) fn step_characters(world: &mut World) {
    let mut query = world.query_filtered::<(
        Entity,
        &Name,
        &PhysicsBody,
        &PhysicsProxy,
        &CharacterStepping,
        &CharacterGround,
    ), Without<RetiredEntity>>();
    let mut walkers: Vec<Walker> = query
        .iter(world)
        .filter_map(|(entity, name, body, proxy, settings, ground)| {
            let PhysicsShape::Capsule {
                half_height,
                radius,
            } = body.shape
            else {
                return None;
            };
            (body.kind == PhysicsBodyKind::Character).then(|| Walker {
                name: name.as_str().to_string(),
                entity,
                proxy: proxy.entity(),
                settings: *settings,
                ground: *ground,
                half_height,
                radius,
            })
        })
        .collect();
    walkers.sort_by(|left, right| left.name.cmp(&right.name));
    let mut moved = false;
    for walker in walkers {
        let Some(mut motion) = read_motion(world, walker.proxy) else {
            continue;
        };
        if let Some(wanted) = world.get::<PhysicsVelocitySet>(walker.entity) {
            motion.velocity = wanted.0;
        }
        let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
        let outcome = move_character(world, &walker, motion, dt);
        if outcome.position != motion.position {
            write_position(world, walker.proxy, outcome.position);
            moved = true;
        }
        let mut entity = world.entity_mut(walker.entity);
        if outcome.ground != walker.ground {
            entity.insert(outcome.ground);
        }
        entity.insert(PhysicsVelocitySet(outcome.velocity));
    }
    if moved {
        sync_body_readouts(world);
    }
}

fn read_motion(world: &mut World, proxy: Entity) -> Option<Motion> {
    let mut sets = world.query::<&RapierRigidBodySet>();
    let set = sets.single(world).ok()?;
    let body = set.bodies.get(*set.entity2body().get(&proxy)?)?;
    Some(Motion {
        position: body.translation(),
        velocity: body.linvel(),
    })
}

fn write_position(world: &mut World, proxy: Entity, position: Vec3) {
    let mut sets = world.query::<&mut RapierRigidBodySet>();
    if let Ok(mut set) = sets.single_mut(world)
        && let Some(handle) = set.entity2body().get(&proxy).copied()
        && let Some(body) = set.bodies.get_mut(handle)
    {
        body.set_translation(position, true);
    }
}

/// Sweeps the thinned capsule of `walker` from `origin`; the distance and surface normal there.
fn sweep(
    world: &mut World,
    walker: &Walker,
    origin: Vec3,
    direction: Vec3,
    distance: f32,
) -> Option<(f32, Vec3, Vec3)> {
    cast_shape(
        world,
        &ShapeCast {
            shape: PhysicsShape::Capsule {
                half_height: walker.half_height,
                radius: walker.radius - SKIN,
            },
            origin,
            rotation: Quat::IDENTITY,
            direction,
            max_distance: distance,
            exclude: vec![walker.entity],
        },
    )
    .map(|hit| (hit.distance, hit.normal, hit.point))
}

fn is_walkable(normal: Vec3, max_slope_degrees: f32) -> bool {
    normal.y >= max_slope_degrees.to_radians().cos()
}

/// A surface a character can stand on.
struct Surface {
    normal: Vec3,
    /// Its height where it was touched.
    height: f32,
}

/// The walkable surface at a contact: the contact's own, or, on the edge of a step (where the
/// capsule touches at a slant), the surface just past the edge.
fn walkable_surface(
    world: &mut World,
    walker: &Walker,
    center: Vec3,
    normal: Vec3,
    point: Vec3,
) -> Option<Surface> {
    if is_walkable(normal, walker.settings.max_slope_degrees) {
        return Some(Surface {
            normal,
            height: point.y,
        });
    }
    let away = Vec3::new(point.x - center.x, 0.0, point.z - center.z).normalize_or_zero();
    let hit = cast_ray(
        world,
        &RayCast {
            origin: point + away * EDGE_REACH + Vec3::Y * EDGE_REACH,
            direction: Vec3::NEG_Y,
            max_distance: EDGE_REACH * 2.0,
            exclude: vec![walker.entity],
        },
    )?;
    // A distance of 0 means the ray started inside the obstacle: a wall, not a step.
    (hit.distance > 0.0 && is_walkable(hit.normal, walker.settings.max_slope_degrees)).then_some(
        Surface {
            normal: hit.normal,
            height: hit.point.y,
        },
    )
}

/// The horizontal velocity laid along a surface, keeping its horizontal speed.
fn along_surface(horizontal: Vec3, normal: Vec3) -> Vec3 {
    let rise = -(normal.x * horizontal.x + normal.z * horizontal.z) / normal.y;
    Vec3::new(horizontal.x, rise, horizontal.z)
}

fn move_character(world: &mut World, walker: &Walker, motion: Motion, dt: f32) -> Outcome {
    let airborne = Outcome {
        position: motion.position,
        velocity: motion.velocity,
        ground: CharacterGround::default(),
    };
    // Walking up a slope also moves upwards, so only a grounded character may rise that fast.
    let jumping = motion.velocity.y > RISING_SPEED
        && !(walker.ground.grounded && walker.ground.normal.y < 0.999);
    if jumping {
        return airborne;
    }
    let mut position = motion.position;
    let mut horizontal = Vec3::new(motion.velocity.x, 0.0, motion.velocity.z);
    if walker.ground.grounded && horizontal != Vec3::ZERO {
        match climb_step(world, walker, position, horizontal, dt) {
            Climb::Up(higher) => {
                return Outcome {
                    position: higher,
                    velocity: horizontal,
                    ground: walker.ground,
                };
            }
            Climb::Blocked(wall) => {
                // Take the part towards the obstacle out, so it is not climbed.
                let into = horizontal.dot(wall).min(0.0);
                horizontal -= wall * into;
            }
            Climb::Free => {}
        }
    }
    let reach = if walker.ground.grounded {
        walker.settings.max_step_height
    } else {
        LANDING_PROBE
    };
    let Some((distance, normal, point)) = sweep(world, walker, position, Vec3::NEG_Y, SKIN + reach)
    else {
        return Outcome {
            velocity: Vec3::new(horizontal.x, motion.velocity.y, horizontal.z),
            ..airborne
        };
    };
    let Some(surface) = walkable_surface(world, walker, position, normal, point) else {
        return Outcome {
            velocity: Vec3::new(horizontal.x, motion.velocity.y, horizontal.z),
            ..airborne
        };
    };
    // Stay on the ground when it drops away (a step down, a slope).
    let gap = distance - SKIN;
    if walker.ground.grounded && gap > 0.005 {
        position.y -= gap;
    }
    Outcome {
        position,
        velocity: along_surface(horizontal, surface.normal),
        ground: CharacterGround {
            grounded: true,
            normal: surface.normal,
        },
    }
}

enum Climb {
    /// Nothing steep in the way.
    Free,
    /// The body goes up onto a step.
    Up(Vec3),
    /// Something too high or steep; its horizontal normal.
    Blocked(Vec3),
}

fn climb_step(
    world: &mut World,
    walker: &Walker,
    position: Vec3,
    horizontal: Vec3,
    dt: f32,
) -> Climb {
    let direction = horizontal.normalize();
    let travel = horizontal.length() * dt;
    let ahead = sweep(world, walker, position, direction, travel + SKIN * 2.0);
    let Some((_, normal, point)) = ahead else {
        return Climb::Free;
    };
    let feet = position.y - walker.half_height - walker.radius;
    // A touch at foot level is the ground itself (for example along its edge), not an obstacle.
    if is_walkable(normal, walker.settings.max_slope_degrees) || point.y <= feet + SKIN {
        return Climb::Free;
    }
    let wall = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    let height = walker.settings.max_step_height;
    let rise = sweep(world, walker, position, Vec3::Y, height).map_or(height, |hit| hit.0);
    let raised = position + Vec3::Y * rise;
    let wanted = travel.max(MIN_STEP_FORWARD);
    let forward = sweep(world, walker, raised, direction, wanted).map_or(wanted, |hit| hit.0);
    if forward <= SKIN {
        return Climb::Blocked(wall);
    }
    let over = raised + direction * forward;
    let Some((down, normal, point)) = sweep(world, walker, over, Vec3::NEG_Y, rise + SKIN) else {
        return Climb::Blocked(wall);
    };
    let lift = rise - down + SKIN;
    if lift <= 0.005 || lift > height + SKIN {
        return Climb::Blocked(wall);
    }
    // The top of the step, not just the part touched, must be within a step of the feet: otherwise
    // a high edge would be climbed a bit each tick.
    match walkable_surface(world, walker, over, normal, point) {
        Some(surface) if surface.height - feet <= height + SKIN => {
            Climb::Up(position + Vec3::Y * lift)
        }
        _ => Climb::Blocked(wall),
    }
}

/// `physics.set_stepping <name> <step_height> <slope_degrees>`.
fn set_stepping_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let settings = CharacterStepping {
        max_step_height: args.parse(1, "step_height")?,
        max_slope_degrees: args.parse(2, "slope_degrees")?,
    };
    if !(settings.max_step_height >= 0.0 && (0.0..90.0).contains(&settings.max_slope_degrees)) {
        return Err(args.failure("the step height is at least 0 and the slope from 0 to below 90"));
    }
    let entity = find_unique_entity_by_name(world, args, &name)?;
    world.entity_mut(entity).insert(settings);
    Ok(CommandOutcome::Continue)
}

/// `assert.grounded <name> <true|false>`.
fn assert_grounded_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let expected: bool = args.parse(1, "grounded")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    match world.get::<CharacterGround>(entity) {
        Some(ground) if ground.grounded == expected => Ok(CommandOutcome::Continue),
        Some(ground) => Err(args.failure(format!(
            "`{name}` grounded is {}, expected {expected}",
            ground.grounded
        ))),
        None => Err(args.failure(format!("`{name}` has no CharacterStepping"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_follows_the_surface_at_the_same_horizontal_speed() {
        let flat = along_surface(Vec3::new(3.0, 0.0, 0.0), Vec3::Y);
        assert_eq!(flat, Vec3::new(3.0, 0.0, 0.0));
        // A 45 degree slope that rises towards +x.
        let normal = Vec3::new(-1.0, 1.0, 0.0).normalize();
        let up = along_surface(Vec3::new(3.0, 0.0, 0.0), normal);
        assert!(up.distance(Vec3::new(3.0, 3.0, 0.0)) < 1e-5, "{up}");
        assert!(is_walkable(normal, 50.0));
        assert!(!is_walkable(normal, 40.0));
    }
}
