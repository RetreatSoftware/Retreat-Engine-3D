//! Delta encoding of server states: the one place that decides which bodies a state message lists
//! and how a client rebuilds the whole state from it.
//!
//! **Rule (decided, FRAMEWORK.md §12 / CLAUDE.md "Networking").** A body whose state is bit-identical to
//! the one the server broadcast last time is *resting*. A resting body is listed for
//! [`LISTED_UNCHANGED_BROADCASTS`] more broadcasts after it stopped changing (so one lost chunk does not
//! hide it) and is then omitted. Every [`FULL_STATE_EVERY_BROADCASTS`]-th broadcast, and the first one
//! after a client joins, is a *full* state that lists every body; a full state is also what removes a
//! body from the client's picture of the world, because an omitted body is "unchanged", never "gone".
//!
//! The client keeps the last listed state of every body ([`StateAssembler`]) and fills omitted bodies in
//! from it, so `process_client_states` still compares and corrects whole, name-sorted lists. A delta
//! that arrives before the client has seen any full state is discarded. The rule is the same for every
//! client, so the server encodes once per broadcast.

use std::collections::BTreeMap;

use super::body_state::BodyState;

/// A body that stopped changing is still listed this many broadcasts, as a guard against loss.
pub const LISTED_UNCHANGED_BROADCASTS: u32 = 3;
/// Every this-many-th broadcast is a full state (bounds how long a lost update or a removal lasts).
pub const FULL_STATE_EVERY_BROADCASTS: u64 = 30;

/// The server half: decides what a broadcast lists.
#[derive(Debug, Default)]
pub struct DeltaEncoder {
    /// Per body name: the state broadcast last and how many broadcasts it has been unchanged.
    previous: BTreeMap<String, (BodyState, u32)>,
    broadcasts: u64,
    force_full: bool,
}

impl DeltaEncoder {
    /// The next broadcast is a full state (a client joined).
    pub fn request_full_state(&mut self) {
        self.force_full = true;
    }

    /// Returns `(full, listed bodies)` for the next broadcast of `bodies` (sorted by name).
    pub fn encode(&mut self, bodies: &[BodyState]) -> (bool, Vec<BodyState>) {
        let full = self.force_full || self.broadcasts.is_multiple_of(FULL_STATE_EVERY_BROADCASTS);
        self.force_full = false;
        self.broadcasts += 1;
        let mut listed = Vec::new();
        let mut next = BTreeMap::new();
        for body in bodies {
            let unchanged = match self.previous.get(&body.name) {
                Some((old, count)) if old == body => count + 1,
                _ => 0,
            };
            if full || unchanged <= LISTED_UNCHANGED_BROADCASTS {
                listed.push(body.clone());
            }
            next.insert(body.name.clone(), (body.clone(), unchanged));
        }
        // Bodies that are gone are forgotten here; a client forgets them at the next full state.
        self.previous = next;
        (full, listed)
    }
}

/// The client half: rebuilds whole states from deltas.
#[derive(Debug, Default)]
pub struct StateAssembler {
    known: BTreeMap<String, BodyState>,
    has_baseline: bool,
    newest_tick: Option<u64>,
}

impl StateAssembler {
    /// Turns a received state into the whole state, or `None` when it must be discarded (older than
    /// one already used, or a delta before any full state).
    pub fn assemble(
        &mut self,
        tick: u64,
        full: bool,
        listed: Vec<BodyState>,
    ) -> Option<Vec<BodyState>> {
        if self.newest_tick.is_some_and(|newest| tick <= newest) {
            return None;
        }
        if full {
            self.known.clear();
            self.has_baseline = true;
        } else if !self.has_baseline {
            return None;
        }
        self.newest_tick = Some(tick);
        for body in listed {
            self.known.insert(body.name.clone(), body);
        }
        // `BTreeMap` iterates in name order, which is the order every state list uses.
        Some(self.known.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use retreat_engine_api::{PhysicsBody, Replicated};

    fn body(name: &str, x: f32) -> BodyState {
        BodyState {
            name: name.to_string(),
            description: PhysicsBody::dynamic_cube(1.0),
            translation: Vec3::new(x, 0.0, 0.0),
            rotation: Quat::IDENTITY,
            linear_velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            sleeping: false,
            synced: Replicated::full().kinds.into_iter().collect(),
        }
    }

    fn names(bodies: &[BodyState]) -> Vec<&str> {
        bodies.iter().map(|body| body.name.as_str()).collect()
    }

    #[test]
    fn a_resting_body_is_listed_a_few_times_then_omitted_until_the_next_full_state() {
        let mut encoder = DeltaEncoder::default();
        let world = [body("a", 0.0), body("b", 1.0)];
        let (full, listed) = encoder.encode(&world);
        assert!(full && listed.len() == 2, "the first broadcast is full");
        for _ in 0..LISTED_UNCHANGED_BROADCASTS {
            let (full, listed) = encoder.encode(&world);
            assert!(!full);
            assert_eq!(
                names(&listed),
                ["a", "b"],
                "still listed while the guard lasts"
            );
        }
        let (full, listed) = encoder.encode(&world);
        assert!(!full && listed.is_empty(), "resting bodies are omitted");
        let moved = [body("a", 0.5), body("b", 1.0)];
        let (_, listed) = encoder.encode(&moved);
        assert_eq!(
            names(&listed),
            ["a"],
            "only the body that changed is listed"
        );
        // Run up to the next full state.
        let mut fulls = 0;
        for _ in 0..FULL_STATE_EVERY_BROADCASTS {
            let (full, listed) = encoder.encode(&moved);
            if full {
                fulls += 1;
                assert_eq!(listed.len(), 2);
            }
        }
        assert_eq!(fulls, 1);
    }

    #[test]
    fn a_requested_full_state_lists_everything_once() {
        let mut encoder = DeltaEncoder::default();
        let world = [body("a", 0.0)];
        for _ in 0..10 {
            encoder.encode(&world);
        }
        encoder.request_full_state();
        let (full, listed) = encoder.encode(&world);
        assert!(full && listed.len() == 1);
        let (full, _) = encoder.encode(&world);
        assert!(!full);
    }

    #[test]
    fn the_assembler_fills_omitted_bodies_and_a_full_state_forgets_removed_ones() {
        let mut assembler = StateAssembler::default();
        assert!(
            assembler.assemble(1, false, vec![body("a", 0.0)]).is_none(),
            "a delta before any full state is discarded"
        );
        let whole = assembler
            .assemble(2, true, vec![body("a", 0.0), body("b", 1.0)])
            .expect("a full state is always usable");
        assert_eq!(names(&whole), ["a", "b"]);
        let whole = assembler.assemble(3, false, vec![body("b", 2.0)]).unwrap();
        assert_eq!(
            names(&whole),
            ["a", "b"],
            "a was omitted: unchanged, not gone"
        );
        assert_eq!(whole[1].translation.x, 2.0);
        assert!(
            assembler.assemble(3, false, vec![]).is_none(),
            "a state that is not newer than the last one is discarded"
        );
        let whole = assembler.assemble(4, true, vec![body("b", 2.0)]).unwrap();
        assert_eq!(
            names(&whole),
            ["b"],
            "a full state removes bodies it does not list"
        );
    }
}
