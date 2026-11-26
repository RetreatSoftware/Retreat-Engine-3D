//! Answers the contract's ray and shape casts (`retreat_engine_api/src/physics_queries.rs`) and
//! registers the cast commands.
//!
//! A cast tests the shape of every live body at the pose the physics body set holds now. It does
//! not use Rapier's broad-phase index, which only learns new positions at the next physics step, so
//! a cast right after a client's correction would see old positions and disagree with the server.
//! Cost is one test per body per cast; revisit (use the index) with worlds of thousands of bodies.
//!
//! Commands: `physics.raycast <ox> <oy> <oz> <dx> <dy> <dz> <max>` (logs the hit),
//! `assert.ray_hit <name> <distance> <ox> <oy> <oz> <dx> <dy> <dz> <max>`,
//! `assert.ray_miss <ox> <oy> <oz> <dx> <dy> <dz> <max>`,
//! `assert.ball_cast_hit <name> <distance> <radius> <ox> <oy> <oz> <dx> <dy> <dz> <max>`.

use std::collections::BTreeMap;

use bevy::prelude::*;
use bevy_rapier3d::plugin::context::RapierRigidBodySet;
use bevy_rapier3d::rapier::math::Pose;
use bevy_rapier3d::rapier::parry::query::{Ray, ShapeCastOptions, cast_shapes};
use bevy_rapier3d::rapier::parry::shape::SharedShape;
use retreat_engine_api::{
    AppCommandExt, CastHit, CommandArgs, CommandError, CommandOutcome, CommandResult, Intangible,
    LagCaster, PhysicsBody, PhysicsBodyKind, PhysicsCaster, PhysicsShape, RayCast, RetiredEntity,
    ShapeCast,
};

use crate::net::lag_compensation::PoseHistory;
use crate::physics::PhysicsProxy;

/// How close a hit's distance must be to the expected one in the assert commands.
const ASSERT_DISTANCE_TOLERANCE: f32 = 0.05;

/// Installs the [`PhysicsCaster`] and the cast commands (part of `PhysicsPlugin`).
pub(crate) fn add_physics_queries(app: &mut App) {
    app.init_resource::<PoseHistory>()
        .insert_resource(LagCaster {
            ray: cast_ray_in_past,
            shape: cast_shape_in_past,
        })
        .register_command(
            "assert.lag_ray_hit",
            "assert.lag_ray_hit <name> <view_tick> <ox> <oy> <oz> <dx> <dy> <dz> <max>",
            assert_lag_ray_hit_command,
        );
    app.insert_resource(PhysicsCaster {
        ray: cast_ray_in_world,
        shape: cast_shape_in_world,
    })
    .register_command(
        "physics.raycast",
        "physics.raycast <ox> <oy> <oz> <dx> <dy> <dz> <max>",
        raycast_command,
    )
    .register_command(
        "assert.ray_hit",
        "assert.ray_hit <name> <distance> <ox> <oy> <oz> <dx> <dy> <dz> <max>",
        assert_ray_hit_command,
    )
    .register_command(
        "assert.ray_miss",
        "assert.ray_miss <ox> <oy> <oz> <dx> <dy> <dz> <max>",
        assert_ray_miss_command,
    )
    .register_command(
        "assert.ball_cast_hit",
        "assert.ball_cast_hit <name> <distance> <radius> <ox> <oy> <oz> <dx> <dy> <dz> <max>",
        assert_ball_cast_hit_command,
    );
}

/// The one mapping of a contract shape onto the physics library's geometry for casts; the
/// simulation's colliders come from `physics.rs::rapier_components`.
fn parry_shape(shape: PhysicsShape) -> SharedShape {
    match shape {
        PhysicsShape::Cuboid { half_extents } => {
            SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z)
        }
        PhysicsShape::Ball { radius } => SharedShape::ball(radius),
        PhysicsShape::Capsule {
            half_height,
            radius,
        } => SharedShape::capsule_y(half_height, radius),
    }
}

/// One body a cast can meet.
struct Target {
    entity: Entity,
    name: String,
    /// Never moves.
    fixed: bool,
    shape: SharedShape,
    pose: Pose,
}

