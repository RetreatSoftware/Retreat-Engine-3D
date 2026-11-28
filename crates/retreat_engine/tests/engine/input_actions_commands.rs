//! Input mapping in an app without a window or GPU: scripted key presses run the bound action's
//! command once, held state follows the keys per tick, bindings are data, and mistakes give clear
//! errors. The sandbox check is `scripts/verify/input_actions.cmds`.

use bevy::prelude::*;
use retreat_engine::{CommandScript, InputActionsPlugin, RetreatCommandsPlugin};
use retreat_engine_api::{
    AppCommandExt, AppInputExt, CommandOutcome, InputAction, InputActionSystems, InputActions,
    InputKey,
};

const MAX_UPDATES: u32 = 200;

/// How often `test.hit` ran.
#[derive(Resource, Default)]
struct Hits(u32);

/// How often `test.other` ran.
#[derive(Resource, Default)]
struct OtherHits(u32);

/// `Held` as a module system sees it: copied each tick, after `InputActionSystems`.
#[derive(Resource, Default)]
struct HeldLog(Vec<bool>);

fn add_test_actions(app: &mut App) {
    app.init_resource::<Hits>()
        .init_resource::<OtherHits>()
        .init_resource::<HeldLog>()
        .register_command("test.hit", "test.hit", |world, _args| {
            world.resource_mut::<Hits>().0 += 1;
            Ok(CommandOutcome::Continue)
        })
        .register_command("test.other", "test.other", |world, _args| {
            world.resource_mut::<OtherHits>().0 += 1;
            Ok(CommandOutcome::Continue)
        })
        .register_action(InputAction {
            id: "hit",
            command_line: "test.hit",
            default_keys: &[InputKey::Space],
        })
        .register_action(InputAction {
            id: "other",
            command_line: "test.other",
            default_keys: &[InputKey::Space],
        })
        .add_systems(FixedUpdate, record_held.after(InputActionSystems));
}

fn record_held(actions: Res<InputActions>, mut log: ResMut<HeldLog>) {
    log.0.push(actions.is_held("hit"));
}

fn test_app(text: &str) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins(InputActionsPlugin);
    add_test_actions(&mut app);
    app.finish();
    app.cleanup();
    app
}

/// Runs the script to its end; returns the app and the script's failure, if any.
fn run_script(text: &str) -> (App, Option<String>) {
    let mut app = test_app(text);
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            let failure = app
                .world()
                .resource::<CommandScript>()
                .failure()
                .map(String::from);
            return (app, failure);
        }
    }
    panic!("the script did not finish within {MAX_UPDATES} updates");
}

#[test]
fn key_down_runs_the_bound_command_once_however_long_it_is_held() {
    let (app, failure) = run_script("input.key_down Space\nwait 10\nassert.input_held hit\nquit\n");
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 1);
}

#[test]
fn pressing_again_after_a_release_runs_the_command_again() {
    let (app, failure) = run_script(
        "input.key_down Space\nwait 2\ninput.key_up Space\nwait 2\ninput.key_down Space\nwait 2\nquit\n",
    );
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 2);
}

#[test]
fn held_state_follows_the_key_and_is_seen_by_module_systems_in_the_same_tick() {
    let (app, failure) = run_script(
        "assert.input_not_held hit\ninput.key_down Space\nassert.input_held hit\n\
         input.key_up Space\nwait 1\nassert.input_not_held hit\nquit\n",
    );
    assert_eq!(failure, None);
    let log = &app.world().resource::<HeldLog>().0;
    // The tick that ran `input.key_down` already reports the key as held, because
    // `InputActionSystems` runs after the script line.
    assert_eq!(&log[..4], [false, true, true, false], "{log:?}");
}

#[test]
fn tap_holds_the_key_for_exactly_one_tick_and_runs_the_command_once() {
    // The tick after the tap still reports the held state of the tap's own tick...
    let (app, failure) = run_script("input.tap Space\nassert.input_held hit\nquit\n");
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 1);

    // ...and the tick after that no longer does.
    let (app, failure) = run_script("input.tap Space\nwait 1\nassert.input_not_held hit\nquit\n");
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 1);
}

