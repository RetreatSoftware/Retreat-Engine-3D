//! Switching an entity's collisions off: a dead player that others walk through.
//!
//! While [`Intangible`] is on an entity with a [`crate::PhysicsBody`], moving bodies pass through
//! it and every cast (present and lag-compensated) ignores it. It still collides with **fixed**
//! bodies, so a body keeps resting on the ground. It is synced state: set it in a command or a
//! `FixedUpdate` system and it is replicated, predicted and rolled back like any module state.
//! Removing it makes the body solid again where it is (bodies that overlap it are pushed apart by
//! the physics). A character body that is intangible only meets fixed bodies in its own stepping.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::NetState;

/// See the module docs.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intangible;

impl NetState for Intangible {
    const ID: &'static str = "engine.intangible";
}
