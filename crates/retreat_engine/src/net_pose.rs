//! Copies the synced [`NetPose`] of bodyless entities into `Transform` (contract `net_pose.rs`):
//! after every tick, and by [`sync_net_poses`] after a client wrote the server's values.

use bevy::prelude::*;
use retreat_engine_api::{AppNetStateExt, NetPose, PhysicsBody};

pub struct NetPosePlugin;

impl Plugin for NetPosePlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<NetPose>()
            .add_systems(FixedPostUpdate, sync_net_poses);
    }
}

pub(crate) fn sync_net_poses(world: &mut World) {
    let mut poses = world.query_filtered::<(&NetPose, &mut Transform), Without<PhysicsBody>>();
    for (pose, mut transform) in poses.iter_mut(world) {
        let wanted = Transform {
            translation: pose.translation,
            rotation: pose.rotation,
            scale: transform.scale,
        };
        transform.set_if_neq(wanted);
    }
}
