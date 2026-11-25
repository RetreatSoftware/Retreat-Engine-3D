//! Delta encoding of server states: the one place that decides which bodies a state message lists
//! and how a client rebuilds the whole state from it.
//!
//! **Rule (decided, FRAMEWORK.md §12 / CLAUDE.md "Networking").** A body or module state value that is
//! bit-identical to the one the server broadcast last time is *resting*. A resting one is listed for
//! [`LISTED_UNCHANGED_BROADCASTS`] more broadcasts after it stopped changing (so one lost chunk does not
//! hide it) and is then omitted. Every [`FULL_STATE_EVERY_BROADCASTS`]-th broadcast, the first one after
//! a client joins, and every broadcast in which something disappeared is a *full* state that lists
//! everything; a full state is what removes a body or value from the client's picture of the world,
//! because an omitted one is "unchanged", never "gone".
//!
//! [`split_into_chunks`] cuts a state into datagram-sized messages. The client keeps the last listed
//! value of everything ([`StateAssembler`]) and fills omitted ones in from it, so
//! `process_client_states` still compares and corrects whole, sorted lists. A delta that arrives before
//! the client has seen any full state is discarded. The rule is the same for every client, so the
//! server encodes once per broadcast.

use std::collections::BTreeMap;

use super::MAX_BODIES_PER_STATE_MESSAGE;
use super::body_state::BodyState;
use super::module_state::{ModuleState, encoded_size};

/// A body that stopped changing is still listed this many broadcasts, as a guard against loss.
pub const LISTED_UNCHANGED_BROADCASTS: u32 = 3;
/// Every this-many-th broadcast is a full state (bounds how long a lost update or a removal lasts).
pub const FULL_STATE_EVERY_BROADCASTS: u64 = 30;

/// Byte budget for the bodies and module states of one state message (a datagram holds about 1200;
/// the rest is the message header). A chunk of bodies alone is bounded by their count instead.
pub const MAX_STATE_PAYLOAD_BYTES: usize = 1100;

/// Everything a state message replicates: bodies sorted by name, module states sorted by
/// (entity name, id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReplicatedState {
    pub bodies: Vec<BodyState>,
    pub states: Vec<ModuleState>,
}

/// Cuts a state into message-sized pieces, in order: bodies in groups of
/// [`MAX_BODIES_PER_STATE_MESSAGE`], then module states filling the last piece and further ones up to
/// [`MAX_STATE_PAYLOAD_BYTES`]. There is always at least one piece, so a client hears every tick.
pub fn split_into_chunks(state: ReplicatedState) -> Vec<ReplicatedState> {
    let mut chunks: Vec<ReplicatedState> = state
        .bodies
        .chunks(MAX_BODIES_PER_STATE_MESSAGE)
        .map(|bodies| ReplicatedState {
            bodies: bodies.to_vec(),
            states: Vec::new(),
        })
        .collect();
    let mut last = chunks.pop().unwrap_or_default();
    let mut used: usize = last.bodies.iter().map(encoded_size).sum();
    for item in state.states {
        let size = encoded_size(&item);
        let empty = last.bodies.is_empty() && last.states.is_empty();
        if !empty && used + size > MAX_STATE_PAYLOAD_BYTES {
            chunks.push(std::mem::take(&mut last));
            used = 0;
        }
        used += size;
        last.states.push(item);
    }
    chunks.push(last);
    chunks
}

/// Per key: the value broadcast last and how many broadcasts it has been unchanged.
type Previous<K, V> = BTreeMap<K, (V, u32)>;

/// Lists the values that are not resting (all of them when `full`) and updates `previous`. The
/// second result is true when a key broadcast before is gone.
fn encode_list<K: Ord, V: Clone + PartialEq>(
    previous: &mut Previous<K, V>,
    values: &[V],
    key: impl Fn(&V) -> K,
    full: bool,
) -> (Vec<V>, bool) {
    let mut listed = Vec::new();
    let mut next = BTreeMap::new();
    let mut kept = 0;
    for value in values {
        let id = key(value);
        let old = previous.get(&id);
        kept += usize::from(old.is_some());
        let unchanged = match old {
            Some((old, count)) if old == value => count + 1,
            _ => 0,
        };
        if full || unchanged <= LISTED_UNCHANGED_BROADCASTS {
            listed.push(value.clone());
        }
        next.insert(id, (value.clone(), unchanged));
    }
    let removed = kept < previous.len();
    // Values that are gone are forgotten here; a client forgets them at the next full state.
    *previous = next;
    (listed, removed)
}