#[test]
fn two_actions_on_one_key_both_run() {
    let (app, failure) = run_script("input.tap Space\nwait 1\nquit\n");
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 1);
    assert_eq!(app.world().resource::<OtherHits>().0, 1);
}

#[test]
fn bind_and_unbind_change_which_key_runs_the_action() {
    let (app, failure) = run_script(
        "input.unbind hit Space\ninput.bind hit Q\ninput.tap Space\nwait 1\ninput.tap Q\nwait 1\nquit\n",
    );
    assert_eq!(failure, None);
    // `hit` ran only for Q; `other` still runs for Space.
    assert_eq!(app.world().resource::<Hits>().0, 1);
    assert_eq!(app.world().resource::<OtherHits>().0, 1);
}

#[test]
fn mouse_buttons_work_like_keys() {
    let (app, failure) =
        run_script("input.bind hit MouseLeft\ninput.tap MouseLeft\nwait 1\nquit\n");
    assert_eq!(failure, None);
    assert_eq!(app.world().resource::<Hits>().0, 1);
}

#[test]
fn a_mouse_press_over_a_ui_button_runs_no_action_but_a_key_does() {
    use bevy::picking::hover::HoverMap;
    use bevy::picking::pointer::PointerId;
    let mut app = test_app(
        "input.bind hit MouseLeft
input.tap MouseLeft
wait 1
input.tap Space
wait 1
quit
",
    );
    let button = app.world_mut().spawn(retreat_engine::UiClickTarget).id();
    let mut hover = HoverMap::default();
    hover.entry(PointerId::Mouse).or_default().insert(
        button,
        bevy::picking::backend::HitData::new(button, 0.0, None, None),
    );
    app.insert_resource(hover);
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    assert_eq!(app.world().resource::<CommandScript>().failure(), None);
    // The click went to the button; Space (also bound to `hit`) still ran it once.
    assert_eq!(app.world().resource::<Hits>().0, 1);
}

#[test]
fn errors_name_the_command_the_bad_value_and_the_valid_choices() {
    let (_, failure) = run_script("input.key_down Spcae\nquit\n");
    let failure = failure.expect("an unknown key must fail");
    assert!(failure.contains("unknown key `Spcae`"), "{failure}");
    assert!(failure.contains("keys: A, B"), "{failure}");

    let (_, failure) = run_script("input.bind jmup Space\nquit\n");
    let failure = failure.expect("an unknown action must fail");
    assert!(
        failure.contains("no action `jmup`; actions: hit, other"),
        "{failure}"
    );

    let (_, failure) = run_script("input.unbind hit Q\nquit\n");
    let failure = failure.expect("unbinding an unbound key must fail");
    assert!(failure.contains("not bound to `Q`"), "{failure}");

    let (_, failure) = run_script("input.key_down Space\ninput.tap Space\nquit\n");
    let failure = failure.expect("a tap on a held key must fail");
    assert!(failure.contains("already down"), "{failure}");

    let (_, failure) = run_script("assert.input_held hit\nquit\n");
    let failure = failure.expect("an action that is not held must fail the assert");
    assert!(failure.contains("action `hit` is not held"), "{failure}");
}

#[test]
#[should_panic(expected = "input action `broken` runs command `no.such_command`")]
fn an_action_with_an_unregistered_command_panics_at_startup() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins(InputActionsPlugin)
        .register_action(InputAction {
            id: "broken",
            command_line: "no.such_command",
            default_keys: &[InputKey::X],
        });
    app.finish();
}

#[test]
fn an_action_may_name_a_command_registered_by_a_later_plugin() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin::default())
        .add_plugins(InputActionsPlugin)
        .register_action(InputAction {
            id: "early",
            command_line: "late.command",
            default_keys: &[InputKey::X],
        })
        .register_command("late.command", "late.command", |_world, _args| {
            Ok(CommandOutcome::Continue)
        });
    app.finish();
}
