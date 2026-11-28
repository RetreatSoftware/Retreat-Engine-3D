//! Module state that is rolled back and replicated: health, cooldowns, a match score.
//!
//! A module makes a component part of the shared simulation by implementing [`NetState`] and
//! registering it with [`AppNetStateExt::register_net_state`], on server and clients alike:
//!
//! ```ignore
//! #[derive(Component, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
//! pub struct Health { pub points: u32 }
//! impl NetState for Health { const ID: &'static str = "health.points"; }
//! app.register_net_state::<Health>();
//! ```
//!
//! (The module's `Cargo.toml` needs `serde` with the `derive` feature.)
//!
//! **What the engine then does** (`retreat_engine/src/net/module_state.rs`):
//! - it captures the component with every physics snapshot and restores it when a client rewinds;
//! - the server sends it to every client; a client whose prediction differs rewinds, takes the
//!   server's value and replays. A client creates an entity it lacks (just `Name` and the synced
//!   components; add visuals with an `Added<T>` system) and removes what the server no longer has;
//! - it is part of `state.hash`.
//!
//! **Rules for a module** (breaking one makes clients disagree with the server):
//! - The entity has a unique `Name` from the moment it is spawned; the name is its identity on every
//!   machine. It needs no physics body (match state is just a named entity).
//! - Change synced components only in commands or `FixedUpdate` systems, from other synced state,
//!   physics state, `SimulationRng` and `Time<Fixed>`. Anything else that carries over between ticks
//!   (`Local`, a resource, a message, a component that is not registered) is **not** rewound.
//! - A synced component holds no `Entity` (ids differ between machines): store a name or client id.
//! - Query order follows Bevy's storage, which a rewind can change: sort by `Name` when order matters.
//! - Never despawn a synced entity: insert [`RemoveNetEntity`]. Systems that must ignore removed
//!   entities filter with `Without<RetiredEntity>`.
//! - Add a `PhysicsBody` in the same spawn as the entity, not to an older entity.
//! - Keep each component small: its encoded size (with the entity name) must stay under 1 KB, and a
//!   component that changes every tick costs bandwidth every broadcast.

use std::collections::BTreeMap;
use std::time::Duration;

use bevy::ecs::component::Component;
use bevy::prelude::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A component that is simulation state shared between machines (see the module docs).
pub trait NetState: Component + Clone + PartialEq + Serialize + DeserializeOwned {
    /// Unique, stable id, `module.thing` (for example `health.points`). It names the type on the
    /// wire and orders it.
    const ID: &'static str;
    /// Bump when the fields change: a client refuses a value with another version.
    const VERSION: u8 = 1;
    /// Whether a client's predicted value is close enough to the server's to need no correction.
    /// The default is exact equality; override it for floats derived from physics.
    fn matches(&self, server: &Self) -> bool {
        self == server
    }
}

/// On an entity: asks the engine to remove a replicated entity safely (the same as
/// [`crate::RemovePhysicsBody`], for entities with or without a body). The engine retires it at once
/// ([`RetiredEntity`]) and despawns it after the rollback window.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemoveNetEntity;

/// On a replicated entity, on the server: its module states are sent only to this client (a
/// player's bag, chest or balance: private, and no bandwidth for the others). The server keeps and
/// decides them as usual; other clients never create the entity. Not replicated itself; when a
/// player's connection changes (a takeover), the module that owns the entity updates the client id.
/// Bodies (physics) are never private.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetPrivateTo {
    pub client_id: u32,
}

/// Set by the engine on a removed entity until it is despawned: it is hidden, not replicated and not
/// hashed, and a rewind can bring it back. Module systems skip it with `Without<RetiredEntity>`.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetiredEntity {
    /// The fixed time at which it was removed.
    pub since: Duration,
}

/// The type-erased operations of one registered [`NetState`], used by the engine.
#[derive(Clone, Copy)]
pub struct NetStateType {
    pub id: &'static str,
    pub version: u8,
    /// The Rust type, to tell a repeated registration from two types sharing an id.
    pub type_name: fn() -> &'static str,
    /// Every entity that has the component.
    pub entities: fn(&mut World) -> Vec<Entity>,
    /// The entity's value, encoded, or `None` when it has none.
    pub read: fn(&World, Entity) -> Option<Vec<u8>>,
    /// Sets the entity's value from encoded bytes (only touches the component when it differs).
    pub write: fn(&mut World, Entity, &[u8]) -> Result<(), String>,
    /// Removes the component if present.
    pub remove: fn(&mut World, Entity),
    /// Whether these bytes are a value of this type.
    pub decodes: fn(&[u8]) -> bool,
    /// Whether two encoded values count as the same ([`NetState::matches`]).
    pub matches: fn(&[u8], &[u8]) -> bool,
}

