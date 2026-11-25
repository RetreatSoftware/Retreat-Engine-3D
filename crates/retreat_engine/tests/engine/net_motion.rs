//! Teleport, steps and slopes, and render smoothing, alone and over the network.

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::{prepare_simulation_app, simulate_tick};
use retreat_engine::{RenderSmoothingPlugin, capture_physics, restore_physics};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CharacterGround, CharacterStepping, CommandArgs, CommandOutcome,
    CommandResult, NetState, PhysicsBody, PhysicsBodyKind, PhysicsControlSystems, PhysicsShape,
    PhysicsTeleport, PhysicsVelocity, PhysicsVelocitySet, RenderSmoothing, run_command_line,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

const HALF_HEIGHT: f32 = 0.5;
const RADIUS: f32 = 0.4;
/// Height of a standing character's center above the floor.
const STANDING: f32 = HALF_HEIGHT + RADIUS;
const GRAVITY: f32 = 19.6;

/// The velocity a test character wants.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Heading {
    x: f32,
    jump: bool,
}

impl NetState for Heading {
    const ID: &'static str = "test.heading";
}

/// `test.walk <name> <x> <jump 0|1>`.
fn walk_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let x: f32 = args.parse(1, "x")?;
    let jump: u8 = args.parse(2, "jump")?;
    for (found, mut heading) in world.query::<(&Name, &mut Heading)>().iter_mut(world) {
        if found.as_str() == name {
            *heading = Heading { x, jump: jump != 0 };
        }
    }
    Ok(CommandOutcome::Continue)
}

fn control(
    time: Res<Time<Fixed>>,
    walkers: Query<(Entity, &Heading, &CharacterGround, &PhysicsVelocity)>,
    mut commands: Commands,
) {
    let dt = time.timestep().as_secs_f32();
    for (entity, heading, ground, velocity) in &walkers {
        let vertical = match (ground.grounded, heading.jump) {
            (true, true) => 7.0,
            (true, false) => 0.0,
            (false, _) => velocity.linear.y - GRAVITY * dt,
        };
        commands
            .entity(entity)
            .insert(PhysicsVelocitySet(Vec3::new(heading.x, vertical, 0.0)));
    }
}

fn motion_app() -> App {
    let mut app = make_app();
    app.add_plugins(RenderSmoothingPlugin)
        .register_net_state::<Heading>()
        .register_command("test.walk", "test.walk <name> <x> <jump>", walk_command)
        .add_systems(FixedUpdate, control.in_set(PhysicsControlSystems));
    app
}

fn character(name: &str, x: f32) -> (Name, PhysicsBody, Transform, Heading, CharacterStepping) {
    (
        Name::new(name.to_string()),
        PhysicsBody::character(HALF_HEIGHT, RADIUS),
        Transform::from_xyz(x, STANDING + 0.01, 0.0),
        Heading::default(),
        CharacterStepping {
            max_step_height: 0.3,
            max_slope_degrees: 45.0,
        },
    )
}

/// A fixed box from `min_x` to `max_x` whose top is at `top`.
fn block(name: &str, min_x: f32, max_x: f32, top: f32) -> (Name, PhysicsBody, Transform) {
    (
        Name::new(name.to_string()),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::new((max_x - min_x) / 2.0, top / 2.0, 4.0),
            },
            shape_offset: Vec3::ZERO,
        },
        Transform::from_xyz((min_x + max_x) / 2.0, top / 2.0, 0.0),
    )
}

/// Five steps of 0.2 up to a platform 1 high from x 2.
fn staircase(world: &mut World) {
    for step in 0..5 {
        let start = 2.0 + 0.5 * step as f32;
        let end = if step == 4 { 12.0 } else { start + 0.5 };
        world.spawn(block(
            &format!("step_{step}"),
            start,
            end,
            0.2 * (step + 1) as f32,
        ));
    }
}

/// A slope starting at x 2 that rises towards +x.
fn ramp(world: &mut World, degrees: f32) {
    let angle = degrees.to_radians();
    let (length, thickness) = (8.0, 0.5);
    let along = Vec3::new(angle.cos(), angle.sin(), 0.0);
    let up = Vec3::new(-angle.sin(), angle.cos(), 0.0);
    let center = Vec3::new(2.0, 0.0, 0.0) + along * (length / 2.0) - up * (thickness / 2.0);
    world.spawn((
        Name::new("ramp"),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::new(length / 2.0, thickness / 2.0, 4.0),
            },
            shape_offset: Vec3::ZERO,
        },
        Transform::from_translation(center).with_rotation(Quat::from_rotation_z(angle)),
    ));
}

fn scene_app(scene: impl FnOnce(&mut World)) -> App {
    let mut app = motion_app();
    prepare_simulation_app(&mut app);
    let world = app.world_mut();
    scene(world);
    world.spawn(character("hero", 0.0));
    ticks(world, 30);
    app
}

fn ticks(world: &mut World, count: u32) {
    for _ in 0..count {
        simulate_tick(world);
    }
}

fn entity(world: &mut World, name: &str) -> Entity {
    world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no `{name}`"))
}