fn state_key(state: &ModuleState) -> (String, String) {
    (state.entity.clone(), state.id.clone())
}

/// The server half: decides what a broadcast lists.
#[derive(Debug, Default)]
pub struct DeltaEncoder {
    bodies: Previous<String, BodyState>,
    states: Previous<(String, String), ModuleState>,
    broadcasts: u64,
    force_full: bool,
}

impl DeltaEncoder {
    /// The next broadcast is a full state (a client joined).
    pub fn request_full_state(&mut self) {
        self.force_full = true;
    }

    /// Returns `(full, listed)` for the next broadcast of `state`. A broadcast in which something
    /// disappeared is full, so a removal reaches clients at once.
    pub fn encode(&mut self, state: &ReplicatedState) -> (bool, ReplicatedState) {
        let full = self.force_full || self.broadcasts.is_multiple_of(FULL_STATE_EVERY_BROADCASTS);
        self.force_full = false;
        self.broadcasts += 1;
        let (bodies, body_removed) = encode_list(
            &mut self.bodies,
            &state.bodies,
            |body| body.name.clone(),
            full,
        );
        let (states, state_removed) = encode_list(&mut self.states, &state.states, state_key, full);
        if body_removed || state_removed {
            return (true, state.clone());
        }
        (full, ReplicatedState { bodies, states })
    }
}

