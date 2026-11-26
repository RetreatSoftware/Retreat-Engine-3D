//! A demo of what a loot module uses for its pickups (it is not the arena's loot): a synced entity
//! without a physics body, placed by its [`NetPose`], dressed on every machine from synced state
//! (late joiners too), turning and bobbing, and hidden while taken.
//!
//! - `sandbox.pickup_spawn <name> <x> <y> <z>`
//! - `sandbox.pickup_set <name> <available|taken>`

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, AppNetStateExt, BoxVisual, CommandArgs, CommandOutcome, CommandResult,
    HideVisuals, NetPose, NetState, RetiredEntity, VisualMotion,
};
use serde::{Deserialize, Serialize};

/// A pickup's synced state.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pickup {
    pub available: bool,
}

impl NetState for Pickup {
    const ID: &'static str = "sandbox.pickup";
}

pub struct PickupPlugin;

impl Plugin for PickupPlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<Pickup>()
            .register_command(
                "sandbox.pickup_spawn",
                "sandbox.pickup_spawn <name> <x> <y> <z>",
                spawn_command,
            )
            .register_command(
                "sandbox.pickup_set",
                "sandbox.pickup_set <name> <available|taken>",
                set_command,
            )
            .add_systems(Update, dress_pickups);
    }
}

/// Looks, on each machine, from the synced state alone.
fn dress_pickups(
    pickups: Query<(Entity, Ref<Pickup>), Without<RetiredEntity>>,
    mut commands: Commands,
) {
    for (entity, pickup) in &pickups {
        if pickup.is_added() {
            commands.entity(entity).insert((
                BoxVisual {
                    size: Vec3::new(0.7, 0.4, 0.5),
                    color: Color::srgb(0.2, 0.8, 0.9),
                    glow: 0.0,
                },
                VisualMotion {
                    turn: 1.5,
                    bob_height: 0.2,
                    bob_seconds: 2.0,
                },
            ));
        }
        if pickup.is_changed() {
            if pickup.available {
                commands.entity(entity).remove::<HideVisuals>();
            } else {
                commands.entity(entity).insert(HideVisuals);
            }
        }
    }
}

fn spawn_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let number = |index, label| args.parse::<f32>(index, label);
    let place = Vec3::new(number(1, "x")?, number(2, "y")?, number(3, "z")?);
    if !place.is_finite() {
        return Err(args.failure("the position is finite"));
    }
    if world
        .query::<&Name>()
        .iter(world)
        .any(|found| found.as_str() == name)
    {
        return Err(args.failure(format!("the name `{name}` is taken")));
    }
    world.spawn((
        Name::new(name),
        Pickup { available: true },
        NetPose::at(place),
    ));
    Ok(CommandOutcome::Continue)
}

fn set_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let word: String = args.parse(1, "available|taken")?;
    let available = match word.as_str() {
        "available" => true,
        "taken" => false,
        other => {
            return Err(args.failure(format!("expected `available` or `taken`, got `{other}`")));
        }
    };
    let found = world
        .query::<(&Name, &mut Pickup)>()
        .iter_mut(world)
        .find(|(found, _)| found.as_str() == name)
        .map(|(_, mut pickup)| pickup.available = available);
    found
        .map(|()| CommandOutcome::Continue)
        .ok_or_else(|| args.failure(format!("there is no pickup `{name}`")))
}
