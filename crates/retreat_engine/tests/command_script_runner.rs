//! Runs command scripts in a real Bevy app (no window, no GPU) and checks frame timing,
//! exit codes and error messages.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use retreat_engine::{CommandScript, PendingCommandWork, RetreatCommandsPlugin};
use retreat_engine_api::{AppCommandExt, CommandOutcome, SimulationRng};

/// More updates than any test script needs; a test that reaches it never quit.
const MAX_UPDATES: u32 = 1000;

/// Bevy starts its clock on the first update, so that update has no fixed tick:
/// fixed tick N runs during update N + 1.
const UPDATES_BEFORE_FIRST_TICK: u32 = 1;

/// Counts the fixed ticks in which the `test.mark` command ran.
#[derive(Resource, Default)]
struct MarkedTicks(Vec<u32>);

/// Counts fixed ticks, so tests can see on which tick each line ran.
#[derive(Resource, Default)]
struct TickCounter(u32);

fn count_tick(mut counter: ResMut<TickCounter>) {
    counter.0 += 1;
}

/// Fake work started by `test.start_work <ticks> <succeed|fail>`, standing in for a
/// snapshot: it finishes after the given number of ticks.
#[derive(Resource, Default)]
struct FakeWork {
    ticks_left: Option<u32>,
    should_fail: bool,
}

fn advance_fake_work(mut work: ResMut<FakeWork>, mut pending: ResMut<PendingCommandWork>) {
    let Some(ticks_left) = work.ticks_left else {
        return;
    };
    if ticks_left > 1 {
        work.ticks_left = Some(ticks_left - 1);
        return;
    }
    work.ticks_left = None;
    if work.should_fail {
        pending.fail("fake work failed");
    } else {
        pending.finish();
    }
}

fn app_with_script(text: &str) -> App {
    app_with_command_script(CommandScript::from_text("test.cmds", text))
}

fn app_with_command_script(script: CommandScript) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 9,
            command_script: Some(script),
        })
        .init_resource::<MarkedTicks>()
        .init_resource::<TickCounter>()
        .init_resource::<FakeWork>()
        // Count ticks before the script line of the same tick runs.
        .add_systems(FixedPreUpdate, count_tick)
        .add_systems(FixedPostUpdate, advance_fake_work)
        .register_command(
            "test.start_work",
            "test.start_work <ticks> <succeed|fail>",
            |world, args| {
                args.expect_count(2)?;
                let ticks: u32 = args.parse(0, "ticks")?;
                let should_fail = match args.parse::<String>(1, "succeed|fail")?.as_str() {
                    "succeed" => false,
                    "fail" => true,
                    other => {
                        return Err(args.failure(format!("expected succeed or fail, got {other}")));
                    }
                };
                world.resource_mut::<PendingCommandWork>().start();
                // 0 ticks means "never finishes", to test the timeout.
                let ticks_left = if ticks == 0 { None } else { Some(ticks) };
                *world.resource_mut::<FakeWork>() = FakeWork {
                    ticks_left,
                    should_fail,
                };
                Ok(CommandOutcome::Continue)
            },
        )
        .register_command("test.mark", "test.mark", |world, args| {
            args.expect_count(0)?;
            let tick = world.resource::<TickCounter>().0;
            world.resource_mut::<MarkedTicks>().0.push(tick);
            Ok(CommandOutcome::Continue)
        });
    app
}

/// Updates until the app asks to exit; returns how it exited and after how many updates.
fn run_until_exit(app: &mut App) -> (AppExit, u32) {
    for update in 1..=MAX_UPDATES {
        app.update();
        if let Some(exit) = app.should_exit() {
            return (exit, update);
        }
    }
    panic!("the script did not quit within {MAX_UPDATES} updates");
}

#[test]
fn runs_one_line_per_tick_and_quits_successfully() {
    let mut app = app_with_script("test.mark\ntest.mark\nquit\n");
    let (exit, updates) = run_until_exit(&mut app);
    assert_eq!(exit, AppExit::Success);
    assert_eq!(
        updates,
        UPDATES_BEFORE_FIRST_TICK + 3,
        "three lines must take three ticks"
    );
    assert_eq!(app.world().resource::<MarkedTicks>().0, vec![1, 2]);
}

#[test]
fn comments_and_blank_lines_cost_no_tick() {
    let mut app = app_with_script("# setup\n\ntest.mark\n# again\ntest.mark\n");
    for _ in 0..UPDATES_BEFORE_FIRST_TICK + 2 {
        app.update();
    }
    assert_eq!(app.world().resource::<MarkedTicks>().0, vec![1, 2]);
}

