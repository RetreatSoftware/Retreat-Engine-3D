//! The `net.*` commands: drive and check a networked game from a command script or the browser bridge.
//!
//! - `net.send <command line...>`: a client queues a command line as the player's input: it runs on the
//!   client at the start of the next tick and is sent to the server (which runs it only if the command is
//!   allowed). Everything after `net.send` is the command line.
//! - `net.status`: logs the role, tick and counters.
//! - `net.bodies`: logs one line per physics body, `net-body <name> <x> <y> <z>`, so a script that
//!   starts several processes can compare what each of them sees.
//! - `net.states`: logs one line per module state value, `net-state <entity> <id> v<version> <hex>`.
//! - `net.remove <name>` (registered with the physics commands): removes a replicated entity safely.
//! - `assert.net_joined`: fails unless this client has joined the server.
//! - `assert.net_clients <n>`: fails unless this server knows exactly `n` clients.
//! - `net.stats`: logs one greppable line, `net-stats role=<server|client> bytes_sent=<n> ...`, with
//!   the byte, correction, late-input and refusal counters of this process.
//! - `assert.net_bytes_below <sent|received> <n>`: fails unless this process sent (or received)
//!   fewer than `n` bytes so far. Works on a server and on a client.
//! - `assert.net_corrections_at_most <n>`: client only; fails if prediction was corrected more than `n` times.
//! - `assert.net_late_inputs_at_most <n>`: server only; fails if more than `n` inputs arrived after their tick.
//! - `assert.net_input_wait_at_most <ticks>`: client only; fails unless the server's last report says
//!   this client's inputs wait at most that many ticks there (the delay others see; see
//!   `tick_pacing.rs`).
//! - `assert.net_refusals_at_most <n>`: server only; fails if the refusal counters (rate limits, bad
//!   inputs, mutes, kicks, ...; see `ServerStats::refusals`) add up to more than `n`.
//!
//! Counters only grow, so a script puts the assert after the play it measures. On a real network
//! corrections and late inputs depend on timing: assert a generous bound there, an exact `0` only
//! in the in-process harness.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, add_net_contract,
};

use super::body_state::read_body_states;
use super::client::{ClientPhase, NetClient};
use super::module_state::read_module_states;
use super::server::NetServer;

/// Adds the `net.*` commands.
pub struct NetCommandsPlugin;

impl Plugin for NetCommandsPlugin {
    fn build(&self, app: &mut App) {
        add_net_contract(app);
        app.register_command("net.send", "net.send <command line...>", send_command)
            .register_command("net.status", "net.status", status_command)
            .register_command("net.bodies", "net.bodies", bodies_command)
            .register_command("net.states", "net.states", states_command)
            .register_command(
                "assert.net_joined",
                "assert.net_joined",
                assert_joined_command,
            )
            .register_command(
                "assert.net_clients",
                "assert.net_clients <count>",
                assert_clients_command,
            )
            .register_command("net.stats", "net.stats", stats_command)
            .register_command(
                "assert.net_bytes_below",
                "assert.net_bytes_below <sent|received> <bytes>",
                assert_bytes_below_command,
            )
            .register_command(
                "assert.net_corrections_at_most",
                "assert.net_corrections_at_most <count>",
                assert_corrections_command,
            )
            .register_command(
                "assert.net_late_inputs_at_most",
                "assert.net_late_inputs_at_most <count>",
                assert_late_inputs_command,
            )
            .register_command(
                "assert.net_refusals_at_most",
                "assert.net_refusals_at_most <count>",
                assert_refusals_command,
            )
            .register_command(
                "assert.net_input_wait_at_most",
                "assert.net_input_wait_at_most <ticks>",
                assert_input_wait_command,
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
            "net client {}: {:?}, tick {}, round trip {:?} ticks, {:?}",
            client.id, client.phase, client.tick, client.measured_round_trip, client.stats
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

/// `net.states`.
fn states_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    for state in read_module_states(world) {
        let data: String = state
            .data
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        info!(
            "net-state {} {} v{} {data}",
            state.entity, state.id, state.version
        );
    }
    Ok(CommandOutcome::Continue)
}

/// Which way the bytes of `assert.net_bytes_below` went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteDirection {
    Sent,
    Received,
}

impl ByteDirection {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "sent" => Some(Self::Sent),
            "received" => Some(Self::Received),
            _ => None,
        }
    }
}

/// The counters of this process, whichever role it has. `None` marks a counter the role lacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct NetCounters {
    bytes_sent: u64,
    bytes_received: u64,
    corrections: Option<u64>,
    late_inputs: Option<u64>,
    refusals: Option<u64>,
}

impl NetCounters {
    fn bytes(&self, direction: ByteDirection) -> u64 {
        match direction {
            ByteDirection::Sent => self.bytes_sent,
            ByteDirection::Received => self.bytes_received,
        }
    }
}