/// Every live body not in `exclude`, sorted by name.
fn targets(world: &mut World, exclude: &[Entity]) -> Vec<Target> {
    let mut owners = world.query_filtered::<
        (Entity, &Name, &PhysicsBody, &PhysicsProxy),
        (Without<RetiredEntity>, Without<Intangible>),
    >();
    let owners: Vec<(Entity, String, PhysicsBody, Entity)> = owners
        .iter(world)
        .filter(|(entity, ..)| !exclude.contains(entity))
        .map(|(entity, name, body, proxy)| {
            let name = name.as_str().to_string();
            (entity, name, body.clone(), proxy.entity())
        })
        .collect();
    let mut sets = world.query::<&RapierRigidBodySet>();
    let Ok(set) = sets.single(world) else {
        return Vec::new();
    };
    let mut targets: Vec<Target> = owners
        .into_iter()
        .filter_map(|(entity, name, description, proxy)| {
            let body = set.bodies.get(*set.entity2body().get(&proxy)?)?;
            let rotation = *body.rotation();
            let center = body.translation() + rotation * description.shape_offset;
            Some(Target {
                entity,
                name,
                fixed: description.kind == PhysicsBodyKind::Fixed,
                shape: parry_shape(description.shape),
                pose: Pose::from_parts(center, rotation),
            })
        })
        .collect();
    targets.sort_by(|left, right| left.name.cmp(&right.name));
    targets
}

/// The targets as they were at server tick `view_tick`: fixed bodies as they are, moving bodies
/// from the pose history (a body without a record at that time cannot be hit). The present when
/// there is no history or the tick is not positive.
fn targets_at(world: &mut World, exclude: &[Entity], view_tick: f64) -> Vec<Target> {
    let past = (view_tick > 0.0)
        .then(|| world.get_resource::<PoseHistory>()?.at(view_tick))
        .flatten();
    let Some(past) = past else {
        return targets(world, exclude);
    };
    // Moving bodies by name, removed ones included: the shooter saw them.
    let mut entities: BTreeMap<String, Entity> = BTreeMap::new();
    let mut owners =
        world.query_filtered::<(Entity, &Name, Has<RetiredEntity>), With<PhysicsBody>>();
    for (entity, name, retired) in owners.iter(world) {
        if !retired || !entities.contains_key(name.as_str()) {
            entities.insert(name.as_str().to_string(), entity);
        }
    }
    let mut candidates = targets(world, exclude);
    candidates.retain(|target| !past.contains_key(&target.name) && target.fixed);
    for (name, body) in past {
        let Some(description) = body.description else {
            continue;
        };
        let Some(entity) = entities.get(&name).copied() else {
            continue;
        };
        // Whether it can be hit at all is judged now, like health.
        if exclude.contains(&entity) || world.get::<Intangible>(entity).is_some() {
            continue;
        }
        let center = body.translation + body.rotation * description.shape_offset;
        candidates.push(Target {
            entity,
            name,
            fixed: false,
            shape: parry_shape(description.shape),
            pose: Pose::from_parts(center, body.rotation),
        });
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates
}

/// The unit direction of a cast, or `None` when the cast is invalid.
fn valid_direction(origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Vec3> {
    let valid = origin.is_finite() && max_distance.is_finite() && max_distance >= 0.0;
    valid.then(|| direction.try_normalize()).flatten()
}

/// Keeps the closer hit; the earlier one (first name) wins a tie.
fn keep_closer(best: &mut Option<CastHit>, hit: CastHit) {
    if best.as_ref().is_none_or(|old| hit.distance < old.distance) {
        *best = Some(hit);
    }
}

fn cast_ray_in_world(world: &mut World, cast: &RayCast) -> Option<CastHit> {
    let candidates = targets(world, &cast.exclude);
    closest_ray_hit(candidates, cast)
}

fn cast_ray_in_past(world: &mut World, cast: &RayCast, view_tick: f64) -> Option<CastHit> {
    let candidates = targets_at(world, &cast.exclude, view_tick);
    closest_ray_hit(candidates, cast)
}

fn closest_ray_hit(candidates: Vec<Target>, cast: &RayCast) -> Option<CastHit> {
    let direction = valid_direction(cast.origin, cast.direction, cast.max_distance)?;
    let ray = Ray::new(cast.origin, direction);
    let mut best = None;
    for target in candidates {
        let hit = target
            .shape
            .cast_ray_and_get_normal(&target.pose, &ray, cast.max_distance, true);
        if let Some(hit) = hit {
            keep_closer(
                &mut best,
                CastHit {
                    entity: target.entity,
                    name: target.name,
                    distance: hit.time_of_impact,
                    point: cast.origin + direction * hit.time_of_impact,
                    normal: hit.normal,
                },
            );
        }
    }
    best
}

fn cast_shape_in_world(world: &mut World, cast: &ShapeCast) -> Option<CastHit> {
    let candidates = targets(world, &cast.exclude);
    closest_shape_hit(candidates, cast)
}

fn cast_shape_in_past(world: &mut World, cast: &ShapeCast, view_tick: f64) -> Option<CastHit> {
    let candidates = targets_at(world, &cast.exclude, view_tick);
    closest_shape_hit(candidates, cast)
}

fn closest_shape_hit(candidates: Vec<Target>, cast: &ShapeCast) -> Option<CastHit> {
    let direction = valid_direction(cast.origin, cast.direction, cast.max_distance)?;
    if !cast.rotation.is_normalized() || !cast.shape.is_valid() {
        return None;
    }
    let shape = parry_shape(cast.shape);
    let pose = Pose::from_parts(cast.origin, cast.rotation);
    let options = ShapeCastOptions {
        max_time_of_impact: cast.max_distance,
        ..ShapeCastOptions::default()
    };
    let mut best = None;
    for target in candidates {
        // A velocity of length 1 makes the time of impact the distance travelled.
        let result = cast_shapes(
            &pose,
            direction,
            shape.as_ref(),
            &target.pose,
            Vec3::ZERO,
            target.shape.as_ref(),
            options,
        );
        let Ok(Some(hit)) = result else {
            continue;
        };
        keep_closer(
            &mut best,
            CastHit {
                entity: target.entity,
                name: target.name,
                distance: hit.time_of_impact,
                point: target.pose.transform_point(hit.witness2),
                normal: target.pose.rotation * hit.normal2,
            },
        );
    }
    best
}

/// Seven numbers from `first` on: origin, direction, maximum distance.
fn parse_ray(args: &CommandArgs, first: usize) -> Result<RayCast, CommandError> {
    let number = |offset: usize, name: &str| args.parse::<f32>(first + offset, name);
    Ok(RayCast {
        origin: Vec3::new(number(0, "ox")?, number(1, "oy")?, number(2, "oz")?),
        direction: Vec3::new(number(3, "dx")?, number(4, "dy")?, number(5, "dz")?),
        max_distance: number(6, "max")?,
        exclude: Vec::new(),
    })
}

fn check_hit(args: &CommandArgs, hit: Option<CastHit>) -> CommandResult {
    let name: String = args.parse(0, "name")?;
    let distance: f32 = args.parse(1, "distance")?;
    match hit {
        Some(hit)
            if hit.name == name && (hit.distance - distance).abs() <= ASSERT_DISTANCE_TOLERANCE =>
        {
            Ok(CommandOutcome::Continue)
        }
        Some(hit) => Err(args.failure(format!(
            "the cast hit `{}` at {}, expected `{name}` at {distance}",
            hit.name, hit.distance
        ))),
        None => Err(args.failure(format!("the cast hit nothing, expected `{name}`"))),
    }
}

fn raycast_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(7)?;
    let cast = parse_ray(args, 0)?;
    match cast_ray_in_world(world, &cast) {
        Some(hit) => info!(
            "raycast hit `{}` at {} point {} normal {}",
            hit.name, hit.distance, hit.point, hit.normal
        ),
        None => info!("raycast hit nothing"),
    }
    Ok(CommandOutcome::Continue)
}

fn assert_ray_hit_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(9)?;
    let cast = parse_ray(args, 2)?;
    let hit = cast_ray_in_world(world, &cast);
    check_hit(args, hit)
}

