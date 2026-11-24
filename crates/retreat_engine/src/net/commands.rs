//! The `net.*` commands: drive and check a networked game from a command script or the browser bridge.
//!
//! - `net.send <command line...>`: a client queues a command line as the player's input: it runs on the
//!   client at the start of the next tick and is sent to the server (which runs it only if the command is
//!   allowed). Everything after `net.send` is the command line.
//! - `net.status`: logs the role, tick and counters.
//! - `net.bodies`: logs one line per physics body, `net-body <name> <x> <y> <z>`, so a script that
//!   starts several processes can compare what each of them sees.
//! - `assert.net_joined`: fails unless this client has joined the server.
//! - `assert.net_clients <n>`: fails unless this server knows exactly `n` clients.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, add_net_contract,
};

use super::body_state::read_body_states;
use super::client::{ClientPhase, NetClient};
use super::server::NetServer;

/// Adds the `net.*` commands.
pub struct NetCommandsPlugin;

impl Plugin for NetCommandsPlugin {
    fn build(&self, app: &mut App) {
        add_net_contract(app);
        app.register_command("net.send", "net.send <command line...>", send_command)
            .register_command("net.status", "net.status", status_command)
            .register_command("net.bodies", "net.bodies", bodies_command)
            .register_command(
                "assert.net_joined",
                "assert.net_joined",
                assert_joined_command,
            )
            .register_command(
                "assert.net_clients",
                "assert.net_clients <count>",
                assert_clients_command,
            );
    }
}

/// `net.send <command line...>`.
fn send_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let line = args.rest(0).join(" ");
    if line.is_empty() {
        return Err(
            args.failure("give the command line to send, e.g. `net.send physics.push a 1 0 0`")
        );
    }
    let Some(mut client) = world.get_resource_mut::<NetClient>() else {
        return Err(args.failure(
            "this app is not a network client; start it with `--connect <url>` (or the page URL `?connect=<url>`)",
        ));
    };
    if client.phase != ClientPhase::Running {
        return Err(args.failure(
            "the client has not joined the server yet; wait with `wait <ticks>` and check `assert.net_joined`",
        ));
    }
    client.queue_input(line);
    Ok(CommandOutcome::Continue)
}

/// `net.status`.
fn status_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    if let Some(server) = world.get_resource::<NetServer>() {
        info!(
            "net server: tick {}, {} clients, {} late inputs, {} refused lines",
            server.tick,
            server.client_count(),
            server.late_inputs,
            server.rejected_lines.len()
        );
    } else if let Some(client) = world.get_resource::<NetClient>() {
        info!(
            "net client {}: {:?}, tick {}, {:?}",
            client.id, client.phase, client.tick, client.stats
        );
    } else {
        info!("net: single player (no --server or --connect)");
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.net_joined`.
fn assert_joined_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    match world.get_resource::<NetClient>() {
        Some(client) if client.phase == ClientPhase::Running => Ok(CommandOutcome::Continue),
        Some(_) => Err(args.failure("the client has not joined the server yet")),
        None => Err(args.failure("this app is not a network client")),
    }
}

/// `assert.net_clients <count>`.
fn assert_clients_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected: usize = args.parse(0, "count")?;
    let Some(server) = world.get_resource::<NetServer>() else {
        return Err(
            args.failure("this app is not a network server (start it with `--server <port>`)")
        );
    };
    if server.client_count() == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "the server knows {} clients, expected {expected}",
            server.client_count()
        )))
    }
}

/// `net.bodies`.
fn bodies_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    for body in read_body_states(world) {
        info!(
            "net-body {} {:.4} {:.4} {:.4}",
            body.name, body.translation.x, body.translation.y, body.translation.z
        );
    }
    Ok(CommandOutcome::Continue)
}
