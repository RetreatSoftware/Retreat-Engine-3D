//! Server records, engine side: creates the app's `RecordStore` where the launch says and adds the
//! `records.*` commands. The file backend lives in the platform layer (native only).
//!
//! **Which backend** (the pure `LaunchOptions::record_location`): files in `--records-dir <dir>`
//! when given; otherwise memory under a command script (a run starts empty and leaves nothing)
//! and in a browser, and files in `records/` (working directory) for interactive native play and
//! servers. A network client never has a store: `install_client` removes it
//! (`remove_record_store_on_client`).
//!
//! Commands: `records.write <owner> <part> <version> <json>` (JSON without spaces),
//! `records.read <owner> <part>`, `records.delete <owner> <part>`, `records.list <owner>`,
//! `records.status`, `assert.record_field <owner> <part> <field> <json>`,
//! `assert.record_absent <owner> <part>`.

use std::path::PathBuf;

use bevy::prelude::*;
use retreat_engine_api::serde_json::{self, Value};
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, RecordBatch,
    RecordKey, RecordStore, StoredRecord,
};

/// Where an app keeps its records (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordLocation {
    Memory,
    Files(PathBuf),
}

/// The folder records go to natively when the launch names none.
pub const DEFAULT_RECORDS_DIRECTORY: &str = "records";

/// Adds the `RecordStore` (at `location`) and the `records.*` commands.
pub struct RecordsPlugin {
    pub location: RecordLocation,
}

impl Plugin for RecordsPlugin {
    fn build(&self, app: &mut App) {
        // A test that inserted its own store keeps it.
        if !app.world().contains_resource::<RecordStore>() {
            let store = match &self.location {
                RecordLocation::Memory => RecordStore::in_memory(),
                RecordLocation::Files(directory) => {
                    match crate::platform::platform_record_backend(directory.clone()) {
                        Ok(backend) => RecordStore::new(backend),
                        // A server that cannot keep its records must not start as if it could.
                        Err(message) => panic!(
                            "cannot open the server records in `{}`: {message}",
                            directory.display()
                        ),
                    }
                }
            };
            app.insert_resource(store);
        }
        app.register_command(
            "records.write",
            "records.write <owner> <part> <version> <json>",
            write_command,
        )
        .register_command("records.read", "records.read <owner> <part>", read_command)
        .register_command(
            "records.delete",
            "records.delete <owner> <part>",
            delete_command,
        )
        .register_command("records.list", "records.list <owner>", list_command)
        .register_command("records.status", "records.status", status_command)
        .register_command(
            "assert.record_field",
            "assert.record_field <owner> <part> <field> <json>",
            assert_field_command,
        )
        .register_command(
            "assert.record_absent",
            "assert.record_absent <owner> <part>",
            assert_absent_command,
        );
    }
}

/// A network client never decides anything that is saved: its store is removed.
pub(crate) fn remove_record_store_on_client(app: &mut App) {
    app.world_mut().remove_resource::<RecordStore>();
}

fn store<'w>(world: &'w World, args: &CommandArgs) -> Result<&'w RecordStore, CommandError> {
    world.get_resource::<RecordStore>().ok_or_else(|| {
        args.failure(
            "this app has no server records (a network client never has them; run it on the \
             server or without a network)",
        )
    })
}

fn key_from(args: &CommandArgs) -> Result<RecordKey, CommandError> {
    let owner: String = args.parse(0, "owner")?;
    let part: String = args.parse(1, "part")?;
    Ok(RecordKey::new(owner, part))
}

fn parse_json(args: &CommandArgs, index: usize) -> Result<Value, CommandError> {
    let text: String = args.parse(index, "json")?;
    serde_json::from_str(&text)
        .map_err(|error| args.failure(format!("<json> `{text}` is not valid JSON: {error}")))
}

fn load_required(
    world: &World,
    args: &CommandArgs,
) -> Result<(RecordKey, StoredRecord), CommandError> {
    let key = key_from(args)?;
    let found = store(world, args)?
        .read(&key)
        .map_err(|error| args.failure(error.to_string()))?;
    match found {
        Some(record) => Ok((key, record)),
        None => Err(args.failure(format!("there is no record `{key}`"))),
    }
}

/// `records.write <owner> <part> <version> <json>`.
fn write_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let key = key_from(args)?;
    let version: u32 = args.parse(2, "version")?;
    let data = parse_json(args, 3)?;
    store(world, args)?;
    world
        .resource_mut::<RecordStore>()
        .put(key, version, data)
        .map_err(|error| args.failure(error.to_string()))?;
    Ok(CommandOutcome::Continue)
}

/// `records.read <owner> <part>`: logs the record.
fn read_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let (key, record) = load_required(world, args)?;
    info!(
        "record `{key}`: version {}, data {}",
        record.version, record.data
    );
    Ok(CommandOutcome::Continue)
}

/// `records.delete <owner> <part>`: deleting a missing record is fine.
fn delete_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let key = key_from(args)?;
    store(world, args)?;
    world
        .resource_mut::<RecordStore>()
        .write(RecordBatch::new().delete(key))
        .map_err(|error| args.failure(error.to_string()))?;
    Ok(CommandOutcome::Continue)
}

/// `records.list <owner>`: logs the parts stored for an owner.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let owner: String = args.parse(0, "owner")?;
    let parts = store(world, args)?
        .parts(&owner)
        .map_err(|error| args.failure(error.to_string()))?;
    info!(
        "records of `{owner}`: {}",
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(", ")
        }
    );
    Ok(CommandOutcome::Continue)
}

/// `records.status`: logs where the records are.
fn status_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    info!("server records: in {}", store(world, args)?.location());
    Ok(CommandOutcome::Continue)
}

/// `assert.record_field <owner> <part> <field> <json>`: a top-level field of the record's data.
fn assert_field_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let field: String = args.parse(2, "field")?;
    let expected = parse_json(args, 3)?;
    let (key, record) = load_required(world, args)?;
    match record.data.get(&field) {
        Some(actual) if *actual == expected => Ok(CommandOutcome::Continue),
        Some(actual) => Err(args.failure(format!(
            "field `{field}` of record `{key}` is {actual}, expected {expected}"
        ))),
        None => Err(args.failure(format!(
            "record `{key}` has no field `{field}`; its data is {}",
            record.data
        ))),
    }
}

/// `assert.record_absent <owner> <part>`.
fn assert_absent_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let key = key_from(args)?;
    let found = store(world, args)?
        .read(&key)
        .map_err(|error| args.failure(error.to_string()))?;
    if found.is_none() {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("record `{key}` still exists")))
    }
}