/// The client half: rebuilds whole states from deltas.
#[derive(Debug, Default)]
pub struct StateAssembler {
    bodies: BTreeMap<String, BodyState>,
    states: BTreeMap<(String, String), ModuleState>,
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
        listed: ReplicatedState,
    ) -> Option<ReplicatedState> {
        if self.newest_tick.is_some_and(|newest| tick <= newest) {
            return None;
        }
        if full {
            self.bodies.clear();
            self.states.clear();
            self.has_baseline = true;
        } else if !self.has_baseline {
            return None;
        }
        self.newest_tick = Some(tick);
        for body in listed.bodies {
            self.bodies.insert(body.name.clone(), body);
        }
        for state in listed.states {
            self.states.insert(state_key(&state), state);
        }
        // `BTreeMap` iterates in key order, which is the order every state list uses.
        Some(ReplicatedState {
            bodies: self.bodies.values().cloned().collect(),
            states: self.states.values().cloned().collect(),
        })
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

    fn only(bodies: &[BodyState]) -> ReplicatedState {
        ReplicatedState {
            bodies: bodies.to_vec(),
            states: Vec::new(),
        }
    }

    fn item(entity: &str, value: u8, length: usize) -> ModuleState {
        ModuleState {
            entity: entity.to_string(),
            id: "test.value".to_string(),
            version: 1,
            data: vec![value; length],
        }
    }

    fn names(bodies: &[BodyState]) -> Vec<&str> {
        bodies.iter().map(|body| body.name.as_str()).collect()
    }

    #[test]
    fn a_resting_body_is_listed_a_few_times_then_omitted_until_the_next_full_state() {
        let mut encoder = DeltaEncoder::default();
        let world = [body("a", 0.0), body("b", 1.0)];
        let (full, listed) = encoder.encode(&only(&world));
        assert!(
            full && listed.bodies.len() == 2,
            "the first broadcast is full"
        );
        for _ in 0..LISTED_UNCHANGED_BROADCASTS {
            let (full, listed) = encoder.encode(&only(&world));
            assert!(!full);
            assert_eq!(
                names(&listed.bodies),
                ["a", "b"],
                "still listed while the guard lasts"
            );
        }
        let (full, listed) = encoder.encode(&only(&world));
        assert!(
            !full && listed.bodies.is_empty(),
            "resting bodies are omitted"
        );
        let moved = [body("a", 0.5), body("b", 1.0)];
        let (_, listed) = encoder.encode(&only(&moved));
        assert_eq!(
            names(&listed.bodies),
            ["a"],
            "only the body that changed is listed"
        );
        // Run up to the next full state.
        let mut fulls = 0;
        for _ in 0..FULL_STATE_EVERY_BROADCASTS {
            let (full, listed) = encoder.encode(&only(&moved));
            if full {
                fulls += 1;
                assert_eq!(listed.bodies.len(), 2);
            }
        }
        assert_eq!(fulls, 1);
    }

    #[test]
    fn a_requested_full_state_lists_everything_once() {
        let mut encoder = DeltaEncoder::default();
        let world = [body("a", 0.0)];
        for _ in 0..10 {
            encoder.encode(&only(&world));
        }
        encoder.request_full_state();
        let (full, listed) = encoder.encode(&only(&world));
        assert!(full && listed.bodies.len() == 1);
        let (full, _) = encoder.encode(&only(&world));
        assert!(!full);
    }

    #[test]
    fn the_assembler_fills_omitted_bodies_and_a_full_state_forgets_removed_ones() {
        let mut assembler = StateAssembler::default();
        assert!(
            assembler
                .assemble(1, false, only(&[body("a", 0.0)]))
                .is_none(),
            "a delta before any full state is discarded"
        );
        let whole = assembler
            .assemble(2, true, only(&[body("a", 0.0), body("b", 1.0)]))
            .expect("a full state is always usable");
        assert_eq!(names(&whole.bodies), ["a", "b"]);
        let whole = assembler
            .assemble(3, false, only(&[body("b", 2.0)]))
            .unwrap();
        assert_eq!(
            names(&whole.bodies),
            ["a", "b"],
            "a was omitted: unchanged, not gone"
        );
        assert_eq!(whole.bodies[1].translation.x, 2.0);
        assert!(
            assembler.assemble(3, false, only(&[])).is_none(),
            "a state that is not newer than the last one is discarded"
        );
        let whole = assembler
            .assemble(4, true, only(&[body("b", 2.0)]))
            .unwrap();
        assert_eq!(
            names(&whole.bodies),
            ["b"],
            "a full state removes bodies it does not list"
        );
    }

    #[test]
    fn module_states_are_delta_encoded_and_a_removal_makes_a_full_state() {
        let mut encoder = DeltaEncoder::default();
        let mut assembler = StateAssembler::default();
        let mut world = ReplicatedState {
            bodies: Vec::new(),
            states: vec![item("a", 1, 4), item("b", 2, 4)],
        };
        for tick in 1..=6 {
            let (full, listed) = encoder.encode(&world);
            assert_eq!(assembler.assemble(tick, full, listed).unwrap(), world);
        }
        let (full, listed) = encoder.encode(&world);
        assert!(!full && listed.states.is_empty(), "resting values rest");
        world.states[1] = item("b", 3, 4);
        let (full, listed) = encoder.encode(&world);
        assert!(!full && listed.states == [item("b", 3, 4)]);
        world.states.remove(0);
        let (full, listed) = encoder.encode(&world);
        assert!(full, "a removal is sent at once");
        assert_eq!(assembler.assemble(9, full, listed).unwrap(), world);
    }

    #[test]
    fn chunks_keep_order_and_fit_one_datagram() {
        let state = ReplicatedState {
            bodies: (0..9)
                .map(|index| body(&format!("body_{index}"), 0.0))
                .collect(),
            states: (0..40)
                .map(|index| item(&format!("entity_{index:02}"), 7, 100))
                .collect(),
        };
        let chunks = split_into_chunks(state.clone());
        let mut joined = ReplicatedState::default();
        for chunk in chunks.iter().cloned() {
            let bytes: usize = chunk.bodies.iter().map(encoded_size).sum::<usize>()
                + chunk.states.iter().map(encoded_size).sum::<usize>();
            assert!(
                bytes <= MAX_STATE_PAYLOAD_BYTES,
                "{bytes} bytes in one chunk"
            );
            joined.bodies.extend(chunk.bodies);
            joined.states.extend(chunk.states);
        }
        assert_eq!(joined, state);
        assert!(chunks.len() > 2);
        assert_eq!(
            split_into_chunks(ReplicatedState::default()),
            [ReplicatedState::default()]
        );
    }
}
