//! `state.hash` and `assert.state_hash`: one number that summarizes the simulation state, so a
//! script can check that a run is deterministic (same script, same seed → same hash on every
//! machine and platform).
//!
//! The hash covers every rigid body: its `Name`, then the exact bits of its translation and
//! rotation, in name order; then every module state value (`NetState`) in (entity name, id) order. It uses FNV-1a (64-bit), written out here, because Rust's
//! `DefaultHasher` may change between Rust versions and would break recorded hashes.

use bevy::prelude::*;
use bevy_rapier3d::prelude::RigidBody;
use retreat_engine_api::{AppCommandExt, CommandArgs, CommandOutcome, CommandResult};

use crate::net::module_state::read_module_states;

/// Registers `state.hash` and `assert.state_hash`.
pub struct StateHashPlugin;

impl Plugin for StateHashPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastStateHash>()
            .register_command("state.hash", "state.hash", state_hash_command)
            .register_command(
                "assert.state_hash",
                "assert.state_hash <hash>",
                assert_state_hash_command,
            );
    }
}

/// The value the last `state.hash` computed (`None` before the first), for tests and tools.
#[derive(Resource, Default, Debug)]
pub struct LastStateHash(pub Option<u64>);

/// `state.hash`: logs the current state hash (`state.hash = 0x…`), to copy into an
/// `assert.state_hash` line, and stores it in [`LastStateHash`].
fn state_hash_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let (hash, body_count) = compute_state_hash(world).map_err(|message| args.failure(message))?;
    info!(
        "state.hash = {} ({body_count} rigid bodies)",
        format_state_hash(hash)
    );
    world.resource_mut::<LastStateHash>().0 = Some(hash);
    Ok(CommandOutcome::Continue)
}

/// `assert.state_hash <hash>`: fails unless the current state hash equals `<hash>`
/// (as printed by `state.hash`, e.g. `0x00ff…`).
fn assert_state_hash_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected_text: String = args.parse(0, "hash")?;
    let expected = parse_state_hash(&expected_text).map_err(|message| args.failure(message))?;
    let (actual, body_count) =
        compute_state_hash(world).map_err(|message| args.failure(message))?;
    if actual == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "state hash is {}, expected {} ({body_count} rigid bodies): the simulation \
             differs from the recorded run",
            format_state_hash(actual),
            format_state_hash(expected)
        )))
    }
}

/// Hash of all rigid bodies and how many there are. Fails if a rigid body has no `Name`
/// (names give the hash a stable order and make a mismatch traceable).
pub fn compute_state_hash(world: &mut World) -> Result<(u64, usize), String> {
    let mut bodies: Vec<(String, Transform)> = Vec::new();
    let mut query = world.query_filtered::<
        (Entity, Option<&Name>, &Transform),
        (With<RigidBody>, Without<retreat_engine_api::RetiredEntity>),
    >();
    for (entity, name, transform) in query.iter(world) {
        let Some(name) = name else {
            return Err(format!(
                "rigid body {entity} has no Name; state.hash needs every rigid body named"
            ));
        };
        bodies.push((name.as_str().to_string(), *transform));
    }
    bodies.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut hasher = Fnv1a64::new();
    for (name, transform) in &bodies {
        hasher.write(name.as_bytes());
        // A separator, so names "ab"+"c" and "a"+"bc" cannot hash the same.
        hasher.write(&[0]);
        let translation = transform.translation;
        let rotation = transform.rotation;
        for value in [
            translation.x,
            translation.y,
            translation.z,
            rotation.x,
            rotation.y,
            rotation.z,
            rotation.w,
        ] {
            hasher.write(&value.to_bits().to_le_bytes());
        }
    }
    // Module state, when there is any (a world without it hashes as before).
    for state in read_module_states(world) {
        hasher.write(state.entity.as_bytes());
        hasher.write(&[0]);
        hasher.write(state.id.as_bytes());
        hasher.write(&[0, state.version]);
        hasher.write(&(state.data.len() as u64).to_le_bytes());
        hasher.write(&state.data);
    }
    Ok((hasher.finish(), bodies.len()))
}

/// `0x` and 16 hex digits, the form `assert.state_hash` reads.
pub fn format_state_hash(hash: u64) -> String {
    format!("{hash:#018x}")
}

/// Reads a hash written as `0x` and hex digits.
pub fn parse_state_hash(text: &str) -> Result<u64, String> {
    let Some(digits) = text.strip_prefix("0x") else {
        return Err(format!(
            "<hash> must start with 0x (copy it from `state.hash`), got `{text}`"
        ));
    };
    u64::from_str_radix(digits, 16)
        .map_err(|error| format!("<hash> `{text}` is not a 64-bit hex number: {error}"))
}

/// FNV-1a, 64-bit: a tiny, fixed hash function (the same input gives the same number in every
/// Rust version and on every platform). Not for security.
struct Fnv1a64(u64);

impl Fnv1a64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(bytes: &[u8]) -> u64 {
        let mut hasher = Fnv1a64::new();
        hasher.write(bytes);
        hasher.finish()
    }

    #[test]
    fn fnv1a64_matches_the_published_test_vectors() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn format_and_parse_round_trip() {
        let text = format_state_hash(0xab);
        assert_eq!(text, "0x00000000000000ab");
        assert_eq!(parse_state_hash(&text), Ok(0xab));
    }

    #[test]
    fn parse_rejects_missing_prefix_and_bad_digits() {
        assert!(
            parse_state_hash("ab")
                .unwrap_err()
                .contains("must start with 0x")
        );
        assert!(
            parse_state_hash("0xzz")
                .unwrap_err()
                .contains("not a 64-bit hex number")
        );
    }

    #[test]
    fn hash_depends_on_names_and_exact_positions_not_spawn_order() {
        let mut first = World::new();
        first.spawn((
            Name::new("a"),
            RigidBody::Dynamic,
            Transform::from_xyz(1.0, 2.0, 3.0),
        ));
        first.spawn((Name::new("b"), RigidBody::Fixed, Transform::IDENTITY));
        let mut same_reversed = World::new();
        same_reversed.spawn((Name::new("b"), RigidBody::Fixed, Transform::IDENTITY));
        same_reversed.spawn((
            Name::new("a"),
            RigidBody::Dynamic,
            Transform::from_xyz(1.0, 2.0, 3.0),
        ));
        let mut moved = World::new();
        moved.spawn((
            Name::new("a"),
            RigidBody::Dynamic,
            Transform::from_xyz(1.0, 2.0, 3.0001),
        ));
        moved.spawn((Name::new("b"), RigidBody::Fixed, Transform::IDENTITY));

        let (hash, count) = compute_state_hash(&mut first).unwrap();
        assert_eq!(count, 2);
        assert_eq!(compute_state_hash(&mut same_reversed).unwrap().0, hash);
        assert_ne!(compute_state_hash(&mut moved).unwrap().0, hash);
    }

    #[test]
    fn an_unnamed_rigid_body_is_an_error() {
        let mut world = World::new();
        world.spawn((RigidBody::Dynamic, Transform::IDENTITY));
        let error = compute_state_hash(&mut world).unwrap_err();
        assert!(error.contains("has no Name"), "{error}");
    }
}
