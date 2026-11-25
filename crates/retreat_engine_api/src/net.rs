//! What gameplay modules see of networking: who sent a command, who joined or left, which commands a
//! client may send, and where a client's inputs go. The engine does the rest (transport, prediction,
//! replication; see `retreat_engine/src/net/`).
//!
//! - [`NetCommandSender`]: while a command runs, which player ran it (`Some(client id)` for a command
//!   a client sent, or one the local client predicts for itself; `None` for single player and scripts).
//!   A command that acts "on the player who sent it" reads it.
//! - [`NetClientJoined`] / [`NetClientLeft`]: messages the **server** writes when a client joins or its
//!   connection ends; a module reacts (spawn a player body, remove it).
//! - [`NetAllowedCommands`]: the command ids a server runs when a client sends them (anything else is
//!   refused). A module adds its own with [`AppNetExt::allow_client_command`].
//! - [`NetLocalClient`] / [`NetReplaying`]: a client's own id, and whether it is replaying.
//! - [`NetInputQueue`]: a **client** module pushes the command lines for the player's input here; the
//!   engine runs them on the client and sends them to the server, once per tick. In single player nothing
//!   drains it.
//! - [`NetIdentityCheck`] / [`NetIdentity`]: the server's hook that decides whether a joining client is
//!   let in and which stable player id it gets. The default accepts everyone; a platform's trusted
//!   backend plugs in its own check. [`NetPlayerIds`] lists the id of every joined client.

use std::collections::BTreeSet;

use bevy::prelude::*;

/// Who ran the command that is running now (see the module docs).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetCommandSender(pub Option<u32>);

/// A client finished the handshake (written by the server).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetClientJoined {
    pub client_id: u32,
}

/// A client's connection ended (written by the server).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetClientLeft {
    pub client_id: u32,
}

/// On a client, its own client id once it has joined (written by the engine); `None` on a server,
/// without a network and before joining. A module uses it to find what is "mine".
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetLocalClient(pub Option<u32>);

/// True while a client replays past ticks after a correction (written by the engine). Systems with
/// one-shot effects outside the simulation (sounds, particles) skip them while it is true.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetReplaying(pub bool);

/// Command ids a server runs for clients.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct NetAllowedCommands(pub BTreeSet<String>);

/// What a [`NetIdentityCheck`] is asked when a client says hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetIdentityRequest<'a> {
    /// The server's id for this connection (never chosen by the client).
    pub connection_id: u32,
    /// The token the client put in its hello (whatever the platform handed it), or `None`. The server
    /// does not interpret it: only the check does. A token is a claim, never proof, until the check
    /// has verified it.
    pub token: Option<&'a str>,
}

/// The answer of a [`NetIdentityCheck`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetIdentityDecision {
    /// Let the client in as this stable player id (the same person gets the same id on every
    /// connection; the server never takes an id from the client's own claims).
    Allow { player_id: String },
    /// Refuse: the server disconnects the client. The reason is logged and counted, never sent.
    Deny { reason: String },
}

/// Decides who may join a server. Library-neutral: a platform adapter implements it (for example by
/// asking a trusted backend to verify the token) and inserts it with [`NetIdentity::new`]. It runs once
/// per connection, when the first hello arrives, on the simulation thread, so it must not block.
pub trait NetIdentityCheck: Send + Sync {
    fn check(&self, request: NetIdentityRequest) -> NetIdentityDecision;
}

/// The default check: everyone is allowed, as the player id `guest-<connection id>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct AcceptEveryone;

impl NetIdentityCheck for AcceptEveryone {
    fn check(&self, request: NetIdentityRequest) -> NetIdentityDecision {
        NetIdentityDecision::Allow {
            player_id: format!("guest-{}", request.connection_id),
        }
    }
}

/// The server's identity check (a resource; defaults to [`AcceptEveryone`]).
#[derive(Resource)]
pub struct NetIdentity(pub Box<dyn NetIdentityCheck>);

impl NetIdentity {
    pub fn new(check: impl NetIdentityCheck + 'static) -> Self {
        Self(Box::new(check))
    }
}

impl Default for NetIdentity {
    fn default() -> Self {
        Self::new(AcceptEveryone)
    }
}

/// The player id of every joined client, by client id (written by the server just before it writes
/// [`NetClientJoined`], and removed after [`NetClientLeft`]). Empty on a client.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct NetPlayerIds(pub std::collections::BTreeMap<u32, String>);

/// Command lines a client module wants to send as the player's input this tick. The engine sets
/// `accepting` when the app is a network client; until then [`NetInputQueue::push`] drops lines, so a
/// single-player game does not collect lines nobody will run.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct NetInputQueue {
    pub lines: Vec<String>,
    pub accepting: bool,
}

impl NetInputQueue {
    /// Queues `line` if this app is a network client.
    pub fn push(&mut self, line: impl Into<String>) {
        if self.accepting {
            self.lines.push(line.into());
        }
    }
}

/// One piece of a physics body's state that the server can replicate. A closed, library-neutral list:
/// the engine maps each kind onto the physics library in one place (`net/body_state.rs`).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum SyncedKind {
    /// Position and rotation.
    Pose,
    /// Linear and angular velocity.
    Velocity,
    /// Whether the body is asleep.
    Sleeping,
}

