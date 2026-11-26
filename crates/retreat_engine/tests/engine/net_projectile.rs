//! Predicted spawns of bodyless entities (a projectile) under late input, loss and refusal; their
//! drawing in the past; one-shot effects; switching collisions off.

use std::collections::BTreeSet;

use bevy::prelude::*;
use retreat_engine::VisualsPlugin;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::lag_compensation::PoseHistory;
use retreat_engine::net::link::LinkConditions;
use retreat_engine::{EffectLog, NetPosePlugin, RenderSmoothingPlugin, VisualEffectsPlugin};
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, DrawnPose,
    EffectTime, Intangible, NetPose, NetState, NetViewTick, PhysicsBody, PhysicsControlSystems,
    PhysicsVelocitySet, RayCast, RemoteInterpolation, RemoveNetEntity, RetiredEntity, SphereVisual,
    VisualEffect, VisualEffects, cast_ray, spawn_predicted,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

/// Units a shot moves per tick.
const SPEED: f32 = 0.25;
/// A shot explodes at this age and is removed [`LINGER`] ticks later.
const FUSE: u32 = 120;
const LINGER: u32 = 10;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Shot {
    age: u32,
}

impl NetState for Shot {
    const ID: &'static str = "test.shot";
}

/// State a server can change behind a client's back, to force corrections.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Noise(u32);

impl NetState for Noise {
    const ID: &'static str = "test.noise";
}

/// Present on the server only: `test.fire_if_allowed` is refused there.
#[derive(Resource)]
struct Refuses;

/// `test.fire <z>`: a shot flying along +x.
fn fire_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let z: f32 = args.parse(0, "z")?;
    spawn_predicted(
        world,
        "shot",
        (Shot { age: 0 }, NetPose::at(Vec3::new(0.0, 2.0, z))),
    );
    Ok(CommandOutcome::Continue)
}

fn fire_if_allowed_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    if world.contains_resource::<Refuses>() {
        return Err(args.failure("refused"));
    }
    fire_command(world, args)
}

fn fly(
    mut shots: Query<(Entity, &Name, &mut Shot, &mut NetPose), Without<RetiredEntity>>,
    mut effects: ResMut<VisualEffects>,
    mut commands: Commands,
) {
    for (entity, name, mut shot, mut pose) in &mut shots {
        shot.age += 1;
        if shot.age < FUSE {
            pose.translation.x += SPEED;
        }
        if shot.age >= FUSE {
            effects.play(VisualEffect {
                id: format!("{name}/boom"),
                position: pose.translation,
                radius: 2.0,
                color: Color::WHITE,
                glow: 1.0,
                ticks: 20,
                time: EffectTime::Present,
            });
        }
        if shot.age >= FUSE + LINGER {
            commands.entity(entity).insert(RemoveNetEntity);
        }
    }
}

fn projectile_app() -> App {
    let mut app = make_app();
    app.add_plugins((
        NetPosePlugin,
        RenderSmoothingPlugin,
        VisualsPlugin,
        VisualEffectsPlugin,
    ))
    .register_net_state::<Shot>()
    .register_net_state::<Noise>()
    .register_command("test.fire", "test.fire <z>", fire_command)
    .register_command(
        "test.fire_if_allowed",
        "test.fire_if_allowed <z>",
        fire_if_allowed_command,
    )
    .add_systems(FixedUpdate, fly.in_set(PhysicsControlSystems));
    app
}

const ALLOWED: &[&str] = &["test.fire", "test.fire_if_allowed"];

fn link(latency_ticks: u32, jitter_ticks: u32, loss: f32) -> LinkConditions {
    LinkConditions {
        latency_ticks,
        jitter_ticks,
        loss,
    }
}

fn harness(margin_ticks: i64, up: LinkConditions, down: LinkConditions) -> NetHarness {
    let config = HarnessConfig {
        margin_ticks,
        state_interval_ticks: 2,
        up,
        down,
        seed: 23,
    };
    let mut harness = NetHarness::new(config, ALLOWED, 2, projectile_app);
    for _ in 0..600 {
        if harness.all_joined() {
            harness
                .server_app
                .world_mut()
                .spawn((Name::new("noise"), Noise(0)));
            harness.run_idle(60);
            return harness;
        }
        harness.run_idle(1);
    }
    panic!("the clients did not join within 600 ticks");
}

