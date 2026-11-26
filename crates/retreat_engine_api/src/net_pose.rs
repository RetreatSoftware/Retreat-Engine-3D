//! The synced pose of an entity **without a physics body** (a projectile, a pickup).
//!
//! A module moves such an entity by writing [`NetPose`] in `FixedUpdate` (or a command). The engine
//! registers it as net state (rolled back, replicated, predicted), copies it into `Transform` after
//! every tick and after every correction, and records it in the pose history, so
//! [`crate::RemoteInterpolation`] draws it at the same past time as the other players. An entity with
//! a `PhysicsBody` does not use it (bodies replicate their own pose).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::NetState;

/// Largest difference between a predicted and a server pose that needs no correction.
const POSE_TOLERANCE: f32 = 1.0e-3;

/// Where a bodyless synced entity is (see the module docs).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[require(Transform, Visibility)]
pub struct NetPose {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl NetPose {
    /// At `translation`, not rotated.
    pub fn at(translation: Vec3) -> Self {
        Self {
            translation,
            rotation: Quat::IDENTITY,
        }
    }
}

impl NetState for NetPose {
    const ID: &'static str = "engine.pose";

    fn matches(&self, server: &Self) -> bool {
        self.translation.distance(server.translation) <= POSE_TOLERANCE
            && self.rotation.angle_between(server.rotation) <= POSE_TOLERANCE
    }
}