/// Declares which parts of a **dynamic** [`crate::PhysicsBody`] the server replicates to clients (fixed
/// bodies never move, so every machine builds them from its own scene and they are never sent).
///
/// Every `PhysicsBody` gets `Replicated::full()` unless the entity sets another value (the contract
/// `#[require]`s it), so a body replicates all its state by default. Set a smaller list to save
/// bandwidth, or `Replicated::local_only()` for a body that exists only on this machine (cosmetic
/// debris): it is neither sent nor corrected, and a client never removes it. A client rewinds and
/// corrects only the listed kinds; the others keep the client's own prediction. Every body of one
/// kind of object must declare the same list on the server and on every client.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct Replicated {
    pub kinds: BTreeSet<SyncedKind>,
}

impl Replicated {
    /// Pose, velocity and sleeping: everything (the default).
    pub fn full() -> Self {
        Self::of(&[SyncedKind::Pose, SyncedKind::Velocity, SyncedKind::Sleeping])
    }

    /// Exactly the listed kinds.
    pub fn of(kinds: &[SyncedKind]) -> Self {
        Self {
            kinds: kinds.iter().copied().collect(),
        }
    }

    /// Nothing is replicated: the body is local to this machine.
    pub fn local_only() -> Self {
        Self::of(&[])
    }

    /// True when at least one kind is replicated.
    pub fn is_replicated(&self) -> bool {
        !self.kinds.is_empty()
    }

    /// True when `kind` is replicated.
    pub fn syncs(&self, kind: SyncedKind) -> bool {
        self.kinds.contains(&kind)
    }
}

impl Default for Replicated {
    fn default() -> Self {
        Self::full()
    }
}

/// Adds `allow_client_command` to Bevy's `App`.
pub trait AppNetExt {
    /// Lets clients send the command `id` to a server of this app. Use the command's id only (no
    /// arguments). Creates [`NetAllowedCommands`] if needed.
    fn allow_client_command(&mut self, id: &str) -> &mut Self;
}

impl AppNetExt for App {
    fn allow_client_command(&mut self, id: &str) -> &mut Self {
        self.init_resource::<NetAllowedCommands>();
        self.world_mut()
            .resource_mut::<NetAllowedCommands>()
            .0
            .insert(id.to_string());
        self
    }
}

/// Registers the networking resources and messages a module reads or writes. The engine calls it; a
/// module's tests call it to run without the engine.
pub fn add_net_contract(app: &mut App) {
    app.init_resource::<NetCommandSender>()
        .init_resource::<NetAllowedCommands>()
        .init_resource::<NetInputQueue>()
        .init_resource::<NetLocalClient>()
        .init_resource::<NetReplaying>()
        .init_resource::<NetIdentity>()
        .init_resource::<NetPlayerIds>()
        .init_resource::<crate::NetStateRegistry>()
        .add_message::<NetClientJoined>()
        .add_message::<NetClientLeft>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_commands_collect_in_a_sorted_set() {
        let mut app = App::new();
        app.allow_client_command("b.second")
            .allow_client_command("a.first");
        app.allow_client_command("a.first");
        let allowed = app.world().resource::<NetAllowedCommands>();
        assert_eq!(
            allowed.0.iter().collect::<Vec<_>>(),
            ["a.first", "b.second"]
        );
    }

    #[test]
    fn the_contract_registers_its_resources_and_messages() {
        let mut app = App::new();
        add_net_contract(&mut app);
        assert_eq!(
            *app.world().resource::<NetCommandSender>(),
            NetCommandSender(None)
        );
        assert!(app.world().resource::<NetInputQueue>().lines.is_empty());
        app.world_mut()
            .write_message(NetClientJoined { client_id: 3 });
    }

    #[test]
    fn replicated_defaults_to_everything_and_can_be_narrowed_or_local() {
        let full = Replicated::default();
        assert!(full.syncs(SyncedKind::Pose) && full.syncs(SyncedKind::Velocity));
        assert!(full.syncs(SyncedKind::Sleeping) && full.is_replicated());
        let pose = Replicated::of(&[SyncedKind::Pose, SyncedKind::Pose]);
        assert_eq!(pose.kinds.len(), 1);
        assert!(!pose.syncs(SyncedKind::Velocity));
        assert!(!Replicated::local_only().is_replicated());
    }

    #[test]
    fn the_default_identity_check_accepts_everyone_as_a_guest() {
        let identity = NetIdentity::default();
        let request = NetIdentityRequest {
            connection_id: 4,
            token: Some("anything"),
        };
        assert_eq!(
            identity.0.check(request),
            NetIdentityDecision::Allow {
                player_id: "guest-4".to_string()
            }
        );
    }

    #[test]
    fn the_input_queue_ignores_lines_until_the_app_is_a_client() {
        let mut queue = NetInputQueue::default();
        queue.push("player.move 1 0");
        assert!(queue.lines.is_empty());
        queue.accepting = true;
        queue.push("player.move 1 0");
        assert_eq!(queue.lines, ["player.move 1 0"]);
    }
}
