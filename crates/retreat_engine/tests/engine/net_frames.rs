//! What a client shows frame by frame, with frame times that differ from the tick: another player
//! must move evenly on screen while corrections happen all the time.

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::{RenderSmoothingPlugin, capture_physics, restore_physics};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, NetInputQueue,
    NetLocalClient, NetState, PhysicsBody, PhysicsControlSystems, PhysicsVelocitySet,
    RemoteInterpolation, RenderSmoothing,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

const SPEED: f32 = 4.0;
/// Radians the walker turns per tick of input: its heading changes with every input.
const TURN: f32 = 0.02;

/// The way the walker is heading: synced, set by its player's input.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Heading(f32);

impl NetState for Heading {
    const ID: &'static str = "test.heading";
}

/// `test.head <yaw>`.
fn head_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let yaw: f32 = args.parse(0, "yaw")?;
    for (name, mut heading) in world.query::<(&Name, &mut Heading)>().iter_mut(world) {
        if name.as_str() == "turner" {
            heading.0 = yaw;
        }
    }
    Ok(CommandOutcome::Continue)
}

fn walk(walkers: Query<(Entity, &Heading)>, mut commands: Commands) {
    for (entity, heading) in &walkers {
        let way = Quat::from_rotation_y(heading.0) * Vec3::X;
        commands
            .entity(entity)
            .insert(PhysicsVelocitySet(way * SPEED));
    }
}

/// Client 0 turns steadily.
#[derive(Resource, Default)]
struct Turning(f32);

fn steer(
    local: Res<NetLocalClient>,
    mut turning: ResMut<Turning>,
    mut queue: ResMut<NetInputQueue>,
) {
    if local.0 == Some(0) {
        turning.0 += TURN;
        queue.push(format!("test.head {}", turning.0));
    }
}

fn frame_app() -> App {
    let mut app = make_app();
    app.add_plugins(RenderSmoothingPlugin)
        .register_net_state::<Heading>()
        .init_resource::<Turning>()
        .register_command("test.head", "test.head <yaw>", head_command)
        .add_systems(
            FixedUpdate,
            (steer, walk).chain().in_set(PhysicsControlSystems),
        );
    app
}

#[test]
fn a_restore_keeps_the_time_accumulated_towards_the_next_tick() {
    let mut app = frame_app();
    retreat_engine::net::prepare_simulation_app(&mut app);
    retreat_engine::net::simulate_tick(app.world_mut());
    let snapshot = capture_physics(app.world_mut()).expect("physics");
    let part = Duration::from_millis(9);
    app.world_mut()
        .resource_mut::<Time<Fixed>>()
        .accumulate_overstep(part);
    restore_physics(app.world_mut(), &snapshot).expect("restores");
    assert_eq!(app.world().resource::<Time<Fixed>>().overstep(), part);
}

#[test]
fn while_a_player_keeps_turning_both_walkers_move_evenly_on_the_other_screen() {
    // 75, 60 and 144 frames per second against 64 ticks.
    for frame_micros in [13_333, 16_667, 6_944] {
        let conditions = LinkConditions {
            latency_ticks: 3,
            jitter_ticks: 1,
            loss: 0.0,
        };
        let config = HarnessConfig {
            margin_ticks: 6,
            state_interval_ticks: 2,
            up: conditions,
            down: conditions,
            seed: 9,
        };
        let mut harness = NetHarness::new(config, &["test.head"], 2, frame_app);
        // `turner` is client 0's; `own` walks straight and stands for the observer's character.
        for (name, x) in [("turner", 0.0), ("own", -8.0)] {
            harness.server_app.world_mut().spawn((
                Name::new(name),
                PhysicsBody::character(0.5, 0.4),
                Transform::from_xyz(x, 0.9, 0.0),
                Heading::default(),
            ));
        }
        let frame = Duration::from_micros(frame_micros);
        // Join and settle.
        harness.run_frames(frame, 400, |_, _| {});
        let observer = harness.client_apps[1].world_mut();
        let walkers: BTreeMap<String, Entity> = observer
            .query_filtered::<(&Name, Entity), With<Heading>>()
            .iter(observer)
            .map(|(name, entity)| (name.to_string(), entity))
            .collect();
        let (turner, own) = (walkers["turner"], walkers["own"]);
        // As a character module dresses them: the other player from the server's past, the own
        // one smoothed between ticks.
        observer.entity_mut(turner).insert(RemoteInterpolation);
        observer.entity_mut(own).insert(RenderSmoothing::default());
        harness.run_frames(frame, 100, |_, _| {});
        let corrections = harness.client(1).stats.corrections;
        let mut drawn = vec![Vec::new(), Vec::new()];
        harness.run_frames(frame, 300, |harness, _| {
            let world = harness.client_apps[1].world();
            for (index, walker) in [turner, own].into_iter().enumerate() {
                let global = world.get::<GlobalTransform>(walker).expect("drawn");
                drawn[index].push(global.translation());
            }
        });
        let corrected = harness.client(1).stats.corrections - corrections;
        assert!(corrected > 50, "only {corrected} corrections");
        let expected = SPEED * frame.as_secs_f32();
        for (who, drawn) in ["the other player", "the own character"].iter().zip(&drawn) {
            let uneven: Vec<f32> = drawn
                .windows(2)
                .map(|pair| pair[0].distance(pair[1]) / expected)
                .filter(|ratio| !(0.75..=1.25).contains(ratio))
                .collect();
            assert!(
                uneven.is_empty(),
                "{who}, frame {frame_micros} us: {} uneven frames: {:?}",
                uneven.len(),
                &uneven[..uneven.len().min(12)]
            );
        }
    }
}