/// The live shots of a world: entity, name, age, position.
fn shots(world: &mut World) -> Vec<(Entity, String, u32, Vec3)> {
    world
        .query_filtered::<(Entity, &Name, &Shot, &NetPose), Without<RetiredEntity>>()
        .iter(world)
        .map(|(entity, name, shot, pose)| (entity, name.to_string(), shot.age, pose.translation))
        .collect()
}

fn disturb(harness: &mut NetHarness) {
    let world = harness.server_app.world_mut();
    for mut noise in world.query::<&mut Noise>().iter_mut(world) {
        noise.0 += 1;
    }
}

/// Fires once on client 0 and checks, every tick of the flight, that the client shows exactly one
/// shot, always the same entity, which the server ends up agreeing with.
fn assert_one_shot_from_start_to_end(harness: &mut NetHarness, disturb_every: u64) {
    harness.step(&[(0, vec!["test.fire 0".to_string()])]);
    let first = shots(harness.client_apps[0].world_mut());
    assert_eq!(first.len(), 1, "predicted at once");
    let (entity, name) = (first[0].0, first[0].1.clone());
    let mut server_saw_it = false;
    let mut ages = Vec::new();
    for tick in 1..=90u64 {
        if disturb_every > 0 && tick.is_multiple_of(disturb_every) {
            disturb(harness);
        }
        harness.run_idle(1);
        let now = shots(harness.client_apps[0].world_mut());
        assert_eq!(now.len(), 1, "tick {tick}: {now:?}");
        assert_eq!(now[0].0, entity, "tick {tick}: another entity");
        assert_eq!(now[0].1, name, "tick {tick}");
        ages.push(now[0].2);
        let server = shots(harness.server_app.world_mut());
        assert!(server.len() <= 1, "{server:?}");
        if let Some(shot) = server.first() {
            assert_eq!(shot.1, name, "both machines name it alike");
            server_saw_it = true;
        }
    }
    assert!(server_saw_it);
    // No restart: the age only falls back by the lateness the server imposes, never to the start.
    assert!(
        ages.windows(2).all(|pair| pair[1] + 40 > pair[0]),
        "{ages:?}"
    );
    assert!(*ages.last().expect("ran") > 40, "{ages:?}");
    // It explodes once, is removed everywhere, and the effect never repeats.
    harness.run_idle(200);
    assert!(shots(harness.server_app.world_mut()).is_empty());
    assert!(shots(harness.client_apps[0].world_mut()).is_empty());
    assert!(shots(harness.client_apps[1].world_mut()).is_empty());
    for app in &harness.client_apps {
        assert_eq!(app.world().resource::<EffectLog>().played, 1);
    }
}

#[test]
fn a_predicted_shot_stays_one_entity_on_a_clean_link() {
    let mut harness = harness(4, link(6, 0, 0.0), link(6, 0, 0.0));
    assert_one_shot_from_start_to_end(&mut harness, 0);
}

#[test]
fn rewinds_to_before_the_shot_keep_the_same_entity() {
    let mut harness = harness(4, link(8, 0, 0.0), link(8, 0, 0.0));
    let before = harness.client(0).stats.corrections;
    assert_one_shot_from_start_to_end(&mut harness, 3);
    assert!(harness.client(0).stats.corrections > before + 5);
}

#[test]
fn a_late_input_does_not_remove_or_restart_the_shot() {
    let mut harness = harness(4, link(8, 0, 0.0), link(8, 0, 0.0));
    // The link gets slower just as the player fires: the input arrives after its tick.
    harness.set_links(link(28, 0, 0.0), link(8, 0, 0.0));
    let late = harness.server().late_inputs;
    assert_one_shot_from_start_to_end(&mut harness, 5);
    assert!(harness.server().late_inputs > late, "the inputs were late");
}

#[test]
fn jitter_and_lost_states_do_not_remove_or_duplicate_the_shot() {
    let mut harness = harness(8, link(8, 4, 0.0), link(8, 4, 0.3));
    assert_one_shot_from_start_to_end(&mut harness, 7);
}

