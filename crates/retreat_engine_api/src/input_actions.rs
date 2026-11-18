//! Input mapping: named actions bound to keys and mouse buttons, in data.
//!
//! A module declares actions with [`AppInputExt::register_action`]: an id (`jump`), the command
//! line a press runs (`player.jump`), and default keys. A key press dispatches that command line
//! through the command registry, exactly as a script line or a UI button would, so every action
//! is drivable by scripts and the browser bridge. Bindings are data: `input.bind` and
//! `input.unbind` change them at run time.
//!
//! Two ways a module reads input:
//! - **One-shot:** the action's command runs once when a bound key goes down (the engine does
//!   this; the module only provides the command).
//! - **Held:** [`InputActions::is_held`] says whether any bound key is down right now. Read it
//!   from a `FixedUpdate` system ordered `.after(InputActionSystems)`, so the value is the one for
//!   this tick on every platform.
//!
//! This crate names no input library: [`InputKey`] is our own enum, mapped onto Bevy's key codes in
//! exactly one place in the engine (`input_actions.rs`).

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;

/// A key or mouse button an action can be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InputKey {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Space,
    Enter,
    Escape,
    Tab,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    ShiftLeft,
    ControlLeft,
    MouseLeft,
    MouseRight,
    MouseMiddle,
}

/// Every key with its name, in one table: [`InputKey::parse`], [`InputKey::name`] and the
/// error messages all read it. A new [`InputKey`] variant must be added here too (a test checks
/// that every entry round-trips and that names are unique).
pub const INPUT_KEY_NAMES: &[(&str, InputKey)] = &[
    ("A", InputKey::A),
    ("B", InputKey::B),
    ("C", InputKey::C),
    ("D", InputKey::D),
    ("E", InputKey::E),
    ("F", InputKey::F),
    ("G", InputKey::G),
    ("H", InputKey::H),
    ("I", InputKey::I),
    ("J", InputKey::J),
    ("K", InputKey::K),
    ("L", InputKey::L),
    ("M", InputKey::M),
    ("N", InputKey::N),
    ("O", InputKey::O),
    ("P", InputKey::P),
    ("Q", InputKey::Q),
    ("R", InputKey::R),
    ("S", InputKey::S),
    ("T", InputKey::T),
    ("U", InputKey::U),
    ("V", InputKey::V),
    ("W", InputKey::W),
    ("X", InputKey::X),
    ("Y", InputKey::Y),
    ("Z", InputKey::Z),
    ("Space", InputKey::Space),
    ("Enter", InputKey::Enter),
    ("Escape", InputKey::Escape),
    ("Tab", InputKey::Tab),
    ("ArrowLeft", InputKey::ArrowLeft),
    ("ArrowRight", InputKey::ArrowRight),
    ("ArrowUp", InputKey::ArrowUp),
    ("ArrowDown", InputKey::ArrowDown),
    ("ShiftLeft", InputKey::ShiftLeft),
    ("ControlLeft", InputKey::ControlLeft),
    ("MouseLeft", InputKey::MouseLeft),
    ("MouseRight", InputKey::MouseRight),
    ("MouseMiddle", InputKey::MouseMiddle),
];

impl InputKey {
    /// The key called `name` (case-insensitive), or an error that lists every valid name.
    pub fn parse(name: &str) -> Result<InputKey, String> {
        INPUT_KEY_NAMES
            .iter()
            .find(|(key_name, _)| key_name.eq_ignore_ascii_case(name))
            .map(|(_, key)| *key)
            .ok_or_else(|| {
                let names: Vec<&str> = INPUT_KEY_NAMES.iter().map(|(name, _)| *name).collect();
                format!("unknown key `{name}`; keys: {}", names.join(", "))
            })
    }

    /// The name used in commands and messages.
    pub fn name(self) -> &'static str {
        INPUT_KEY_NAMES
            .iter()
            .find(|(_, key)| *key == self)
            .map(|(name, _)| *name)
            .expect("INPUT_KEY_NAMES must list every InputKey")
    }
}

/// The `command_line` of an action that runs no command and is only read as held
/// ([`InputActions::is_held`]), e.g. continuous camera rotation.
pub const HELD_ONLY: &str = "";

/// What a module declares for one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputAction {
    /// Unique id, one word (`jump`, `orbit_left`).
    pub id: &'static str,
    /// The command line run when a bound key goes down (`player.jump`), or [`HELD_ONLY`]. Its
    /// command must be registered by the time all plugins are built; the engine checks this at
    /// startup.
    pub command_line: &'static str,
    /// Keys bound until `input.bind` / `input.unbind` change them.
    pub default_keys: &'static [InputKey],
}

