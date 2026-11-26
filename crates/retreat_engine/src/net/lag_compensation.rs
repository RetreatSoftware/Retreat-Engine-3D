//! Lag compensation (contract `retreat_engine_api/src/lag_compensation.rs`): the history of where
//! moving bodies were, by server tick, and casts against it.
//!
//! - The **server** records every replicated body after each tick, [`MAX_REWIND_TICKS`] deep.
//! - A **client** records the states it receives from the server (the same numbers), so its
//!   prediction of a shot and its drawing of other players use what the server will use. It also
//!   keeps the view tick ([`ViewClock`]): a little behind the newest state, advancing with its own
//!   clock.
//! - Between two recorded ticks a pose is interpolated linearly; outside the history the nearest
//!   record is used. History is not part of rewinds: it only ever holds the server's past.
//!
//! Command: `assert.lag_ray_hit <name> <view_tick> <ox> <oy> <oz> <dx> <dy> <dz> <max>`.

use std::collections::BTreeMap;

use bevy::prelude::*;
use retreat_engine_api::{
    MAX_REWIND_TICKS, NetPose, NetState, NetViewTick, PhysicsBody, RetiredEntity, decode_net_state,
};

use super::body_state::BodyState;
use super::module_state::ModuleState;

/// How far behind the newest server state a client draws other players: room for the broadcast
/// interval and some jitter (about 100 ms).
pub const VIEW_DELAY_TICKS: f64 = 6.0;
/// A client keeps more than the server: its view may lag behind its newest state.
pub const CLIENT_HISTORY_TICKS: u64 = MAX_REWIND_TICKS * 4;
/// Part of the difference to the wanted view tick closed each tick.
const VIEW_CORRECTION: f64 = 0.05;
/// A view this far from where it should be jumps there.
const VIEW_SNAP_TICKS: f64 = 32.0;

/// Where one body was.
#[derive(Clone, Debug, PartialEq)]
pub struct PastBody {
    /// `None` for a bodyless [`NetPose`] entity: it can be drawn in the past but not hit.
    pub description: Option<PhysicsBody>,
    pub translation: Vec3,
    pub rotation: Quat,
}

/// A bodyless entity's name and synced pose.
pub type NamedPose = (String, NetPose);

/// The live [`NetPose`] entities of `world` without a body (what a server records).
pub fn read_net_poses(world: &mut World) -> Vec<NamedPose> {
    world
        .query_filtered::<(&Name, &NetPose), (Without<PhysicsBody>, Without<RetiredEntity>)>()
        .iter(world)
        .map(|(name, pose)| (name.as_str().to_string(), *pose))
        .collect()
}

/// The poses among received module states (what a client records).
pub fn net_poses_in(states: &[ModuleState]) -> Vec<NamedPose> {
    states
        .iter()
        .filter(|state| state.id == NetPose::ID)
        .filter_map(|state| {
            let pose = decode_net_state::<NetPose>(&state.data).ok()?;
            Some((state.entity.clone(), pose))
        })
        .collect()
}

/// Where the moving bodies were, by server tick.
#[derive(Resource, Debug, Default)]
pub struct PoseHistory {
    ticks: BTreeMap<u64, BTreeMap<String, PastBody>>,
}

impl PoseHistory {
    /// Records the state after server tick `tick`, keeping `depth` ticks.
    pub fn record(&mut self, tick: u64, bodies: &[BodyState], poses: &[NamedPose], depth: u64) {
        let mut recorded: BTreeMap<String, PastBody> = bodies
            .iter()
            .map(|body| {
                let past = PastBody {
                    description: Some(body.description.clone()),
                    translation: body.translation,
                    rotation: body.rotation,
                };
                (body.name.clone(), past)
            })
            .collect();
        for (name, pose) in poses {
            recorded.entry(name.clone()).or_insert(PastBody {
                description: None,
                translation: pose.translation,
                rotation: pose.rotation,
            });
        }
        self.ticks.insert(tick, recorded);
        let newest = self.newest().unwrap_or(tick);
        self.ticks.retain(|old, _| newest - old <= depth);
    }

