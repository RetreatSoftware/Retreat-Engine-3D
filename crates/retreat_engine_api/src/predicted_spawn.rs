//! Entities a client creates before the server does: a projectile made by the player's own fire
//! command. The engine merges the client's entity with the server's one.
//!
//! **Use.** In the command (it runs on the shooter's client at once and on the server when the input
//! arrives) create the entity with [`spawn_predicted`] instead of `world.spawn`:
//!
//! ```ignore
//! spawn_predicted(world, "rocket", (Rocket { .. }, NetPose::at(muzzle)));
//! ```
//!
//! **What the engine guarantees** (`retreat_engine/src/net/client.rs`):
//! - Both machines give it the same name, `<kind>/<owner>/<input tick>/<n>`, from the input that
//!   made it (not from a counter that a lost input would shift), so they are one entity.
//! - The client keeps one `Entity` from the prediction on: no duplicate, no respawn when the server
//!   runs the input late, even though rewinds replay the spawn. The server's values then win (a
//!   late input means a younger projectile on the server: the position is corrected, smoothed by
//!   [`crate::RenderSmoothing`]).
//! - While the server has not yet run that input, states without the entity do not remove it. Once
//!   the server has run inputs [`PREDICTION_REORDER_TICKS`] past it and still has no such entity
//!   (the command was refused, or the input was lost), it is removed.
//!
//! **Rules.** No `PhysicsBody` (bodies are replicated by name already, but are not predicted this
//! way). The bundle holds at least one registered [`crate::NetState`] component. The command must
//! create its entities in the same order on both machines. A replay may re-add the bundle to the
//! same entity, so systems that react to `Added` components must be safe to run again.

use bevy::prelude::*;

use crate::{NetCommandSender, NetStateRegistry, PhysicsBody, RetiredEntity};

/// How far past a predicted spawn's input tick the server must have run inputs before a missing
/// entity counts as refused (room for reordered late inputs).
pub const PREDICTION_REORDER_TICKS: u64 = 8;

/// While a command from a player's input runs: the client tick of that input (the same number on
/// the client and on the server, however late the server runs it). Written by the engine.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetInputTick {
    tick: Option<u64>,
    /// Predicted spawns made by this input so far.
    spawned: u32,
    /// Spawns outside any input (single player, server scripts).
    local_spawns: u64,
}

impl NetInputTick {
    /// The input being run, or `None` outside one.
    pub fn current(&self) -> Option<u64> {
        self.tick
    }

    /// The engine starts (or with `None` ends) running one input's lines.
    pub fn begin(&mut self, tick: Option<u64>) {
        self.tick = tick;
        self.spawned = 0;
    }

    /// The next name for a spawn of `kind` by `owner`.
    fn next_name(&mut self, kind: &str, owner: Option<u32>) -> String {
        let owner = owner.map_or_else(|| "local".to_string(), |id| id.to_string());
        match self.tick {
            Some(tick) => {
                let index = self.spawned;
                self.spawned += 1;
                format!("{kind}/{owner}/{tick}/{index}")
            }
            None => {
                let index = self.local_spawns;
                self.local_spawns += 1;
                format!("{kind}/{owner}/local/{index}")
            }
        }
    }
}

/// On an entity made by [`spawn_predicted`] from a player's input (the engine reads it).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictedSpawn {
    pub input_tick: u64,
    /// The server has listed it: from then on the server decides whether it exists.
    pub confirmed: bool,
}

/// Creates (or, in a replay, re-creates) a predicted entity; see the module docs. `kind` is one
/// word, the start of its name. Returns the entity.
pub fn spawn_predicted<B: Bundle>(world: &mut World, kind: &str, bundle: B) -> Entity {
    let owner = world
        .get_resource::<NetCommandSender>()
        .and_then(|sender| sender.0);
    let mut input = world.get_resource_or_insert_with(NetInputTick::default);
    let name = input.next_name(kind, owner);
    let tick = input.current();
    // A rewind set the same entity aside; the replay brings it back instead of making another.
    let set_aside = world
        .query_filtered::<(Entity, &Name), (With<PredictedSpawn>, With<RetiredEntity>)>()
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity);
    let entity = match set_aside {
        Some(entity) => {
            // Synced components of the first run (an exploded flag added later) must not
            // survive into the replay: only the bundle's values count.
            let registry = world.get_resource::<NetStateRegistry>().cloned();
            for kind in registry.iter().flat_map(NetStateRegistry::types) {
                (kind.remove)(world, entity);
            }
            let mut found = world.entity_mut(entity);
            found.remove::<RetiredEntity>();
            if found.contains::<Visibility>() {
                found.insert(Visibility::Inherited);
            }
            found.insert(bundle);
            entity
        }
        None => world.spawn((Name::new(name.clone()), bundle)).id(),
    };
    assert!(
        world.get::<PhysicsBody>(entity).is_none(),
        "spawn_predicted: `{name}` has a PhysicsBody; predicted spawns are for entities without one"
    );
    if let Some(input_tick) = tick {
        world.entity_mut(entity).insert(PredictedSpawn {
            input_tick,
            confirmed: false,
        });
    }
    entity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Marker;

    #[test]
    fn names_come_from_the_input_and_a_set_aside_entity_is_reused() {
        let mut world = World::new();
        world.insert_resource(NetCommandSender(Some(2)));
        let mut input = NetInputTick::default();
        input.begin(Some(40));
        world.insert_resource(input);
        let first = spawn_predicted(&mut world, "rocket", Marker);
        let second = spawn_predicted(&mut world, "rocket", Marker);
        assert_eq!(world.get::<Name>(first).unwrap().as_str(), "rocket/2/40/0");
        assert_eq!(world.get::<Name>(second).unwrap().as_str(), "rocket/2/40/1");
        world.entity_mut(first).insert(RetiredEntity {
            since: std::time::Duration::ZERO,
        });
        world.resource_mut::<NetInputTick>().begin(Some(40));
        assert_eq!(spawn_predicted(&mut world, "rocket", Marker), first);
        assert!(world.get::<RetiredEntity>(first).is_none());
    }

    #[test]
    fn outside_an_input_every_spawn_is_new_and_not_predicted() {
        let mut world = World::new();
        let first = spawn_predicted(&mut world, "rocket", Marker);
        let second = spawn_predicted(&mut world, "rocket", Marker);
        assert_ne!(first, second);
        assert!(world.get::<PredictedSpawn>(first).is_none());
        assert_eq!(
            world.get::<Name>(second).unwrap().as_str(),
            "rocket/local/local/1"
        );
    }
}
