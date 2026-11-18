//! The command registry: every command by id, and the single function that runs one.

use std::collections::BTreeMap;

use bevy::prelude::*;

use crate::{CommandArgs, CommandError, ParsedCommandLine, parse_command_line};

/// What the caller should do after a command succeeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Go on: a script runs its next line on the next frame.
    Continue,
    /// A script stays idle for this many frames before its next line (used by `wait`).
    WaitFrames(u32),
}

/// What every command handler returns.
pub type CommandResult = Result<CommandOutcome, CommandError>;

/// A command's code, stored behind a pointer so handlers of different types fit in one map.
/// It gets the whole `World`, so it can read and change anything a player action could.
type CommandHandler = Box<dyn Fn(&mut World, &CommandArgs) -> CommandResult + Send + Sync>;

struct RegisteredCommand {
    usage: &'static str,
    handler: CommandHandler,
}

/// Every command of the app, by id. One per app (a Bevy resource), created by
/// [`AppCommandExt::register_command`] or by `RetreatEnginePlugin`.
/// Sorted by id (`BTreeMap`) so the list in error messages is stable.
#[derive(Resource, Default)]
pub struct CommandRegistry {
    commands: BTreeMap<&'static str, RegisteredCommand>,
}

impl CommandRegistry {
    /// Adds a command. `id` is one word (`area.verb`, e.g. `physics.spawn_box`); `usage`
    /// shows how to call it (`wait <frames>`) and is printed in argument errors.
    ///
    /// # Panics
    /// If `id` is empty, contains whitespace, or is already registered: two features
    /// claiming the same id is a bug that must be fixed in code, not at run time.
    pub fn register(
        &mut self,
        id: &'static str,
        usage: &'static str,
        handler: impl Fn(&mut World, &CommandArgs) -> CommandResult + Send + Sync + 'static,
    ) {
        assert!(
            !id.is_empty() && !id.contains(char::is_whitespace),
            "CommandRegistry::register: invalid command id `{id}`; an id is one word like `area.verb`"
        );
        if self.commands.contains_key(id) {
            panic!(
                "CommandRegistry::register: command `{id}` is registered twice; \
                 command ids must be unique (search the code for `\"{id}\"`)"
            );
        }
        let command = RegisteredCommand {
            usage,
            handler: Box::new(handler),
        };
        self.commands.insert(id, command);
    }

    /// All registered ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        self.commands.keys().copied().collect()
    }

    fn execute(&self, world: &mut World, line: ParsedCommandLine) -> CommandResult {
        let Some(command) = self.commands.get(line.id.as_str()) else {
            let known_ids = self.ids().into_iter().map(String::from).collect();
            return Err(CommandError::UnknownCommand {
                id: line.id,
                known_ids,
            });
        };
        let args = CommandArgs::new(line.id, command.usage, line.args);
        (command.handler)(world, &args)
    }
}

/// Runs one line of command text, e.g. `wait 3`. This is the single entry point that
/// command scripts, UI buttons, keys and the browser bridge all use.
/// A blank line or a comment does nothing and returns `Continue`.
///
/// # Panics
/// If the world has no [`CommandRegistry`], or if called from inside a command handler
/// (the registry is taken out of the world while a handler runs).
pub fn run_command_line(world: &mut World, line: &str) -> CommandResult {
    let Some(parsed) = parse_command_line(line) else {
        return Ok(CommandOutcome::Continue);
    };
    assert!(
        world.contains_resource::<CommandRegistry>(),
        "run_command_line: the world has no CommandRegistry; add RetreatEnginePlugin \
         (or call app.register_command) before running commands, and do not run a command \
         from inside another command's handler"
    );
    // The handler needs `&mut World`, but the registry lives in that world. `resource_scope`
    // takes the registry out, runs the closure, then puts it back, so both borrows are legal.
    world.resource_scope(|world, registry: Mut<CommandRegistry>| registry.execute(world, parsed))
}

