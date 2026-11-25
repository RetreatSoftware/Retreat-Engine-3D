//! A demo of module state (`NetState`): named counters without a body that are rolled back and
//! replicated like a gameplay module's health or score.
//!
//! - `sandbox.tally_add <name> <amount>`: adds to the counter `<name>`, creating it. Clients may
//!   send it (`net.send`), and predict it.
//! - `assert.tally <name> <total>` / `assert.tally_absent <name>`.
//! - `net.remove <name>` removes a counter safely; `net.states` prints every shared value.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, AppNetExt, AppNetStateExt, CommandArgs, CommandOutcome, CommandResult, NetState,
    RetiredEntity,
};
use serde::{Deserialize, Serialize};

/// One counter.
#[derive(Component, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub total: i64,
}

impl NetState for Tally {
    const ID: &'static str = "sandbox.tally";
}

pub struct TallyPlugin;

impl Plugin for TallyPlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<Tally>()
            .register_command(
                "sandbox.tally_add",
                "sandbox.tally_add <name> <amount>",
                add_command,
            )
            .allow_client_command("sandbox.tally_add")
            .register_command(
                "assert.tally",
                "assert.tally <name> <total>",
                assert_command,
            )
            .register_command(
                "assert.tally_absent",
                "assert.tally_absent <name>",
                assert_absent_command,
            );
    }
}

/// The entity with this name that has not been removed.
fn find_live(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query_filtered::<(Entity, &Name), Without<RetiredEntity>>()
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

fn total_of(world: &mut World, name: &str) -> Option<i64> {
    let entity = find_live(world, name)?;
    world.get::<Tally>(entity).map(|tally| tally.total)
}

fn add_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let amount: i64 = args.parse(1, "amount")?;
    let total = match total_of(world, &name) {
        Some(total) => total,
        // A counter never takes over another object's name (a client sends this command).
        None if world
            .query::<&Name>()
            .iter(world)
            .any(|found| found.as_str() == name) =>
        {
            return Err(args.failure(format!(
                "`{name}` is taken by an entity that is not a counter (or was just removed)"
            )));
        }
        None => 0,
    };
    let Some(total) = total.checked_add(amount) else {
        return Err(args.failure(format!("`{name}` would overflow")));
    };
    let entity =
        find_live(world, &name).unwrap_or_else(|| world.spawn(Name::new(name.clone())).id());
    world.entity_mut(entity).insert(Tally { total });
    Ok(CommandOutcome::Continue)
}

fn assert_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let expected: i64 = args.parse(1, "total")?;
    match total_of(world, &name) {
        Some(total) if total == expected => Ok(CommandOutcome::Continue),
        Some(total) => {
            Err(args.failure(format!("counter `{name}` is {total}, expected {expected}")))
        }
        None => Err(args.failure(format!("there is no counter `{name}`"))),
    }
}

fn assert_absent_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    match total_of(world, &name) {
        None => Ok(CommandOutcome::Continue),
        Some(total) => Err(args.failure(format!("counter `{name}` exists with {total}"))),
    }
}
