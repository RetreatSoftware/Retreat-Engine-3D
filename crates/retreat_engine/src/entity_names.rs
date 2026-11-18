//! Commands address entities by their `Name` component. These helpers are the one place
//! that looks names up, so every command applies the same rule: a name is unique.

use bevy::prelude::*;
use retreat_engine_api::{CommandArgs, CommandError};

/// Every entity whose `Name` is exactly `name`.
pub(crate) fn entities_named(world: &mut World, name: &str) -> Vec<Entity> {
    let mut query = world.query::<(Entity, &Name)>();
    query
        .iter(world)
        .filter(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .collect()
}

/// The one entity named `name`. Fails if there is none, or more than one (the command could
/// otherwise act on the wrong entity).
pub(crate) fn find_unique_entity_by_name(
    world: &mut World,
    args: &CommandArgs,
    name: &str,
) -> Result<Entity, CommandError> {
    let matches = entities_named(world, name);
    match matches.as_slice() {
        [entity] => Ok(*entity),
        [] => Err(args.failure(format!("no entity is named `{name}`"))),
        _ => Err(args.failure(format!(
            "{} entities are named `{name}`; give each entity a unique Name",
            matches.len()
        ))),
    }
}

/// Fails if an entity is already named `name`. Commands that create named entities call it
/// first, so names stay unique.
pub(crate) fn ensure_name_is_free(
    world: &mut World,
    args: &CommandArgs,
    name: &str,
) -> Result<(), CommandError> {
    if entities_named(world, name).is_empty() {
        Ok(())
    } else {
        Err(args.failure(format!(
            "an entity is already named `{name}`; choose another name"
        )))
    }
}