#[test]
fn wait_idles_for_the_given_number_of_ticks() {
    let mut app = app_with_script("test.mark\nwait 3\ntest.mark\nquit\n");
    run_until_exit(&mut app);
    // Tick 1: mark. Tick 2: wait. Ticks 3, 4, 5: idle. Tick 6: mark.
    assert_eq!(app.world().resource::<MarkedTicks>().0, vec![1, 6]);
}

#[test]
fn a_failing_line_exits_with_an_error_and_names_file_and_line() {
    let mut app = app_with_script("test.mark\nwiat 2\ntest.mark\nquit\n");
    let (exit, updates) = run_until_exit(&mut app);
    assert_eq!(exit, AppExit::error());
    assert_eq!(
        updates,
        UPDATES_BEFORE_FIRST_TICK + 2,
        "the app must stop on the failing line"
    );
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from);
    let failure = failure.expect("the script must record its failure");
    assert!(
        failure.starts_with("test.cmds:2: unknown command `wiat`"),
        "{failure}"
    );
    assert_eq!(
        app.world().resource::<MarkedTicks>().0,
        vec![1],
        "no line runs after a failure"
    );
}

#[test]
fn assert_commands_pass_and_fail_on_named_entities() {
    let mut app = app_with_script(
        "assert.entity_exists probe\n\
         assert.position probe 1 2 3\n\
         assert.position probe 1 2 4\n",
    );
    app.world_mut()
        .spawn((Name::new("probe"), Transform::from_xyz(1.0, 2.0, 3.0)));
    let (exit, updates) = run_until_exit(&mut app);
    assert_eq!(exit, AppExit::error());
    assert_eq!(
        updates,
        UPDATES_BEFORE_FIRST_TICK + 3,
        "the first two asserts pass, the third fails"
    );
    let script = app.world().resource::<CommandScript>();
    let failure = script.failure().expect("the third assert must fail");
    assert!(
        failure.contains("test.cmds:3: `assert.position` failed: entity `probe` is at"),
        "{failure}"
    );
}

#[test]
fn assert_entity_exists_rejects_duplicate_names() {
    let mut app = app_with_script("assert.entity_exists twin\n");
    app.world_mut().spawn(Name::new("twin"));
    app.world_mut().spawn(Name::new("twin"));
    run_until_exit(&mut app);
    let script = app.world().resource::<CommandScript>();
    let failure = script.failure().expect("duplicate names must fail");
    assert!(failure.contains("2 entities are named `twin`"), "{failure}");
}

#[test]
fn simulation_rng_is_seeded_from_the_plugin() {
    let mut app = app_with_script("");
    let mut expected = SimulationRng::from_seed(9);
    let mut actual = app.world_mut().resource_mut::<SimulationRng>();
    assert_eq!(actual.0.next_u64(), expected.0.next_u64());
}

#[test]
fn pending_work_holds_the_next_line_until_it_finishes() {
    let mut app = app_with_script(
        "test.mark
test.start_work 3 succeed
test.mark
quit
",
    );
    let (exit, _) = run_until_exit(&mut app);
    assert_eq!(exit, AppExit::Success);
    // Tick 2 starts the work; it finishes at the end of tick 4; the next line runs on tick 5.
    assert_eq!(app.world().resource::<MarkedTicks>().0, vec![1, 5]);
}

#[test]
fn failed_pending_work_fails_the_line_that_started_it() {
    let mut app = app_with_script(
        "test.start_work 2 fail
test.mark
quit
",
    );
    let (exit, _) = run_until_exit(&mut app);
    assert_eq!(exit, AppExit::error());
    let script = app.world().resource::<CommandScript>();
    assert_eq!(script.failure(), Some("test.cmds:1: fake work failed"));
    assert!(
        app.world().resource::<MarkedTicks>().0.is_empty(),
        "no line runs after a failure"
    );
}

#[test]
fn pending_work_that_never_finishes_times_out() {
    const SHORT_TIMEOUT: Duration = Duration::from_millis(50);
    let script = CommandScript::from_text(
        "test.cmds",
        "test.start_work 0 succeed
quit
",
    )
    .with_pending_work_timeout(SHORT_TIMEOUT);
    let mut app = app_with_command_script(script);
    let started = Instant::now();
    let mut exit = None;
    for _ in 0..MAX_UPDATES {
        app.update();
        exit = app.should_exit();
        if exit.is_some() {
            break;
        }
        // The timeout is wall-clock time, so let real time pass between updates.
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(exit, Some(AppExit::error()));
    assert!(
        started.elapsed() >= SHORT_TIMEOUT,
        "it must wait the whole timeout first"
    );
    let script = app.world().resource::<CommandScript>();
    let failure = script.failure().expect("the timeout must fail the script");
    assert!(
        failure.starts_with("test.cmds:1: the work this line started did not finish within 50ms"),
        "{failure}"
    );
}
