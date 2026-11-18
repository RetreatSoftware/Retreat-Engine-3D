//! The seeded random-number source for everything a command script can observe.

use bevy::prelude::*;
use retreat_rng::RetreatRng;

/// The one RNG for simulation (spawn positions, loot rolls, AI choices...). It is seeded from
/// the `--seed` launch argument, so a command script gives the same result on every run and
/// on every platform. Do not create other RNGs for anything that changes game state:
/// their numbers would not be reproducible.
#[derive(Resource, Debug)]
pub struct SimulationRng(pub RetreatRng);

impl SimulationRng {
    pub fn from_seed(seed: u64) -> Self {
        Self(RetreatRng::from_seed(seed))
    }
}