fn assert_ray_miss_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(7)?;
    let cast = parse_ray(args, 0)?;
    match cast_ray_in_world(world, &cast) {
        None => Ok(CommandOutcome::Continue),
        Some(hit) => Err(args.failure(format!(
            "the ray hit `{}` at {}, expected nothing",
            hit.name, hit.distance
        ))),
    }
}

fn assert_ball_cast_hit_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(10)?;
    let radius: f32 = args.parse(2, "radius")?;
    let ray = parse_ray(args, 3)?;
    let cast = ShapeCast {
        shape: PhysicsShape::Ball { radius },
        origin: ray.origin,
        rotation: Quat::IDENTITY,
        direction: ray.direction,
        max_distance: ray.max_distance,
        exclude: Vec::new(),
    };
    let hit = cast_shape_in_world(world, &cast);
    check_hit(args, hit)
}

/// `assert.lag_ray_hit <name> <view_tick> <ox> ... <max>`: the lag-compensated ray meets `name`
/// first.
fn assert_lag_ray_hit_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(9)?;
    let name: String = args.parse(0, "name")?;
    let view_tick: f64 = args.parse(1, "view_tick")?;
    let cast = parse_ray(args, 2)?;
    match cast_ray_in_past(world, &cast, view_tick) {
        Some(hit) if hit.name == name => Ok(CommandOutcome::Continue),
        Some(hit) => Err(args.failure(format!("the ray hit `{}`, expected `{name}`", hit.name))),
        None => Err(args.failure(format!("the ray hit nothing, expected `{name}`"))),
    }
}
