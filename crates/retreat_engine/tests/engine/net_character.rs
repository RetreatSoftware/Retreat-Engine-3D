//! What a character controller needs, together: analog input carried in networked per-tick
//! command lines, ray and shape casts inside the tick, upright character bodies with velocity
//! readout and set, and all of it predicted, rewound and replicated.

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::net::{prepare_simulation_app, simulate_tick};
use retreat_engine::{InputActionsPlugin, capture_physics, compute_state_hash, restore_physics};
use retreat_engine_api::{
    AnalogInput, AppAxisExt, AppCommandExt, AppInputExt, AppNetStateExt, CommandArgs,
    CommandOutcome, CommandResult, HELD_ONLY, InputAction, InputActionSystems, InputAxis, InputKey,
    NetCommandSender, NetInputQueue, NetLocalClient, NetState, PhysicsBody, PhysicsBodyKind,
    PhysicsControlSystems, PhysicsShape, PhysicsVelocity, PhysicsVelocitySet, RayCast,
    RemovePhysicsBody, ShapeCast, cast_ray, cast_shape, run_command_line,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

const HALF_HEIGHT: f32 = 0.5;
const RADIUS: f32 = 0.4;

/// The input a mover got last, and what it saw: state a rewind must repeat exactly.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Mover {
    owner: u32,
    move_x: f32,
    yaw: f32,
    grounded: bool,
    /// Distance to whatever is ahead (from a ray cast), 100 when nothing is.
    ahead: f32,
}

impl NetState for Mover {
    const ID: &'static str = "test.mover";
}

#[derive(Resource, Default)]
struct Aim(f32);

/// `test.drive <move_x> <yaw>`: the sender's input.
fn drive_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let move_x: f32 = args.parse(0, "move_x")?;
    let yaw: f32 = args.parse(1, "yaw")?;
    let Some(owner) = world.resource::<NetCommandSender>().0 else {
        return Err(args.failure("only a client drives"));
    };
    for mut mover in world.query::<&mut Mover>().iter_mut(world) {
        if mover.owner == owner {
            mover.move_x = move_x;
            mover.yaw = yaw;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn send_input(
    analog: Res<AnalogInput>,
    local: Res<NetLocalClient>,
    mut aim: ResMut<Aim>,
    mut queue: ResMut<NetInputQueue>,
) {
    aim.0 += analog.look().x * 0.01;
    if local.0.is_some() {
        queue.push(format!("test.drive {} {}", analog.axis("move_x"), aim.0));
    }
}

fn control(world: &mut World) {
    let mut movers: Vec<(String, Entity, Mover, Vec3)> = world
        .query::<(&Name, Entity, &Mover, &Transform)>()
        .iter(world)
        .map(|(name, entity, mover, transform)| {
            let name = name.as_str().to_string();
            (name, entity, mover.clone(), transform.translation)
        })
        .collect();
    movers.sort_by(|left, right| left.0.cmp(&right.0));
    for (_, entity, mut mover, position) in movers {
        mover.grounded = cast_shape(
            world,
            &ShapeCast {
                shape: PhysicsShape::Ball {
                    radius: RADIUS * 0.9,
                },
                origin: position - Vec3::Y * HALF_HEIGHT,
                rotation: Quat::IDENTITY,
                direction: Vec3::NEG_Y,
                max_distance: 0.09,
                exclude: vec![entity],
            },
        )
        .is_some();
        let heading = Quat::from_rotation_y(mover.yaw) * Vec3::X;
        mover.ahead = cast_ray(
            world,
            &RayCast {
                origin: position,
                direction: heading,
                max_distance: 100.0,
                exclude: vec![entity],
            },
        )
        .map_or(100.0, |hit| hit.distance);
        let falling = world
            .get::<PhysicsVelocity>(entity)
            .map_or(0.0, |velocity| velocity.linear.y);
        let vertical = if mover.grounded { 0.0 } else { falling - 0.3 };
        let velocity = heading * mover.move_x * 4.0 + Vec3::Y * vertical;
        world
            .entity_mut(entity)
            .insert((mover, PhysicsVelocitySet(velocity)));
    }
}

fn wall(name: &str, x: f32) -> (Name, PhysicsBody, Transform) {
    (
        Name::new(name.to_string()),
        PhysicsBody {
            kind: PhysicsBodyKind::Fixed,
            shape: PhysicsShape::Cuboid {
                half_extents: Vec3::new(0.5, 2.0, 20.0),
            },
            shape_offset: Vec3::ZERO,
        },
        Transform::from_xyz(x, 2.0, 0.0),
    )
}

fn character_app() -> App {
    let mut app = make_app();
    app.add_plugins(InputActionsPlugin)
        .register_net_state::<Mover>()
        .init_resource::<Aim>()
        .register_command("test.drive", "test.drive <move_x> <yaw>", drive_command)
        .register_action(held("left", &[InputKey::A]))
        .register_action(held("right", &[InputKey::D]))
        .register_axis(InputAxis {
            id: "move_x",
            negative_action: "left",
            positive_action: "right",
        })
        .add_systems(
            FixedUpdate,
            (
                send_input.after(InputActionSystems),
                control.in_set(PhysicsControlSystems),
            ),
        );
    app.world_mut().spawn(wall("wall_east", 8.0));
    app
}

fn held(id: &'static str, default_keys: &'static [InputKey]) -> InputAction {
    InputAction {
        id,
        command_line: HELD_ONLY,
        default_keys,
    }
}

fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> LinkConditions {
    LinkConditions {
        latency_ticks,
        jitter_ticks,
        loss,
    }
}

/// A harness with two joined clients and one character per client, spawned by the server.
fn harness(margin_ticks: i64, conditions: LinkConditions) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks,
        state_interval_ticks: 2,
        up: conditions,
        down: conditions,
        seed: 21,
    };
    let mut harness = NetHarness::new(config, &["test.drive"], 2, character_app);
    for _ in 0..600 {
        if harness.all_joined() {
            break;
        }
        harness.run_idle(1);
    }
    assert!(harness.all_joined());
    for owner in 0..2 {
        harness.server_app.world_mut().spawn((
            Name::new(format!("mover_{owner}")),
            PhysicsBody::character(HALF_HEIGHT, RADIUS),
            Transform::from_xyz(0.0, 1.5, 3.0 * owner as f32),
            Mover {
                owner,
                ..Mover::default()
            },
        ));
    }
    harness.run_idle(120);
    harness
}