#[test]
fn a_shot_the_server_refuses_vanishes_and_stays_gone() {
    let mut harness = harness(4, link(6, 0, 0.0), link(6, 0, 0.0));
    harness.server_app.world_mut().insert_resource(Refuses);
    harness.step(&[(0, vec!["test.fire_if_allowed 0".to_string()])]);
    assert_eq!(shots(harness.client_apps[0].world_mut()).len(), 1);
    harness.run_idle(60);
    assert!(shots(harness.client_apps[0].world_mut()).is_empty());
    harness.run_idle(60);
    assert!(shots(harness.client_apps[0].world_mut()).is_empty());
    assert_eq!(harness.client(0).stats.refused_rollbacks, 0);
}

#[test]
fn a_shot_whose_input_is_lost_vanishes() {
    let clean = link(6, 0, 0.0);
    let mut harness = harness(4, clean, clean);
    harness.set_links(link(6, 0, 1.0), clean);
    harness.step(&[(0, vec!["test.fire 0".to_string()])]);
    harness.set_links(clean, clean);
    assert_eq!(shots(harness.client_apps[0].world_mut()).len(), 1);
    harness.run_idle(60);
    assert!(shots(harness.client_apps[0].world_mut()).is_empty());
    assert!(shots(harness.server_app.world_mut()).is_empty());
    // The next one is unaffected.
    assert_one_shot_from_start_to_end(&mut harness, 0);
}

#[test]
fn two_shots_in_a_row_keep_their_own_entities() {
    let mut harness = harness(-6, link(6, 2, 0.0), link(6, 2, 0.0));
    harness.step(&[(
        0,
        vec!["test.fire 0".to_string(), "test.fire 1".to_string()],
    )]);
    harness.step(&[(0, vec!["test.fire 2".to_string()])]);
    let ids = |harness: &mut NetHarness| -> BTreeSet<Entity> {
        shots(harness.client_apps[0].world_mut())
            .iter()
            .map(|shot| shot.0)
            .collect()
    };
    let first = ids(&mut harness);
    assert_eq!(first.len(), 3);
    for _ in 0..60 {
        disturb(&mut harness);
        harness.run_idle(1);
        assert_eq!(ids(&mut harness), first);
    }
    assert_eq!(shots(harness.server_app.world_mut()).len(), 3);
}

#[test]
fn another_players_shot_is_drawn_in_the_servers_past_from_birth_to_removal() {
    let mut harness = harness(4, link(8, 0, 0.0), link(8, 0, 0.0));
    harness.step(&[(0, vec!["test.fire 0".to_string()])]);
    let mut dressed = None;
    let mut visible_ticks = 0;
    let mut hidden_after_visible = false;
    for _ in 0..260 {
        harness.run_idle(1);
        let world = harness.client_apps[1].world_mut();
        if dressed.is_none() {
            dressed = world
                .query_filtered::<Entity, With<Shot>>()
                .iter(world)
                .next();
            if let Some(entity) = dressed {
                world.entity_mut(entity).insert((
                    RemoteInterpolation,
                    SphereVisual {
                        radius: 0.2,
                        color: Color::WHITE,
                        glow: 1.0,
                    },
                ));
            }
        }
        let Some(entity) = dressed else {
            continue;
        };
        world.run_schedule(Update);
        let view = world.resource::<NetViewTick>().0.expect("a view tick");
        let past = world.resource::<PoseHistory>().at(view).expect("history");
        let name = world.get::<Name>(entity).expect("named").to_string();
        let visibility = *world.get::<Visibility>(entity).expect("visible or not");
        match past.get(&name) {
            Some(body) => {
                assert_eq!(visibility, Visibility::Inherited);
                let drawn = world.get::<DrawnPose>(entity).expect("drawn").translation;
                assert!(drawn.distance(body.translation) < 1e-3, "{drawn}");
                visible_ticks += 1;
            }
            None => {
                assert_eq!(visibility, Visibility::Hidden);
                hidden_after_visible |= visible_ticks > 0;
            }
        }
    }
    assert!(visible_ticks > 100, "{visible_ticks}");
    assert!(
        hidden_after_visible,
        "gone once the view passed its removal"
    );
}