    pub fn newest(&self) -> Option<u64> {
        self.ticks.keys().next_back().copied()
    }

    /// Every body at `view` (clamped to the recorded ticks), or `None` with no history. A body
    /// recorded on only one side of `view` is where that record has it.
    pub fn at(&self, view: f64) -> Option<BTreeMap<String, PastBody>> {
        let (first, last) = (*self.ticks.keys().next()?, self.newest()?);
        let view = view.clamp(first as f64, last as f64);
        let (before, low) = self.ticks.range(..=view.floor() as u64).next_back()?;
        let after = self.ticks.range(view.ceil() as u64..).next();
        let Some((after, high)) = after.filter(|(after, _)| *after > before) else {
            return Some(low.clone());
        };
        let fraction = ((view - *before as f64) / (after - before) as f64) as f32;
        let mut bodies = low.clone();
        for (name, body) in &mut bodies {
            if let Some(next) = high.get(name) {
                body.translation = body.translation.lerp(next.translation, fraction);
                body.rotation = body.rotation.slerp(next.rotation, fraction);
            }
        }
        Some(bodies)
    }
}

/// A client's view tick.
#[derive(Resource, Debug, Default)]
pub struct ViewClock {
    view: Option<f64>,
}

impl ViewClock {
    /// One live tick of the client, with the newest server tick it has a state of.
    pub fn advance(&mut self, newest: u64) -> f64 {
        let wanted = (newest as f64 - VIEW_DELAY_TICKS).max(1.0);
        let next = match self.view {
            Some(view) if (view + 1.0 - wanted).abs() <= VIEW_SNAP_TICKS => {
                let advanced = view + 1.0;
                advanced + (wanted - advanced) * VIEW_CORRECTION
            }
            _ => wanted,
        };
        // Never past what is known, never backwards by correction alone.
        let next = next.min(newest as f64);
        self.view = Some(next);
        next
    }
}

/// Advances a client's view tick (`FixedLast`, live ticks).
pub(crate) fn advance_view(world: &mut World) {
    let Some(newest) = world.resource::<PoseHistory>().newest() else {
        return;
    };
    let view = world.resource_mut::<ViewClock>().advance(newest);
    world.insert_resource(NetViewTick(Some(view)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use retreat_engine_api::Replicated;

    fn body(x: f32) -> Vec<BodyState> {
        vec![BodyState {
            name: "a".to_string(),
            description: PhysicsBody::dynamic_cube(1.0),
            translation: Vec3::new(x, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            sleeping: false,
            synced: Replicated::full().kinds.into_iter().collect(),
        }]
    }

    fn x_at(history: &PoseHistory, view: f64) -> f32 {
        history.at(view).expect("history")["a"].translation.x
    }

    #[test]
    fn poses_are_interpolated_and_clamped_to_what_is_kept() {
        let mut history = PoseHistory::default();
        assert!(history.at(3.0).is_none());
        for tick in (10..=30).step_by(2) {
            history.record(tick, &body(tick as f32), &[], 10);
        }
        assert_eq!(x_at(&history, 25.0), 25.0);
        assert_eq!(x_at(&history, 25.5), 25.5);
        assert_eq!(x_at(&history, 99.0), 30.0);
        assert_eq!(x_at(&history, 1.0), 20.0, "older ticks are gone");
    }

    #[test]
    fn the_view_advances_with_the_clock_and_stays_behind_the_newest_state() {
        let mut clock = ViewClock::default();
        assert_eq!(clock.advance(100), 94.0);
        // States arrive every other tick.
        let mut view = 94.0;
        for step in 1..200u64 {
            let newest = 100 + step / 2 * 2;
            let next = clock.advance(newest);
            assert!(next > view, "moves forward");
            assert!(next <= newest as f64);
            view = next;
        }
        assert!((299.0 - VIEW_DELAY_TICKS - view).abs() < 2.0, "{view}");
        assert_eq!(clock.advance(1000), 994.0, "a long gap jumps");
    }
}
