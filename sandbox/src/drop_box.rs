//! `sandbox.drop_box`: drops a box at a random spot above the cube. The toolbar's "Drop box"
//! button runs it. The spot and size come from `SimulationRng`, so a script with the same
//! seed drops the same boxes.

use bevy::prelude::*;
use retreat_engine::{PHYSICS_PROXY_NAME_SUFFIX, spawn_physics_box};
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, SimulationRng,
};

/// Height boxes are dropped from.
const DROP_HEIGHT: f32 = 3.5;
/// Boxes land within this distance of the center, on x and z.
const DROP_SPREAD: f32 = 1.5;
const MIN_BOX_SIZE: f32 = 0.3;
const MAX_BOX_SIZE: f32 = 0.7;

pub struct DropBoxPlugin;

impl Plugin for DropBoxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DroppedBoxCount>().register_command(
            "sandbox.drop_box",
            "sandbox.drop_box",
            drop_box_command,
        );
    }
}

/// How many boxes `sandbox.drop_box` has made; the next one is `box_<count + 1>`.
#[derive(Resource, Default)]
struct DroppedBoxCount(u32);

fn drop_box_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let name = next_free_box_name(world);
    let mut rng = world.resource_mut::<SimulationRng>();
    let x = rng.0.range_f32(-DROP_SPREAD, DROP_SPREAD);
    let z = rng.0.range_f32(-DROP_SPREAD, DROP_SPREAD);
    let size = rng.0.range_f32(MIN_BOX_SIZE, MAX_BOX_SIZE);
    spawn_physics_box(world, &name, Vec3::new(x, DROP_HEIGHT, z), size);
    Ok(CommandOutcome::Continue)
}

/// `box_1`, `box_2`, ...: the first number whose name (and proxy name) nobody uses yet.
fn next_free_box_name(world: &mut World) -> String {
    let mut names = world.query::<&Name>();
    let taken: Vec<String> = names
        .iter(world)
        .map(|name| name.as_str().to_string())
        .collect();
    let mut count = world.resource_mut::<DroppedBoxCount>();
    loop {
        count.0 += 1;
        let candidate = format!("box_{}", count.0);
        let proxy = format!("{candidate}{PHYSICS_PROXY_NAME_SUFFIX}");
        if !taken.contains(&candidate) && !taken.contains(&proxy) {
            return candidate;
        }
    }
}
