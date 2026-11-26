//! A pickup without a physics body, drawn from synced state alone, looks right on every client,
//! also one that joins after it was spawned, taken and given back.

use bevy::prelude::*;
use retreat_engine::net::harness::{HarnessConfig, NetHarness};
use retreat_engine::net::link::LinkConditions;
use retreat_engine::{NetPosePlugin, VisualsPlugin};
use retreat_engine_api::{
    AppNetStateExt, BoxVisual, HideVisuals, NetPose, NetState, RemoveNetEntity, VisualMotion,
};
use serde::{Deserialize, Serialize};

use crate::net_harness::make_app;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Loot {
    taken: bool,
}

impl NetState for Loot {
    const ID: &'static str = "test.loot";
}

/// As a loot module does it: looks derived from synced state on each machine.
fn dress(loot: Query<(Entity, Ref<Loot>)>, mut commands: Commands) {
    for (entity, loot) in &loot {
        if loot.is_added() {
            commands.entity(entity).insert((
                BoxVisual {
                    size: Vec3::splat(0.5),
                    color: Color::WHITE,
                    glow: 0.0,
                },
                VisualMotion {
                    turn: 1.0,
                    bob_height: 0.1,
                    bob_seconds: 1.0,
                },
            ));
        }
        if loot.is_changed() {
            if loot.taken {
                commands.entity(entity).insert(HideVisuals);
            } else {
                commands.entity(entity).remove::<HideVisuals>();
            }
        }
    }
}

fn app() -> App {
    let mut app = make_app();
    app.init_asset::<StandardMaterial>()
        .add_plugins((NetPosePlugin, VisualsPlugin))
        .register_net_state::<Loot>()
        .add_systems(Update, dress);
    app
}

/// (place, hidden, has a drawn box) of the loot on client `index`, if it has it.
fn seen(harness: &mut NetHarness, index: usize) -> Option<(Vec3, bool, bool)> {
    let world = harness.client_apps[index].world_mut();
    // Visuals are built per frame, which the harness's ticks do not run.
    world.run_schedule(Update);
    world.run_schedule(Update);
    let found = world
        .query_filtered::<(&Transform, Has<HideVisuals>), With<Loot>>()
        .iter(world)
        .next()
        .map(|(transform, hidden)| (transform.translation, hidden))?;
    let boxed = world
        .query::<(&Name, &Mesh3d)>()
        .iter(world)
        .any(|(name, _)| name.as_str() == "crate/box");
    Some((found.0, found.1, boxed))
}

#[test]
fn a_late_joiner_sees_pickups_where_and_as_they_are() {
    let link = LinkConditions {
        latency_ticks: 2,
        jitter_ticks: 1,
        loss: 0.0,
    };
    let config = HarnessConfig {
        margin_ticks: 4,
        state_interval_ticks: 2,
        up: link,
        down: link,
        seed: 3,
    };
    let mut harness = NetHarness::new(config, &[], 2, app);
    // Client 1 hears nothing until later: it joins late.
    let silent = LinkConditions { loss: 1.0, ..link };
    harness.set_links(silent, silent);
    harness.run_idle(1);
    harness.set_links(link, link);
    let place = Vec3::new(3.0, 1.0, -2.0);
    let entity = harness
        .server_app
        .world_mut()
        .spawn((
            Name::new("crate"),
            Loot { taken: false },
            NetPose::at(place),
        ))
        .id();
    harness.run_idle(120);
    assert_eq!(seen(&mut harness, 0), Some((place, false, true)));

    harness
        .server_app
        .world_mut()
        .get_mut::<Loot>(entity)
        .expect("loot")
        .taken = true;
    harness.run_idle(120);
    for client in 0..2 {
        assert_eq!(
            seen(&mut harness, client),
            Some((place, true, true)),
            "client {client}"
        );
    }
    harness
        .server_app
        .world_mut()
        .get_mut::<Loot>(entity)
        .expect("loot")
        .taken = false;
    harness.run_idle(120);
    assert_eq!(seen(&mut harness, 1), Some((place, false, true)));

    harness
        .server_app
        .world_mut()
        .entity_mut(entity)
        .insert(RemoveNetEntity);
    harness.run_idle(200);
    for client in 0..2 {
        let left = seen(&mut harness, client);
        assert!(
            left.is_none_or(|(_, hidden, _)| !hidden),
            "client {client}: {left:?}"
        );
    }
}