impl NetStateType {
    fn of<T: NetState>() -> Self {
        Self {
            id: T::ID,
            version: T::VERSION,
            type_name: std::any::type_name::<T>,
            entities: |world| {
                world
                    .query_filtered::<Entity, With<T>>()
                    .iter(world)
                    .collect()
            },
            read: |world, entity| world.get::<T>(entity).map(encode_net_state),
            write: |world, entity, bytes| {
                let value: T = decode_net_state(bytes)?;
                if world.get::<T>(entity) != Some(&value) {
                    world.entity_mut(entity).insert(value);
                }
                Ok(())
            },
            remove: |world, entity| {
                if world.get::<T>(entity).is_some() {
                    world.entity_mut(entity).remove::<T>();
                }
            },
            decodes: |bytes| decode_net_state::<T>(bytes).is_ok(),
            matches: |predicted, server| {
                predicted == server
                    || matches!(
                        (decode_net_state::<T>(predicted), decode_net_state::<T>(server)),
                        (Ok(predicted), Ok(server)) if predicted.matches(&server)
                    )
            },
        }
    }
}

/// The bytes of a value (the binary format of network messages, postcard).
pub fn encode_net_state<T: NetState>(value: &T) -> Vec<u8> {
    postcard::to_allocvec(value).unwrap_or_else(|error| {
        panic!(
            "net state `{}` cannot be encoded: {error}; use plain data fields (numbers, bools, \
             strings, enums, Vec, BTreeMap)",
            T::ID
        )
    })
}

/// A value from its bytes; damaged or left-over bytes are an error.
pub fn decode_net_state<T: NetState>(bytes: &[u8]) -> Result<T, String> {
    match postcard::take_from_bytes::<T>(bytes) {
        Ok((value, [])) => Ok(value),
        Ok((_, rest)) => Err(format!(
            "net state `{}` has {} bytes left over",
            T::ID,
            rest.len()
        )),
        Err(error) => Err(format!("net state `{}` is damaged: {error}", T::ID)),
    }
}

/// Every registered [`NetState`] type, by id (so every machine walks them in the same order).
#[derive(Resource, Default, Clone)]
pub struct NetStateRegistry {
    types: BTreeMap<&'static str, NetStateType>,
}

impl NetStateRegistry {
    /// The registered types, in id order.
    pub fn types(&self) -> impl Iterator<Item = &NetStateType> {
        self.types.values()
    }

    /// The type with this id.
    pub fn get(&self, id: &str) -> Option<&NetStateType> {
        self.types.get(id)
    }

    /// True when no module registered state.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    fn register<T: NetState>(&mut self) {
        let new = NetStateType::of::<T>();
        assert!(
            !T::ID.is_empty() && !T::ID.contains(char::is_whitespace),
            "net state id `{}` of {} must be one word like `module.thing`",
            T::ID,
            (new.type_name)()
        );
        if let Some(old) = self.types.get(T::ID) {
            assert_eq!(
                (old.type_name)(),
                (new.type_name)(),
                "net state id `{}` is used by two types; give each type its own id",
                T::ID
            );
        }
        self.types.insert(T::ID, new);
    }
}

/// Adds `register_net_state` to Bevy's `App`.
pub trait AppNetStateExt {
    /// Makes `T` rolled back and replicated. Registering the same type again does nothing; two
    /// types with one id panic. Call it on the server and on every client.
    fn register_net_state<T: NetState>(&mut self) -> &mut Self;
}

impl AppNetStateExt for App {
    fn register_net_state<T: NetState>(&mut self) -> &mut Self {
        self.init_resource::<NetStateRegistry>();
        self.world_mut()
            .resource_mut::<NetStateRegistry>()
            .register::<T>();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
    struct Aim(f32);

    impl NetState for Aim {
        const ID: &'static str = "test.aim";
        fn matches(&self, server: &Self) -> bool {
            (self.0 - server.0).abs() < 0.1
        }
    }

    #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
    struct Imposter(u8);

    impl NetState for Imposter {
        const ID: &'static str = "test.aim";
    }

    #[test]
    fn the_registered_operations_read_write_compare_and_remove() {
        let mut app = App::new();
        app.register_net_state::<Aim>().register_net_state::<Aim>();
        let kind = *app
            .world()
            .resource::<NetStateRegistry>()
            .get("test.aim")
            .expect("registered");
        let world = app.world_mut();
        let entity = world.spawn(Aim(1.0)).id();
        assert_eq!((kind.entities)(world), [entity]);
        let bytes = (kind.read)(world, entity).expect("has the component");
        assert!((kind.matches)(&bytes, &encode_net_state(&Aim(1.05))));
        assert!(!(kind.matches)(&bytes, &encode_net_state(&Aim(2.0))));
        (kind.write)(world, entity, &encode_net_state(&Aim(2.0))).unwrap();
        assert_eq!(world.get::<Aim>(entity), Some(&Aim(2.0)));
        (kind.remove)(world, entity);
        assert_eq!((kind.read)(world, entity), None);
    }

    #[test]
    fn damaged_or_longer_bytes_are_refused() {
        let mut bytes = encode_net_state(&Aim(1.0));
        assert!(decode_net_state::<Aim>(&bytes[..2]).is_err());
        bytes.push(0);
        assert!(
            decode_net_state::<Aim>(&bytes)
                .unwrap_err()
                .contains("left over")
        );
    }

    #[test]
    #[should_panic(expected = "used by two types")]
    fn two_types_cannot_share_an_id() {
        App::new()
            .register_net_state::<Aim>()
            .register_net_state::<Imposter>();
    }
}