/// Reads the counters, or `None` when this app is neither a server nor a client.
fn read_counters(world: &World) -> Option<(&'static str, NetCounters)> {
    if let Some(server) = world.get_resource::<NetServer>() {
        return Some((
            "server",
            NetCounters {
                bytes_sent: server.stats.bytes_sent,
                bytes_received: server.stats.bytes_received,
                corrections: None,
                late_inputs: Some(server.late_inputs),
                refusals: Some(server.stats.refusals()),
            },
        ));
    }
    let client = world.get_resource::<NetClient>()?;
    Some((
        "client",
        NetCounters {
            bytes_sent: client.stats.bytes_sent,
            bytes_received: client.stats.bytes_received,
            corrections: Some(client.stats.corrections),
            late_inputs: None,
            refusals: None,
        },
    ))
}

/// The counters, or the failure every stats command gives outside a networked app.
fn counters_or_failure(
    world: &World,
    args: &CommandArgs,
) -> Result<(&'static str, NetCounters), CommandError> {
    read_counters(world).ok_or_else(|| {
        args.failure("this app is neither a network server nor a client (start it with `--server <port>` or `--connect <url>`)")
    })
}

/// Fails unless `actual <= limit`; `what` names the counter, `only_on` the role that has it.
fn check_at_most(
    args: &CommandArgs,
    what: &str,
    actual: Option<u64>,
    limit: u64,
    role: &str,
    only_on: &str,
) -> CommandResult {
    let Some(actual) = actual else {
        return Err(args.failure(format!(
            "{what} only exists on a network {only_on}, this app is a network {role}"
        )));
    };
    if actual <= limit {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("{what} is {actual}, at most {limit} allowed")))
    }
}

/// `net.stats`.
fn stats_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let (role, counters) = counters_or_failure(world, args)?;
    info!(
        "net-stats role={role} bytes_sent={} bytes_received={} corrections={} late_inputs={} refusals={}",
        counters.bytes_sent,
        counters.bytes_received,
        counters.corrections.unwrap_or(0),
        counters.late_inputs.unwrap_or(0),
        counters.refusals.unwrap_or(0),
    );
    // How long this client's inputs wait in the server, and how fast its clock runs to correct it.
    if let Some(client) = world.get_resource::<NetClient>() {
        info!(
            "net-pacing input_wait_ticks={} clock_speed={:.3} catch_up_ticks={} resyncs={}",
            client
                .stats
                .input_wait_ticks
                .map_or_else(|| "none".to_string(), |ticks| ticks.to_string()),
            client.pacing.time_scale,
            client.stats.catch_up_ticks,
            client.stats.resyncs,
        );
    }
    // The server's refusals by cause, so a nonzero total can be traced without a debugger.
    if let Some(server) = world.get_resource::<NetServer>() {
        info!("net-stats-detail {:?}", server.stats);
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.net_bytes_below <sent|received> <bytes>`.
fn assert_bytes_below_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let direction_text: String = args.parse(0, "direction")?;
    let Some(direction) = ByteDirection::parse(&direction_text) else {
        return Err(args.failure(format!(
            "direction `{direction_text}` is not `sent` or `received`"
        )));
    };
    let limit: u64 = args.parse(1, "bytes")?;
    let (_, counters) = counters_or_failure(world, args)?;
    let actual = counters.bytes(direction);
    if actual < limit {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "{direction_text} {actual} bytes, expected fewer than {limit}"
        )))
    }
}

/// `assert.net_corrections_at_most <count>`.
fn assert_corrections_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: u64 = args.parse(0, "count")?;
    let (role, counters) = counters_or_failure(world, args)?;
    check_at_most(
        args,
        "the correction count",
        counters.corrections,
        limit,
        role,
        "client",
    )
}

/// `assert.net_late_inputs_at_most <count>`.
fn assert_late_inputs_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: u64 = args.parse(0, "count")?;
    let (role, counters) = counters_or_failure(world, args)?;
    check_at_most(
        args,
        "the late input count",
        counters.late_inputs,
        limit,
        role,
        "server",
    )
}

/// `assert.net_input_wait_at_most <ticks>`.
fn assert_input_wait_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: i64 = args.parse(0, "ticks")?;
    let Some(client) = world.get_resource::<NetClient>() else {
        return Err(args.failure("this app is not a network client"));
    };
    match client.stats.input_wait_ticks {
        Some(wait) if wait <= limit => Ok(CommandOutcome::Continue),
        Some(wait) => Err(args.failure(format!(
            "inputs wait {wait} ticks in the server, at most {limit} allowed"
        ))),
        None => Err(args.failure("the server has not reported on this client's inputs yet")),
    }
}

/// `assert.net_refusals_at_most <count>`.
fn assert_refusals_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let limit: u64 = args.parse(0, "count")?;
    let (role, counters) = counters_or_failure(world, args)?;
    check_at_most(
        args,
        "the refusal count",
        counters.refusals,
        limit,
        role,
        "server",
    )
}