fn run_on_client(harness: &mut NetHarness, client: usize, line: &str) {
    run_command_line(harness.client_apps[client].world_mut(), line).expect("a client command");
}

fn find(world: &mut World, name: &str) -> Entity {
    world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no `{name}`"))
}

fn mover(world: &mut World, name: &str) -> (Mover, Transform, PhysicsVelocity) {
    let entity = find(world, name);
    (
        world.get::<Mover>(entity).expect("mover").clone(),
        *world.get::<Transform>(entity).expect("transform"),
        *world.get::<PhysicsVelocity>(entity).expect("velocity"),
    )
}

/// Every machine has every character where the server has it.
fn assert_converged(harness: &mut NetHarness) {
    for name in ["mover_0", "mover_1"] {
        let server = mover(harness.server_app.world_mut(), name);
        for index in 0..2 {
            let client = mover(harness.client_apps[index].world_mut(), name);
            assert!(
                client.1.translation.distance(server.1.translation) < 2e-3,
                "client {index} `{name}`: {} vs {}",
                client.1.translation,
                server.1.translation
            );
            assert_eq!(client.0.yaw, server.0.yaw, "client {index} `{name}`");
            assert_eq!(harness.client(index).stats.refused_rollbacks, 0);
        }
    }
}

/// Client 0 walks east into the wall while client 1 walks west, turning with the mouse.
fn play(harness: &mut NetHarness) {
    run_on_client(harness, 0, "input.set_axis move_x 1");
    run_on_client(harness, 1, "input.key_down A");
    harness.run_idle(60);
    run_on_client(harness, 0, "input.look 20 0");
    harness.run_idle(60);
    run_on_client(harness, 0, "input.look -20 0");
    harness.run_idle(100);
    run_on_client(harness, 0, "input.clear_axis move_x");
    run_on_client(harness, 1, "input.key_up A");
    harness.run_idle(200);
}

#[test]
fn analog_input_drives_characters_that_stand_upright_and_stop_at_walls() {
    let mut harness = harness(1, LinkConditions::PERFECT);
    play(&mut harness);
    let (walked, pose, velocity) = mover(harness.server_app.world_mut(), "mover_0");
    // The wall's face is at x 7.5; the capsule stops one radius before it.
    assert!((pose.translation.x - 7.1).abs() < 0.1, "{pose:?}");
    assert!((pose.translation.y - 0.9).abs() < 0.05, "stands: {pose:?}");
    assert_eq!(pose.rotation, Quat::IDENTITY, "rotation is locked");
    assert!(walked.grounded);
    assert!(velocity.linear.length() < 0.05, "{velocity:?}");
    assert_eq!(walked.yaw, 0.0);
    let (_, other, _) = mover(harness.server_app.world_mut(), "mover_1");
    assert!(other.translation.x < -5.0, "{other:?}");
    assert_converged(&mut harness);
}

#[test]
fn with_enough_lead_a_character_is_predicted_without_corrections() {
    let mut harness = harness(10, link(5, 0, 0.0));
    // Client 1 only learns what client 0 does from the server, so judge a game with one player.
    run_on_client(&mut harness, 0, "input.set_axis move_x 1");
    harness.run_idle(40);
    let before = harness.client(0).stats.corrections;
    run_on_client(&mut harness, 0, "input.look 15 0");
    harness.run_idle(200);
    assert_eq!(harness.server().late_inputs, 0);
    assert_eq!(harness.client(0).stats.corrections, before);
}