/// `test.walk <name> <speed>`: keeps a character moving along +x.
#[derive(Component)]
struct Walks(f32);

fn walk(walkers: Query<(Entity, &Walks)>, mut commands: Commands) {
    for (entity, walks) in &walkers {
        commands
            .entity(entity)
            .insert(PhysicsVelocitySet(Vec3::X * walks.0));
    }
}

fn walking_app() -> App {
    let mut app = projectile_app();
    app.add_systems(FixedUpdate, walk.in_set(PhysicsControlSystems));
    app
}

#[test]
fn an_intangible_body_is_walked_through_ignored_by_casts_and_keeps_its_ground() {
    let mut app = walking_app();
    retreat_engine::net::prepare_simulation_app(&mut app);
    let world = app.world_mut();
    let down = world
        .spawn((
            Name::new("down"),
            PhysicsBody::character(0.5, 0.4),
            Transform::from_xyz(2.0, 0.9, 0.0),
            Intangible,
        ))
        .id();
    let walker = world
        .spawn((
            Name::new("walker"),
            PhysicsBody::character(0.5, 0.4),
            Transform::from_xyz(0.0, 0.9, 0.0),
            Walks(2.0),
        ))
        .id();
    for _ in 0..128 {
        retreat_engine::net::simulate_tick(app.world_mut());
    }
    let world = app.world_mut();
    let x = world
        .get::<Transform>(walker)
        .expect("walker")
        .translation
        .x;
    assert!(x > 3.5, "walked through: {x}");
    let rest = world.get::<Transform>(down).expect("down").translation;
    assert!(
        (rest.y - 0.9).abs() < 0.05 && (rest.x - 2.0).abs() < 0.05,
        "{rest}"
    );
    let cast = RayCast {
        origin: Vec3::new(2.0, 0.9, 10.0),
        direction: Vec3::NEG_Z,
        max_distance: 20.0,
        exclude: Vec::new(),
    };
    assert!(cast_ray(world, &cast).is_none());
    world.entity_mut(down).remove::<Intangible>();
    world.entity_mut(walker).insert(Walks(-2.0));
    assert_eq!(
        cast_ray(world, &cast).map(|hit| hit.name),
        Some("down".to_string())
    );
    for _ in 0..128 {
        retreat_engine::net::simulate_tick(app.world_mut());
    }
    let pushed = app
        .world()
        .get::<Transform>(down)
        .expect("down")
        .translation
        .x;
    assert!(pushed < 0.0, "solid again, it is pushed along: {pushed}");
}

#[test]
fn a_client_predicts_through_a_body_the_server_made_intangible() {
    let conditions = link(6, 0, 0.0);
    let config = HarnessConfig {
        margin_ticks: 4,
        state_interval_ticks: 2,
        up: conditions,
        down: conditions,
        seed: 3,
    };
    let mut harness = NetHarness::new(config, ALLOWED, 1, walking_app);
    harness.run_idle(60);
    let server = harness.server_app.world_mut();
    let down = server
        .spawn((
            Name::new("down"),
            PhysicsBody::character(0.5, 0.4),
            Transform::from_xyz(3.0, 0.9, 0.0),
        ))
        .id();
    harness.run_idle(60);
    harness
        .server_app
        .world_mut()
        .entity_mut(down)
        .insert(Intangible);
    harness.run_idle(60);
    for app in std::iter::once(&mut harness.client_apps[0]) {
        let world = app.world_mut();
        let found: Vec<String> = world
            .query_filtered::<&Name, With<Intangible>>()
            .iter(world)
            .map(Name::to_string)
            .collect();
        assert_eq!(found, ["down"]);
    }
    harness.server_app.world_mut().spawn((
        Name::new("walker"),
        PhysicsBody::character(0.5, 0.4),
        Transform::from_xyz(0.0, 0.9, 0.0),
        Walks(2.0),
    ));
    harness.run_idle(200);
    let server = harness.server_app.world_mut();
    let x = server
        .query_filtered::<&Transform, With<Walks>>()
        .single(server)
        .expect("walker")
        .translation
        .x;
    assert!(x > 4.5, "walked through on the server: {x}");
}
