//! Saving, engine side: creates the app's `SaveStore` on the right backend and adds the `save.*`
//! commands. The backends themselves (files, `localStorage`) live in the platform layer.
//!
//! **Which backend.** Under a command script the store is in memory, so a run starts empty and
//! leaves nothing behind (reproducible, like every script). Interactive play uses the platform
//! backend. A script that wants to test the real platform backend says so with
//! `save.use_platform_backend` (and cleans up with `save.delete`).
//!
//! Commands: `save.write <module> <version> <json>` (the JSON has no spaces: command arguments
//! are split on whitespace), `save.read <module>` (logs the document), `save.delete <module>`,
//! `save.list`, `save.status`, `save.use_platform_backend`,
//! `assert.save_version <module> <version>`, `assert.save_field <module> <field> <json>`,
//! `assert.save_absent <module>`.

use bevy::prelude::*;
use retreat_engine_api::serde_json::{self, Value};
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, SaveError, SaveStore,
    SavedData,
};

use crate::CommandScript;
use crate::platform::{SAVE_LOCATION, platform_save_backend};

/// Adds the `SaveStore` resource and the `save.*` commands.
pub struct StorageCommandsPlugin;

impl Plugin for StorageCommandsPlugin {
    fn build(&self, app: &mut App) {
        // A script, or a test that already inserted a store, keeps memory/its own; otherwise the
        // platform's backend.
        if !app.world().contains_resource::<SaveStore>() {
            let store = if app.world().contains_resource::<CommandScript>() {
                SaveStore::in_memory()
            } else {
                SaveStore::new(platform_save_backend())
            };
            app.insert_resource(store);
        }
        app.register_command(
            "save.write",
            "save.write <module> <version> <json>",
            write_command,
        )
        .register_command("save.read", "save.read <module>", read_command)
        .register_command("save.delete", "save.delete <module>", delete_command)
        .register_command("save.list", "save.list", list_command)
        .register_command("save.status", "save.status", status_command)
        .register_command(
            "save.use_platform_backend",
            "save.use_platform_backend",
            use_platform_backend_command,
        )
        .register_command(
            "assert.save_version",
            "assert.save_version <module> <version>",
            assert_version_command,
        )
        .register_command(
            "assert.save_field",
            "assert.save_field <module> <field> <json>",
            assert_field_command,
        )
        .register_command(
            "assert.save_absent",
            "assert.save_absent <module>",
            assert_absent_command,
        );
    }
}

fn failure(args: &CommandArgs, error: SaveError) -> CommandError {
    args.failure(error.to_string())
}

/// The stored document of the module named by argument 0, or an error saying it is missing.
fn load_required(world: &World, args: &CommandArgs) -> Result<(String, SavedData), CommandError> {
    let module: String = args.parse(0, "module")?;
    let loaded = world
        .resource::<SaveStore>()
        .load(&module)
        .map_err(|error| failure(args, error))?;
    match loaded {
        Some(saved) => Ok((module, saved)),
        None => Err(args.failure(format!(
            "module `{module}` has no save; write one with `save.write {module} <version> <json>`"
        ))),
    }
}

/// `save.write <module> <version> <json>`.
fn write_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let module: String = args.parse(0, "module")?;
    let version: u32 = args.parse(1, "version")?;
    let text: String = args.parse(2, "json")?;
    let data: Value = serde_json::from_str(&text)
        .map_err(|error| args.failure(format!("<json> `{text}` is not valid JSON: {error}")))?;
    world
        .resource_mut::<SaveStore>()
        .save(&module, version, &data)
        .map_err(|error| failure(args, error))?;
    Ok(CommandOutcome::Continue)
}

/// `save.read <module>`: logs the module's document.
fn read_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let (module, saved) = load_required(world, args)?;
    info!(
        "save `{module}`: version {}, data {}",
        saved.version, saved.data
    );
    Ok(CommandOutcome::Continue)
}

/// `save.delete <module>`: deleting a module with no save is not an error.
fn delete_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let module: String = args.parse(0, "module")?;
    world
        .resource_mut::<SaveStore>()
        .delete(&module)
        .map_err(|error| failure(args, error))?;
    Ok(CommandOutcome::Continue)
}

/// `save.list`: logs the modules that have a save.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let modules = world
        .resource::<SaveStore>()
        .modules()
        .map_err(|error| failure(args, error))?;
    if modules.is_empty() {
        info!("saves: none");
    } else {
        info!("saves: {}", modules.join(", "));
    }
    Ok(CommandOutcome::Continue)
}

/// `save.status`: logs where saves live on this platform.
fn status_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let in_script = world.contains_resource::<CommandScript>();
    info!(
        "saves: this platform stores them in {SAVE_LOCATION}{}",
        if in_script {
            "; a running script keeps them in memory until `save.use_platform_backend`"
        } else {
            ""
        }
    );
    Ok(CommandOutcome::Continue)
}

/// `save.use_platform_backend`: switches the store to the platform backend (files or
/// `localStorage`), so a script can test the real thing.
fn use_platform_backend_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world
        .resource_mut::<SaveStore>()
        .replace_backend(platform_save_backend());
    Ok(CommandOutcome::Continue)
}

/// `assert.save_version <module> <version>`.
fn assert_version_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let expected: u32 = args.parse(1, "version")?;
    let (module, saved) = load_required(world, args)?;
    if saved.version == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "the save of `{module}` is version {}, expected {expected}",
            saved.version
        )))
    }
}

/// `assert.save_field <module> <field> <json>`: the top-level field of the saved data equals the
/// JSON value (written without spaces).
fn assert_field_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let field: String = args.parse(1, "field")?;
    let text: String = args.parse(2, "json")?;
    let expected: Value = serde_json::from_str(&text)
        .map_err(|error| args.failure(format!("<json> `{text}` is not valid JSON: {error}")))?;
    let (module, saved) = load_required(world, args)?;
    match saved.data.get(&field) {
        Some(actual) if *actual == expected => Ok(CommandOutcome::Continue),
        Some(actual) => Err(args.failure(format!(
            "field `{field}` of the save of `{module}` is {actual}, expected {expected}"
        ))),
        None => Err(args.failure(format!(
            "the save of `{module}` has no field `{field}`; its data is {}",
            saved.data
        ))),
    }
}

/// `assert.save_absent <module>`: fails if the module has a save.
fn assert_absent_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let module: String = args.parse(0, "module")?;
    let loaded = world
        .resource::<SaveStore>()
        .load(&module)
        .map_err(|error| failure(args, error))?;
    if loaded.is_none() {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("module `{module}` still has a save")))
    }
}
