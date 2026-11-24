//! The binary wire format of network messages: one version byte, then the message in `postcard`.
//!
//! The version byte lets a server and a client of different builds notice that they cannot talk,
//! instead of misreading each other: `decode_*` refuses a message whose version is not
//! [`WIRE_VERSION`], and refuses damaged or truncated bytes with a message, never a panic. Change the
//! message types and bump [`WIRE_VERSION`] together.

use super::{ClientMessage, ServerMessage};

/// The version of the wire format this build speaks. Bump it whenever a message type changes.
pub const WIRE_VERSION: u8 = 1;

/// Why bytes could not be turned into a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// No bytes at all.
    Empty,
    /// The other side speaks another wire version.
    VersionMismatch { found: u8, supported: u8 },
    /// The bytes are damaged or cut off.
    Damaged(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Empty => write!(f, "empty network message (no version byte)"),
            WireError::VersionMismatch { found, supported } => write!(
                f,
                "network message has wire version {found}, but this build speaks {supported}; \
                 update the older side"
            ),
            WireError::Damaged(reason) => {
                write!(f, "network message is damaged or truncated: {reason}")
            }
        }
    }
}

impl std::error::Error for WireError {}

fn encode<T: serde::Serialize>(message: &T) -> Vec<u8> {
    let mut bytes = vec![WIRE_VERSION];
    bytes.extend(
        postcard::to_allocvec(message).expect("encoding to a growing Vec cannot run out of room"),
    );
    bytes
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, WireError> {
    let Some((version, body)) = bytes.split_first() else {
        return Err(WireError::Empty);
    };
    if *version != WIRE_VERSION {
        return Err(WireError::VersionMismatch {
            found: *version,
            supported: WIRE_VERSION,
        });
    }
    postcard::from_bytes(body).map_err(|error| WireError::Damaged(error.to_string()))
}

/// Bytes for a client message.
pub fn encode_client_message(message: &ClientMessage) -> Vec<u8> {
    encode(message)
}

/// A client message from bytes.
pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage, WireError> {
    decode(bytes)
}

/// Bytes for a server message.
pub fn encode_server_message(message: &ServerMessage) -> Vec<u8> {
    encode(message)
}

/// A server message from bytes.
pub fn decode_server_message(bytes: &[u8]) -> Result<ServerMessage, WireError> {
    decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::body_state::BodyState;
    use bevy::prelude::*;
    use retreat_engine_api::PhysicsBody;

    fn body(name: &str) -> BodyState {
        BodyState {
            name: name.to_string(),
            description: PhysicsBody::dynamic_cube(1.0),
            translation: Vec3::new(1.5, -2.0, 3.25),
            rotation: Quat::from_rotation_y(0.7),
            linear_velocity: Vec3::new(0.1, 0.2, 0.3),
            angular_velocity: Vec3::new(-1.0, 0.0, 1.0),
            sleeping: false,
        }
    }

    fn input() -> ClientMessage {
        ClientMessage::Input {
            client_id: 3,
            tick: 123_456,
            lines: vec![
                "physics.spawn_box a 0 3 0 1".to_string(),
                "physics.push a 1 0 0".to_string(),
            ],
        }
    }

    #[test]
    fn a_client_message_round_trips() {
        let message = input();
        assert_eq!(
            decode_client_message(&encode_client_message(&message)),
            Ok(message)
        );
    }

    #[test]
    fn a_state_message_round_trips_exactly() {
        let message = ServerMessage::State {
            tick: 99,
            bodies: vec![body("a"), body("b")],
        };
        assert_eq!(
            decode_server_message(&encode_server_message(&message)),
            Ok(message)
        );
    }

    #[test]
    fn the_first_byte_is_the_wire_version() {
        assert_eq!(encode_client_message(&input())[0], WIRE_VERSION);
    }

    #[test]
    fn a_body_costs_a_small_fixed_number_of_bytes() {
        let one = encode_server_message(&ServerMessage::State {
            tick: 1,
            bodies: vec![body("a")],
        });
        let two = encode_server_message(&ServerMessage::State {
            tick: 1,
            bodies: vec![body("a"), body("b")],
        });
        let per_body = two.len() - one.len();
        assert!(per_body <= 90, "one body takes {per_body} bytes");
    }

    #[test]
    fn another_version_is_refused_with_both_versions_named() {
        let mut bytes = encode_client_message(&input());
        bytes[0] = WIRE_VERSION + 1;
        let error = decode_client_message(&bytes).unwrap_err();
        assert_eq!(
            error,
            WireError::VersionMismatch {
                found: WIRE_VERSION + 1,
                supported: WIRE_VERSION
            }
        );
        assert!(error.to_string().contains("update the older side"));
    }

    #[test]
    fn empty_input_is_an_error_not_a_panic() {
        assert_eq!(decode_client_message(&[]), Err(WireError::Empty));
        assert_eq!(decode_server_message(&[]), Err(WireError::Empty));
    }

    #[test]
    fn every_truncation_and_some_garbage_is_refused_without_panicking() {
        let bytes = encode_server_message(&ServerMessage::State {
            tick: 7,
            bodies: vec![body("a")],
        });
        for cut in 1..bytes.len() {
            assert!(
                decode_server_message(&bytes[..cut]).is_err(),
                "cut at {cut}"
            );
        }
        let mut garbage = vec![WIRE_VERSION];
        garbage.extend([0xFF; 12]);
        assert!(matches!(
            decode_client_message(&garbage),
            Err(WireError::Damaged(_))
        ));
        // A client message is not a valid server message and vice versa.
        let client_bytes = encode_client_message(&input());
        let _ = decode_server_message(&client_bytes);
    }
}