#[test]
fn lag_jitter_and_loss_are_corrected_and_converge() {
    let mut harness = harness(3, link(6, 3, 0.15));
    play(&mut harness);
    assert!(harness.client(0).stats.corrections > 0);
    assert_converged(&mut harness);
    let settled = harness.client(0).stats.corrections;
    harness.run_idle(100);
    assert_eq!(harness.client(0).stats.corrections, settled);
}

#[test]
fn a_replay_does_not_swallow_pending_mouse_movement() {
    let mut harness = harness(10, link(5, 0, 0.0));
    // A server-only move forces a correction while client 0 has movement pending.
    for step in 0..30 {
        let world = harness.server_app.world_mut();
        let entity = find(world, "mover_1");
        if step % 10 == 0 {
            world
                .entity_mut(entity)
                .insert(PhysicsVelocitySet(Vec3::new(0.0, 3.0, 0.0)));
            world.get_mut::<Mover>(entity).expect("mover").move_x = 0.5;
        }
        run_on_client(&mut harness, 0, "input.look 1 0");
        harness.run_idle(1);
    }
    harness.run_idle(100);
    assert!(harness.client(0).stats.corrections > 0);
    let (server, ..) = mover(harness.server_app.world_mut(), "mover_0");
    assert!(
        (server.yaw - 0.30).abs() < 1e-4,
        "every count of movement arrived once: {}",
        server.yaw
    );
}

fn single_app() -> App {
    let mut app = character_app();
    prepare_simulation_app(&mut app);
    app
}

fn ticks(world: &mut World, count: u32) {
    for _ in 0..count {
        simulate_tick(world);
    }
}

#[test]
fn casts_report_the_closest_live_body_and_skip_excluded_and_removed_ones() {
    let mut app = single_app();
    let world = app.world_mut();
    world.spawn(wall("wall_near", 4.0));
    ticks(world, 3);
    let near = find(world, "wall_near");
    let mut ray = RayCast {
        origin: Vec3::new(0.0, 1.0, 0.0),
        direction: Vec3::new(2.0, 0.0, 0.0),
        max_distance: 50.0,
        exclude: Vec::new(),
    };
    let hit = cast_ray(world, &ray).expect("the near wall");
    assert_eq!((hit.name.as_str(), hit.entity), ("wall_near", near));
    assert!((hit.distance - 3.5).abs() < 1e-4);
    assert!(hit.point.distance(Vec3::new(3.5, 1.0, 0.0)) < 1e-4);
    assert!(hit.normal.distance(Vec3::NEG_X) < 1e-4);
    let swept = ShapeCast {
        shape: PhysicsShape::Capsule {
            half_height: 0.5,
            radius: 0.25,
        },
        origin: ray.origin,
        rotation: Quat::IDENTITY,
        direction: Vec3::X,
        max_distance: 50.0,
        exclude: Vec::new(),
    };
    let hit = cast_shape(world, &swept).expect("the near wall");
    assert!((hit.distance - 3.25).abs() < 1e-3, "{hit:?}");
    assert!(hit.normal.distance(Vec3::NEG_X) < 1e-3, "{hit:?}");
    assert!((hit.point.x - 3.5).abs() < 1e-3, "{hit:?}");
    ray.exclude = vec![near];
    assert_eq!(cast_ray(world, &ray).expect("far").name, "wall_east");
    ray.exclude.clear();
    world.entity_mut(near).insert(RemovePhysicsBody);
    ticks(world, 1);
    assert_eq!(cast_ray(world, &ray).expect("far").name, "wall_east");
    ray.max_distance = 1.0;
    assert_eq!(cast_ray(world, &ray), None);
    ray.direction = Vec3::ZERO;
    assert_eq!(cast_ray(world, &ray), None);
}

#[test]
fn a_restore_puts_pose_velocity_and_casts_back_before_the_next_step() {
    let mut app = single_app();
    let world = app.world_mut();
    world.spawn((
        Name::new("walker"),
        PhysicsBody::character(HALF_HEIGHT, RADIUS),
        Transform::from_xyz(0.0, 1.5, 0.0),
        Mover {
            move_x: 1.0,
            ..Mover::default()
        },
    ));
    ticks(world, 40);
    let snapshot = capture_physics(world).expect("capture");
    let then = mover(world, "walker");
    assert!(then.2.linear.x > 3.0, "{then:?}");
    ticks(world, 30);
    let later = compute_state_hash(world).expect("hash").0;
    assert!(mover(world, "walker").1.translation.x > then.1.translation.x + 1.0);
    restore_physics(world, &snapshot).expect("restore");
    assert_eq!(mover(world, "walker"), then);
    let ray = RayCast {
        origin: Vec3::new(-10.0, then.1.translation.y, 0.0),
        direction: Vec3::X,
        max_distance: 100.0,
        exclude: Vec::new(),
    };
    let hit = cast_ray(world, &ray).expect("the character where it was");
    let expected = then.1.translation.x - RADIUS + 10.0;
    assert!((hit.distance - expected).abs() < 1e-3, "{hit:?}");
    ticks(world, 30);
    assert_eq!(compute_state_hash(world).expect("hash").0, later);
}
