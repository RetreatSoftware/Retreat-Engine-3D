//! Lag compensation: a client's shot at what it sees hits on the server, because the server casts
//! against where the target was at the client's view tick.

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::lag_compensation::PoseHistory;
use retreat_engine::net::link::LinkConditions;
use retreat_engine::{RenderSmoothingPlugin, VisualsPlugin};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, DrawnPose,
    HideVisuals, MAX_REWIND_TICKS, NetState, NetViewTick, PhysicsBody, PhysicsControlSystems,
    PhysicsVelocitySet, RayCast, RemoteInterpolation, RemovePhysicsBody, cast_ray_at,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

/// Fast enough that the target moves several body widths during the lag.
const TARGET_SPEED: f32 = 8.0;

/// What the shots of this world hit, in order (synced, so client and server can be compared).
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Shots(Vec<String>);

impl NetState for Shots {
    const ID: &'static str = "test.shots";
}

/// Marks the body that runs along +x.
#[derive(Component)]
struct Runner;

/// `test.shoot <x> <view_tick>`: a ray along -z at `x`.
fn shoot_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let x: f32 = args.parse(0, "x")?;
    let view_tick: f64 = args.parse(1, "view_tick")?;
    let cast = RayCast {
        origin: Vec3::new(x, 1.0, 10.0),
        direction: Vec3::NEG_Z,
        max_distance: 20.0,
        exclude: Vec::new(),
    };
    let hit = cast_ray_at(world, &cast, view_tick).map_or("nothing".to_string(), |hit| hit.name);
    for mut shots in world.query::<&mut Shots>().iter_mut(world) {
        shots.0.push(hit.clone());
    }
    Ok(CommandOutcome::Continue)
}

fn run_runners(runners: Query<Entity, With<Runner>>, mut commands: Commands) {
    for entity in &runners {
        commands
            .entity(entity)
            .insert(PhysicsVelocitySet(Vec3::X * TARGET_SPEED));
    }
}

fn lag_app() -> App {
    let mut app = make_app();
    app.add_plugins((RenderSmoothingPlugin, VisualsPlugin))
        .register_net_state::<Shots>()
        .register_command("test.shoot", "test.shoot <x> <view_tick>", shoot_command)
        .add_systems(FixedUpdate, run_runners.in_set(PhysicsControlSystems));
    app
}

/// A running target above the ground and the shot record, both on the server.
fn harness() -> NetHarness {
    let conditions = LinkConditions {
        latency_ticks: 8,
        jitter_ticks: 1,
        loss: 0.0,
    };
    let config = HarnessConfig {
        margin_ticks: 4,
        state_interval_ticks: 2,
        up: conditions,
        down: conditions,
        seed: 17,
    };
    let mut harness = NetHarness::new(config, &["test.shoot"], 1, lag_app);
    for _ in 0..600 {
        if harness.all_joined() {
            break;
        }
        harness.run_idle(1);
    }
    let server = harness.server_app.world_mut();
    server.spawn((Name::new("shots"), Shots::default()));
    server.spawn((
        Name::new("runner"),
        PhysicsBody::character(0.5, 0.4),
        Transform::from_xyz(-20.0, 1.0, 0.0),
        Runner,
    ));
    harness.run_idle(200);
    harness
}

fn shots(world: &mut World) -> Vec<String> {
    world
        .query::<&Shots>()
        .single(world)
        .expect("shots")
        .0
        .clone()
}

/// What the client sees: its view tick and where the target is drawn at it.
fn client_view(harness: &NetHarness) -> (f64, f32) {
    let client = harness.client_apps[0].world();
    let view = client.resource::<NetViewTick>().0.expect("a view tick");
    let past = client.resource::<PoseHistory>().at(view).expect("history");
    (view, past["runner"].translation.x)
}

#[test]
fn a_shot_at_what_the_client_sees_hits_only_with_its_view_tick() {
    let mut harness = harness();
    let (view, seen_x) = client_view(&harness);
    let server_tick = harness.server().tick;
    assert!(view < server_tick as f64, "{view} is in the server's past");
    harness.step(&[(0, vec![format!("test.shoot {seen_x} {view}")])]);
    harness.run_idle(100);
    assert_eq!(shots(harness.server_app.world_mut()), ["runner"]);
    // The client predicted the same result.
    assert_eq!(shots(harness.client_apps[0].world_mut()), ["runner"]);

    let (_, seen_x) = client_view(&harness);
    harness.step(&[(0, vec![format!("test.shoot {seen_x} 0")])]);
    harness.run_idle(100);
    assert_eq!(
        shots(harness.server_app.world_mut()),
        ["runner", "nothing"],
        "without the view tick the target has moved on"
    );
}

#[test]
fn the_server_rewinds_no_further_than_its_history() {
    let mut harness = harness();
    let server = harness.server_app.world();
    let history = server.resource::<PoseHistory>();
    let newest = history.newest().expect("recorded");
    let oldest = (newest - MAX_REWIND_TICKS) as f64;
    let at_oldest = history.at(oldest).expect("kept")["runner"].translation.x;
    assert_eq!(
        history.at(1.0).expect("clamped")["runner"].translation.x,
        at_oldest
    );
    let at_newest = history.at(newest as f64).expect("kept")["runner"]
        .translation
        .x;
    assert!(at_newest - at_oldest > 7.5, "{at_oldest} to {at_newest}");
    harness.run_idle(1);
}

#[test]
fn a_target_removed_after_the_view_tick_can_still_be_hit() {
    let mut harness = harness();
    let (view, seen_x) = client_view(&harness);
    let server = harness.server_app.world_mut();
    let runner = server
        .query::<(Entity, &Name)>()
        .iter(server)
        .find(|(_, name)| name.as_str() == "runner")
        .map(|(entity, _)| entity)
        .expect("runner");
    server.entity_mut(runner).insert(RemovePhysicsBody);
    harness.step(&[(0, vec![format!("test.shoot {seen_x} {view}")])]);
    harness.run_idle(100);
    assert_eq!(shots(harness.server_app.world_mut()), ["runner"]);
}

#[test]
fn a_remote_interpolated_entity_is_drawn_where_the_server_will_look() {
    let mut harness = harness();
    let client = harness.client_apps[0].world_mut();
    let runner = client
        .query::<(Entity, &Name)>()
        .iter(client)
        .find(|(_, name)| name.as_str() == "runner")
        .map(|(entity, _)| entity)
        .expect("runner");
    client.entity_mut(runner).insert(RemoteInterpolation);
    harness.run_idle(2);
    let (_, seen_x) = client_view(&harness);
    let client = harness.client_apps[0].world_mut();
    client.run_schedule(Update);
    client.run_schedule(PostUpdate);
    let drawn = client.get::<DrawnPose>(runner).expect("drawn").translation;
    let global = client
        .get::<GlobalTransform>(runner)
        .expect("propagated")
        .translation();
    assert!((drawn.x - seen_x).abs() < 1e-4, "{drawn} vs {seen_x}");
    assert_eq!(global, drawn);
    let simulated = client
        .get::<Transform>(runner)
        .expect("simulation")
        .translation;
    assert!(
        simulated.x > drawn.x + 1.0,
        "the simulation is ahead: {simulated}"
    );

    client.entity_mut(runner).insert(HideVisuals);
    client.run_schedule(Update);
    assert_eq!(client.get::<Visibility>(runner), Some(&Visibility::Hidden));
    client.entity_mut(runner).remove::<HideVisuals>();
    client.run_schedule(Update);
    assert_eq!(
        client.get::<Visibility>(runner),
        Some(&Visibility::Inherited)
    );
}
