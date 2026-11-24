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
//! - [`NetInputQueue`]: a **client** module pushes the command lines for the player's input here; the
//!   engine runs them on the client and sends them to the server, once per tick. In single player nothing
//!   drains it.

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

/// Command ids a server runs for clients.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct NetAllowedCommands(pub BTreeSet<String>);

/// Command lines a client module wants to send as the player's input this tick.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct NetInputQueue(pub Vec<String>);

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
        assert!(app.world().resource::<NetInputQueue>().0.is_empty());
        app.world_mut()
            .write_message(NetClientJoined { client_id: 3 });
    }
}