/// One registered action and its current state.
#[derive(Clone, Debug)]
struct ActionEntry {
    command_line: &'static str,
    keys: BTreeSet<InputKey>,
    held: bool,
}

/// Every action of the app (a Bevy resource). Created by [`AppInputExt::register_action`].
#[derive(Resource, Default, Debug)]
pub struct InputActions {
    actions: BTreeMap<&'static str, ActionEntry>,
}

impl InputActions {
    /// Adds an action with its default keys.
    ///
    /// # Panics
    /// If the id is empty, contains whitespace, or is already registered: two features claiming
    /// one action id is a bug to fix in code (same rule as command ids).
    pub fn register(&mut self, action: InputAction) {
        assert!(
            !action.id.is_empty() && !action.id.contains(char::is_whitespace),
            "InputActions::register: invalid action id `{}`; an id is one word like `jump`",
            action.id
        );
        assert!(
            !self.actions.contains_key(action.id),
            "InputActions::register: action `{}` is registered twice; action ids must be \
             unique (search the code for `\"{}\"`)",
            action.id,
            action.id
        );
        self.actions.insert(
            action.id,
            ActionEntry {
                command_line: action.command_line,
                keys: action.default_keys.iter().copied().collect(),
                held: false,
            },
        );
    }

    /// All action ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        self.actions.keys().copied().collect()
    }

    /// Whether an action with this id is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.actions.contains_key(id)
    }

    /// The command line an action runs; `None` for an unknown id or a [`HELD_ONLY`] action.
    pub fn command_line(&self, id: &str) -> Option<&'static str> {
        self.actions
            .get(id)
            .map(|entry| entry.command_line)
            .filter(|line| !line.is_empty())
    }

    /// The keys bound to an action, in key order. Empty for an unknown id.
    pub fn keys(&self, id: &str) -> Vec<InputKey> {
        self.actions
            .get(id)
            .map(|entry| entry.keys.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Whether any bound key of the action is down as of the last
    /// [`InputActionSystems`] run. `false` for an unknown id.
    pub fn is_held(&self, id: &str) -> bool {
        self.actions.get(id).is_some_and(|entry| entry.held)
    }

    /// Binds one more key. Several actions may share a key: a press runs all of them, in action-id
    /// order.
    pub fn bind(&mut self, id: &str, key: InputKey) -> Result<(), String> {
        self.entry_mut(id)?.keys.insert(key);
        Ok(())
    }

    /// Removes a binding. Fails if the action does not have that key, so a typo is visible.
    pub fn unbind(&mut self, id: &str, key: InputKey) -> Result<(), String> {
        if self.entry_mut(id)?.keys.remove(&key) {
            Ok(())
        } else {
            Err(format!(
                "action `{id}` is not bound to `{}`; its keys: {}",
                key.name(),
                self.key_names(id)
            ))
        }
    }

    /// The action ids bound to `key`, sorted: the actions a press of `key` runs.
    pub fn actions_for_key(&self, key: InputKey) -> Vec<&'static str> {
        self.actions
            .iter()
            .filter(|(_, entry)| entry.keys.contains(&key))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Recomputes every action's held state from the keys that are down. Called by the engine
    /// once per fixed tick; modules do not call it.
    pub fn update_held(&mut self, is_key_down: impl Fn(InputKey) -> bool) {
        for entry in self.actions.values_mut() {
            entry.held = entry.keys.iter().any(|key| is_key_down(*key));
        }
    }

    /// Ids of actions whose command line starts with a command id that `is_registered` rejects,
    /// with that command id: the startup check of the engine.
    pub fn actions_with_unknown_commands(
        &self,
        is_registered: impl Fn(&str) -> bool,
    ) -> Vec<(&'static str, String)> {
        self.actions
            .iter()
            .filter_map(|(id, entry)| {
                if entry.command_line == HELD_ONLY {
                    return None;
                }
                let command_id = entry.command_line.split_whitespace().next().unwrap_or("");
                (!is_registered(command_id)).then(|| (*id, command_id.to_string()))
            })
            .collect()
    }

    /// `key names` of an action for messages: `W, ArrowUp` or `(none)`.
    pub fn key_names(&self, id: &str) -> String {
        let names: Vec<&str> = self.keys(id).iter().map(|key| key.name()).collect();
        if names.is_empty() {
            "(none)".to_string()
        } else {
            names.join(", ")
        }
    }

    fn entry_mut(&mut self, id: &str) -> Result<&mut ActionEntry, String> {
        if !self.actions.contains_key(id) {
            let ids: Vec<&str> = self.actions.keys().copied().collect();
            return Err(format!("no action `{id}`; actions: {}", ids.join(", ")));
        }
        Ok(self.actions.get_mut(id).expect("checked above"))
    }
}

/// Ordering handle. The engine updates held state in this set, in `FixedUpdate`, after the
/// script line of the tick. A module system that reads [`InputActions::is_held`] runs
/// `.after(InputActionSystems)`, so single-threaded WASM and multi-threaded native agree.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct InputActionSystems;

/// Adds `register_action` to Bevy's `App`, so a module's plugin can declare its actions.
pub trait AppInputExt {
    /// Registers an action, creating [`InputActions`] if needed. See [`InputActions::register`].
    fn register_action(&mut self, action: InputAction) -> &mut Self;
}

impl AppInputExt for App {
    fn register_action(&mut self, action: InputAction) -> &mut Self {
        self.init_resource::<InputActions>();
        self.world_mut()
            .resource_mut::<InputActions>()
            .register(action);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jump_action() -> InputAction {
        InputAction {
            id: "jump",
            command_line: "player.jump",
            default_keys: &[InputKey::Space],
        }
    }

    #[test]
    fn every_key_name_round_trips_and_names_are_unique() {
        let mut seen = BTreeSet::new();
        for (name, key) in INPUT_KEY_NAMES {
            assert_eq!(InputKey::parse(name), Ok(*key));
            assert_eq!(key.name(), *name);
            assert!(seen.insert(name.to_ascii_lowercase()), "duplicate {name}");
        }
    }

    #[test]
    fn parse_ignores_case_and_lists_valid_keys_on_error() {
        assert_eq!(InputKey::parse("space"), Ok(InputKey::Space));
        let error = InputKey::parse("Spcae").unwrap_err();
        assert!(error.contains("unknown key `Spcae`"), "{error}");
        assert!(error.contains("MouseLeft"), "{error}");
    }

    #[test]
    fn register_installs_default_keys() {
        let mut actions = InputActions::default();
        actions.register(jump_action());
        assert_eq!(actions.keys("jump"), vec![InputKey::Space]);
        assert_eq!(actions.command_line("jump"), Some("player.jump"));
        assert_eq!(actions.actions_for_key(InputKey::Space), vec!["jump"]);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn registering_an_action_twice_panics() {
        let mut actions = InputActions::default();
        actions.register(jump_action());
        actions.register(jump_action());
    }

    #[test]
    fn bind_and_unbind_change_the_keys() {
        let mut actions = InputActions::default();
        actions.register(jump_action());
        actions.bind("jump", InputKey::W).unwrap();
        assert_eq!(actions.key_names("jump"), "W, Space");
        actions.unbind("jump", InputKey::Space).unwrap();
        assert_eq!(actions.keys("jump"), vec![InputKey::W]);
        let error = actions.unbind("jump", InputKey::Space).unwrap_err();
        assert!(error.contains("not bound to `Space`"), "{error}");
        let error = actions.bind("jmup", InputKey::W).unwrap_err();
        assert!(error.contains("no action `jmup`; actions: jump"), "{error}");
    }

    #[test]
    fn two_actions_on_one_key_both_fire_in_id_order() {
        let mut actions = InputActions::default();
        actions.register(InputAction {
            id: "b_second",
            command_line: "x.b",
            default_keys: &[InputKey::E],
        });
        actions.register(InputAction {
            id: "a_first",
            command_line: "x.a",
            default_keys: &[InputKey::E],
        });
        assert_eq!(
            actions.actions_for_key(InputKey::E),
            vec!["a_first", "b_second"]
        );
    }

    #[test]
    fn held_follows_the_keys_that_are_down() {
        let mut actions = InputActions::default();
        actions.register(jump_action());
        assert!(!actions.is_held("jump"));
        actions.update_held(|key| key == InputKey::Space);
        assert!(actions.is_held("jump"));
        actions.update_held(|_| false);
        assert!(!actions.is_held("jump"));
        assert!(!actions.is_held("no_such_action"));
    }

    #[test]
    fn held_only_actions_have_no_command_and_are_never_unknown() {
        let mut actions = InputActions::default();
        actions.register(InputAction {
            id: "hold",
            command_line: HELD_ONLY,
            default_keys: &[InputKey::Q],
        });
        assert!(actions.contains("hold"));
        assert_eq!(actions.command_line("hold"), None);
        assert!(actions.actions_with_unknown_commands(|_| false).is_empty());
    }

    #[test]
    fn unknown_commands_are_reported_with_their_action() {
        let mut actions = InputActions::default();
        actions.register(jump_action());
        let unknown = actions.actions_with_unknown_commands(|id| id == "other");
        assert_eq!(unknown, vec![("jump", "player.jump".to_string())]);
        assert!(
            actions
                .actions_with_unknown_commands(|id| id == "player.jump")
                .is_empty()
        );
    }
}
