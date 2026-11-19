//! Saving commands in an app without a window: scripts write, read back and delete documents in
//! memory, errors are clear, and a module-style save/load works through the contract. The real
//! platform backends are checked by `scripts/verify/storage.cmds` (files natively, `localStorage`
//! in the browser) and by the file backend's tests.

use bevy::prelude::*;
use retreat_engine::{CommandScript, RetreatCommandsPlugin, StorageCommandsPlugin};
use retreat_engine_api::SaveStore;
use retreat_engine_api::serde_json::json;

const MAX_UPDATES: u32 = 200;

fn run_script(text: &str) -> (App, Option<String>) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(RetreatCommandsPlugin {
            seed: 0,
            command_script: Some(CommandScript::from_text("test.cmds", text)),
        })
        .add_plugins(StorageCommandsPlugin);
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from);
    (app, failure)
}

fn failure_of(text: &str) -> String {
    run_script(text).1.expect("the script must fail")
}

#[test]
fn a_script_writes_reads_and_deletes_a_save() {
    let (app, failure) = run_script(
        "save.write score 2 {\"score\":120,\"name\":\"ada\"}\nassert.save_version score 2\n\
         assert.save_field score score 120\nassert.save_field score name \"ada\"\nsave.read score\n\
         save.list\nsave.status\nsave.delete score\nassert.save_absent score\nsave.delete score\nquit\n",
    );
    assert_eq!(failure, None);
    assert_eq!(
        app.world().resource::<SaveStore>().modules().unwrap().len(),
        0
    );
}

#[test]
fn a_module_can_save_and_load_through_the_contract() {
    let (app, failure) = run_script("save.write m 1 {\"n\":5}\nquit\n");
    assert_eq!(failure, None);
    let loaded = app
        .world()
        .resource::<SaveStore>()
        .load("m")
        .unwrap()
        .expect("the script saved it");
    assert_eq!((loaded.version, loaded.data), (1, json!({"n": 5})));
}

#[test]
fn a_script_run_starts_with_no_saves() {
    let (app, _) = run_script("quit\n");
    assert!(
        app.world()
            .resource::<SaveStore>()
            .modules()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn errors_name_the_command_the_bad_value_and_the_fix() {
    let failure = failure_of("save.write score 1 {oops}\nquit\n");
    assert!(failure.contains("`{oops}` is not valid JSON"), "{failure}");
    let failure = failure_of("save.write bad/id 1 {}\nquit\n");
    assert!(
        failure.contains("`bad/id` is not a valid module id"),
        "{failure}"
    );
    let failure = failure_of("save.read nothing\nquit\n");
    assert!(
        failure.contains("module `nothing` has no save; write one with `save.write nothing"),
        "{failure}"
    );
    let failure = failure_of("save.write score x {}\nquit\n");
    assert!(
        failure.contains("usage: save.write <module> <version> <json>"),
        "{failure}"
    );
    let failure = failure_of("save.write s 1 {\"a\":1}\nassert.save_version s 2\nquit\n");
    assert!(failure.contains("is version 1, expected 2"), "{failure}");
    let failure = failure_of("save.write s 1 {\"a\":1}\nassert.save_field s a 2\nquit\n");
    assert!(
        failure.contains("field `a` of the save of `s` is 1, expected 2"),
        "{failure}"
    );
    let failure = failure_of("save.write s 1 {\"a\":1}\nassert.save_field s b 1\nquit\n");
    assert!(
        failure.contains("has no field `b`; its data is {\"a\":1}"),
        "{failure}"
    );
    let failure = failure_of("save.write s 1 {}\nassert.save_absent s\nquit\n");
    assert!(failure.contains("module `s` still has a save"), "{failure}");
}