/// Adds `register_command` to Bevy's `App`, so plugins (engine and modules) can register
/// commands in their `build` method.
pub trait AppCommandExt {
    /// Registers a command, creating the [`CommandRegistry`] if needed. See [`CommandRegistry::register`].
    fn register_command(
        &mut self,
        id: &'static str,
        usage: &'static str,
        handler: impl Fn(&mut World, &CommandArgs) -> CommandResult + Send + Sync + 'static,
    ) -> &mut Self;
}

impl AppCommandExt for App {
    fn register_command(
        &mut self,
        id: &'static str,
        usage: &'static str,
        handler: impl Fn(&mut World, &CommandArgs) -> CommandResult + Send + Sync + 'static,
    ) -> &mut Self {
        self.init_resource::<CommandRegistry>();
        self.world_mut()
            .resource_mut::<CommandRegistry>()
            .register(id, usage, handler);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts how often the `test.count` command ran.
    #[derive(Resource, Default)]
    struct TestCounter(u32);

    fn count_command(world: &mut World, args: &CommandArgs) -> CommandResult {
        args.expect_count(1)?;
        let amount: u32 = args.parse(0, "amount")?;
        world.resource_mut::<TestCounter>().0 += amount;
        Ok(CommandOutcome::Continue)
    }

    fn world_with_test_commands() -> World {
        let mut world = World::new();
        world.init_resource::<TestCounter>();
        let mut registry = CommandRegistry::default();
        registry.register("test.count", "test.count <amount>", count_command);
        registry.register("test.pause", "test.pause", |_world, _args| {
            Ok(CommandOutcome::WaitFrames(2))
        });
        world.insert_resource(registry);
        world
    }

    #[test]
    fn runs_the_handler_with_its_arguments() {
        let mut world = world_with_test_commands();
        assert_eq!(
            run_command_line(&mut world, "test.count 5"),
            Ok(CommandOutcome::Continue)
        );
        assert_eq!(
            run_command_line(&mut world, "test.count 2"),
            Ok(CommandOutcome::Continue)
        );
        assert_eq!(world.resource::<TestCounter>().0, 7);
    }

    #[test]
    fn returns_the_handler_outcome() {
        let mut world = world_with_test_commands();
        assert_eq!(
            run_command_line(&mut world, "test.pause"),
            Ok(CommandOutcome::WaitFrames(2))
        );
    }

    #[test]
    fn unknown_command_lists_the_known_ids() {
        let mut world = world_with_test_commands();
        let error = run_command_line(&mut world, "test.cuont 1").unwrap_err();
        assert_eq!(
            error,
            CommandError::UnknownCommand {
                id: "test.cuont".to_string(),
                known_ids: vec!["test.count".to_string(), "test.pause".to_string()],
            }
        );
    }

    #[test]
    fn argument_errors_come_back_from_the_handler() {
        let mut world = world_with_test_commands();
        let error = run_command_line(&mut world, "test.count many").unwrap_err();
        assert!(
            error.to_string().contains("usage: test.count <amount>"),
            "{error}"
        );
        assert_eq!(
            world.resource::<TestCounter>().0,
            0,
            "a failed command must not change state"
        );
    }

    #[test]
    fn blank_lines_and_comments_do_nothing() {
        let mut world = world_with_test_commands();
        assert_eq!(
            run_command_line(&mut world, "# note"),
            Ok(CommandOutcome::Continue)
        );
        assert_eq!(
            run_command_line(&mut world, ""),
            Ok(CommandOutcome::Continue)
        );
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn registering_an_id_twice_panics() {
        let mut registry = CommandRegistry::default();
        registry.register("test.count", "test.count <amount>", count_command);
        registry.register("test.count", "test.count <amount>", count_command);
    }

    #[test]
    #[should_panic(expected = "invalid command id")]
    fn registering_an_id_with_spaces_panics() {
        let mut registry = CommandRegistry::default();
        registry.register("test count", "test count", count_command);
    }

    #[test]
    fn app_register_command_creates_the_registry() {
        let mut app = App::new();
        app.register_command("test.count", "test.count <amount>", count_command);
        assert_eq!(
            app.world().resource::<CommandRegistry>().ids(),
            vec!["test.count"]
        );
    }
}