fn position(world: &mut World, name: &str) -> Vec3 {
    let found = entity(world, name);
    world
        .get::<Transform>(found)
        .expect("transform")
        .translation
}

fn grounded(world: &mut World, name: &str) -> bool {
    let found = entity(world, name);
    world
        .get::<CharacterGround>(found)
        .expect("ground")
        .grounded
}

fn walk(world: &mut World, x: f32, count: u32) {
    run_command_line(world, &format!("test.walk hero {x} 0")).expect("walk");
    ticks(world, count);
}

/// Walks and returns how many of the ticks the character was off the ground.
fn walk_counting_airborne(world: &mut World, x: f32, count: u32) -> u32 {
    run_command_line(world, &format!("test.walk hero {x} 0")).expect("walk");
    (0..count)
        .filter(|_| {
            simulate_tick(world);
            !grounded(world, "hero")
        })
        .count() as u32
}

#[test]
fn a_character_climbs_a_staircase_and_walks_down_it_on_the_ground() {
    let mut app = scene_app(staircase);
    let world = app.world_mut();
    let airborne = walk_counting_airborne(world, 4.0, 110);
    let top = position(world, "hero");
    assert!(top.x > 5.0, "{top}");
    assert!((top.y - (1.0 + STANDING)).abs() < 0.05, "{top}");
    assert!(airborne <= 2, "{airborne} ticks off the ground going up");
    let airborne = walk_counting_airborne(world, -4.0, 110);
    let bottom = position(world, "hero");
    assert!(bottom.x < 1.0, "{bottom}");
    assert!((bottom.y - STANDING).abs() < 0.05, "{bottom}");
    assert!(airborne <= 4, "{airborne} ticks off the ground going down");
}

#[test]
fn a_step_that_is_too_high_stops_the_character_and_a_jump_still_works() {
    let mut app = scene_app(|world| {
        world.spawn(block("wall", 2.0, 4.0, 0.6));
    });
    let world = app.world_mut();
    walk(world, 4.0, 100);
    let stopped = position(world, "hero");
    assert!((stopped.x - (2.0 - RADIUS)).abs() < 0.08, "{stopped}");
    assert!((stopped.y - STANDING).abs() < 0.05, "{stopped}");
    run_command_line(world, "test.walk hero 4 1").expect("jump");
    ticks(world, 1);
    walk(world, 4.0, 20);
    assert!(position(world, "hero").y > STANDING + 0.6, "jumped");
}

#[test]
fn a_character_walks_a_gentle_slope_without_sliding_and_not_a_steep_one() {
    let mut app = scene_app(|world| ramp(world, 30.0));
    let world = app.world_mut();
    let airborne = walk_counting_airborne(world, 3.0, 140);
    let up = position(world, "hero");
    assert!(up.x > 5.0 && up.y > STANDING + 1.5, "{up}");
    assert!(airborne <= 3, "{airborne} ticks off the ground");
    walk(world, 0.0, 10);
    let standing = position(world, "hero");
    walk(world, 0.0, 120);
    let later = position(world, "hero");
    assert!(standing.distance(later) < 0.02, "slid to {later}");
    let airborne = walk_counting_airborne(world, -3.0, 60);
    assert!(airborne <= 3, "{airborne} ticks off the ground going down");

    let mut app = scene_app(|world| ramp(world, 60.0));
    let world = app.world_mut();
    walk(world, 3.0, 200);
    let blocked = position(world, "hero");
    assert!(blocked.y < STANDING + 0.35, "climbed to {blocked}");
}

#[test]
fn a_teleport_moves_a_body_at_rest_and_it_walks_on_from_there() {
    let mut app = scene_app(|_| {});
    let world = app.world_mut();
    walk(world, 4.0, 20);
    let hero = entity(world, "hero");
    world
        .entity_mut(hero)
        .insert(PhysicsTeleport(Vec3::new(-8.0, 3.0, 1.0)));
    ticks(world, 1);
    assert!(position(world, "hero").distance(Vec3::new(-8.0, 3.0, 1.0)) < 0.1);
    assert!(world.get::<PhysicsTeleport>(hero).is_none());
    walk(world, 4.0, 100);
    let landed = position(world, "hero");
    assert!(
        landed.x > -2.5 && (landed.y - STANDING).abs() < 0.05,
        "{landed}"
    );
}

#[test]
fn a_teleport_waits_for_the_body_and_a_rewind_takes_back_a_later_request() {
    let mut app = scene_app(|_| {});
    let world = app.world_mut();
    let late = world
        .spawn((
            character("late", 3.0),
            PhysicsTeleport(Vec3::new(5.0, STANDING, 0.0)),
        ))
        .id();
    ticks(world, 3);
    assert!((position(world, "late").x - 5.0).abs() < 1e-3);
    let snapshot = capture_physics(world).expect("capture");
    world
        .entity_mut(late)
        .insert(PhysicsTeleport(Vec3::new(-5.0, STANDING, 0.0)));
    restore_physics(world, &snapshot).expect("restore");
    assert!(world.get::<PhysicsTeleport>(late).is_none());
    ticks(world, 2);
    assert!((position(world, "late").x - 5.0).abs() < 1e-3);
}

fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> LinkConditions {
    LinkConditions {
        latency_ticks,
        jitter_ticks,
        loss,
    }
}

/// Two clients and a character the server spawned in front of a staircase.
fn stairs_harness(margin_ticks: i64, conditions: LinkConditions) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks,
        state_interval_ticks: 2,
        up: conditions,
        down: conditions,
        seed: 4,
    };
    let make = || {
        let mut app = motion_app();
        staircase(app.world_mut());
        app
    };
    let mut harness = NetHarness::new(config, &["test.walk"], 2, make);
    for _ in 0..600 {
        if harness.all_joined() {
            break;
        }
        harness.run_idle(1);
    }
    assert!(harness.all_joined());
    harness.server_app.world_mut().spawn(character("hero", 0.0));
    harness.run_idle(120);
    harness
}

fn send(harness: &mut NetHarness, line: &str) {
    harness.step(&[(0, vec![line.to_string()])]);
}

fn assert_all_at_the_same_place(harness: &mut NetHarness) {
    let server = position(harness.server_app.world_mut(), "hero");
    for index in 0..2 {
        let client = position(harness.client_apps[index].world_mut(), "hero");
        assert!(
            client.distance(server) < 2e-3,
            "client {index}: {client} vs {server}"
        );
        assert_eq!(harness.client(index).stats.refused_rollbacks, 0);
    }
}

#[test]
fn stairs_are_predicted_without_corrections() {
    let mut harness = stairs_harness(10, link(5, 0, 0.0));
    send(&mut harness, "test.walk hero 4 0");
    let before = harness.client(0).stats.corrections;
    harness.run_idle(110);
    assert_eq!(harness.client(0).stats.corrections, before);
    send(&mut harness, "test.walk hero 0 0");
    harness.run_idle(200);
    assert!(position(harness.server_app.world_mut(), "hero").y > 1.5);
    assert_all_at_the_same_place(&mut harness);
}

#[test]
fn a_server_teleport_reaches_lagging_clients_and_stairs_converge_with_loss() {
    let mut harness = stairs_harness(6, link(6, 3, 0.15));
    send(&mut harness, "test.walk hero 4 0");
    harness.run_idle(60);
    let server = harness.server_app.world_mut();
    let hero = entity(server, "hero");
    server
        .entity_mut(hero)
        .insert(PhysicsTeleport(Vec3::new(-6.0, STANDING, 0.0)));
    harness.run_idle(200);
    send(&mut harness, "test.walk hero 0 0");
    // Inputs are not resent: make sure the stop arrives.
    for _ in 0..5 {
        send(&mut harness, "test.walk hero 0 0");
    }
    harness.run_idle(300);
    let after = position(harness.server_app.world_mut(), "hero");
    assert!(after.x > -6.0 && after.y > 1.5, "walked back up: {after}");
    assert_all_at_the_same_place(&mut harness);
}

/// Draws one frame, advancing the real clock by `seconds`.
fn draw(world: &mut World, seconds: f32) -> Vec3 {
    world
        .resource_mut::<Time<Real>>()
        .update_with_duration(std::time::Duration::from_secs_f32(seconds));
    let real = world.resource::<Time<Real>>().as_generic();
    *world.resource_mut::<Time>() = real;
    world.run_schedule(PostUpdate);
    let hero = entity(world, "hero");
    world
        .get::<GlobalTransform>(hero)
        .expect("drawn")
        .translation()
}

#[test]
fn a_correction_is_drawn_smoothly_while_the_simulation_is_exact() {
    let mut harness = stairs_harness(8, link(4, 0, 0.0));
    let client = harness.client_apps[1].world_mut();
    let hero = entity(client, "hero");
    client.entity_mut(hero).insert(RenderSmoothing {
        half_life: 0.1,
        snap_distance: 2.0,
    });
    harness.run_idle(10);
    let drawn_before = draw(harness.client_apps[1].world_mut(), 0.001);
    let server = harness.server_app.world_mut();
    let server_hero = entity(server, "hero");
    server
        .entity_mut(server_hero)
        .insert(PhysicsTeleport(Vec3::new(-1.0, STANDING, 0.0)));
    let corrections = harness.client(1).stats.corrections;
    while harness.client(1).stats.corrections == corrections {
        harness.run_idle(1);
    }
    let client = harness.client_apps[1].world_mut();
    let simulated = position(client, "hero");
    assert!((simulated.x + 1.0).abs() < 0.01, "the truth: {simulated}");
    let drawn = draw(client, 0.001);
    assert!(drawn.distance(drawn_before) < 0.05, "no snap: {drawn}");
    assert_eq!(position(client, "hero"), simulated, "untouched by drawing");
    let half = draw(client, 0.1);
    assert!((half.x + 0.5).abs() < 0.05, "half way: {half}");
    let settled = draw(client, 2.0);
    assert!(settled.distance(simulated) < 0.001, "{settled}");
}
