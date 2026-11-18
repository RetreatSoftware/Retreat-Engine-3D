//! Audio commands in an app without a window, a GPU or a sound card: the logical playing set
//! follows the fixed simulation clock, scripts assert on it, and mistakes give clear errors.
//! The real sandbox check is `scripts/verify/audio.cmds`.

use bevy::prelude::*;
use retreat_engine::{
    AudioCommandsPlugin, CommandScript, RetreatCommandsPlugin,
    add_sound_assets_without_audio_device,
};
use retreat_engine_api::{SoundKind, SoundPlayback};

const MAX_UPDATES: u32 = 2000;

/// The sandbox's assets folder, where `sounds/beep.wav` (0.25 s) and `sounds/tone.wav` (1 s) live.
const SANDBOX_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sandbox/assets");

fn test_app(text: &str) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: SANDBOX_ASSETS.to_string(),
            ..default()
        },
    ))
    .add_plugins(RetreatCommandsPlugin {
        seed: 0,
        command_script: Some(CommandScript::from_text("test.cmds", text)),
    })
    .add_plugins(AudioCommandsPlugin);
    add_sound_assets_without_audio_device(&mut app);
    app
}

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

fn failure_of(text: &str) -> String {
    run_script(text).1.expect("the script must fail")
}

#[test]
fn an_effect_plays_for_exactly_its_length_on_the_fixed_clock() {
    // beep.wav lasts 0.25 s = 16 fixed ticks.
    let (_, failure) = run_script(
        "audio.play click sounds/beep.wav 1 once\nassert.audio_playing click\nwait 8\n\
         assert.audio_playing click\nwait 16\nassert.audio_not_playing click\nquit\n",
    );
    assert_eq!(failure, None);
}

#[test]
fn a_looping_sound_plays_until_it_is_stopped() {
    let (_, failure) = run_script(
        "audio.play hum sounds/beep.wav 0.5 loop\nwait 200\nassert.audio_playing hum\n\
         audio.stop hum\nassert.audio_not_playing hum\nquit\n",
    );
    assert_eq!(failure, None);
}

#[test]
fn starting_music_stops_the_previous_music_but_not_effects() {
    let (mut app, failure) = run_script(
        "audio.music first sounds/tone.wav 1\naudio.play ping sounds/tone.wav 1 loop\nwait 2\n\
         audio.music second sounds/beep.wav 1\nwait 2\n\
         assert.audio_not_playing first\nassert.audio_playing second\nassert.audio_playing ping\nquit\n",
    );
    assert_eq!(failure, None);
    let world = app.world_mut();
    let kinds: Vec<SoundKind> = world
        .query::<&SoundPlayback>()
        .iter(world)
        .map(|sound| sound.kind)
        .collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == SoundKind::Music)
            .count(),
        1
    );
}

#[test]
fn volume_changes_are_visible_to_assertions() {
    let (_, failure) = run_script(
        "audio.play hum sounds/tone.wav 1 loop\naudio.volume hum 0.25\nassert.audio_volume hum 0.25\nquit\n",
    );
    assert_eq!(failure, None);
    let failure =
        failure_of("audio.play hum sounds/tone.wav 1 loop\nassert.audio_volume hum 0.5\nquit\n");
    assert!(
        failure.contains("sound `hum` has volume 1, expected 0.5"),
        "{failure}"
    );
}

#[test]
fn stop_all_stops_every_sound() {
    let (_, failure) = run_script(
        "audio.play a sounds/tone.wav 1 loop\naudio.music b sounds/tone.wav 1\naudio.stop_all\n\
         assert.audio_not_playing a\nassert.audio_not_playing b\nquit\n",
    );
    assert_eq!(failure, None);
}

#[test]
fn a_module_can_play_a_sound_by_spawning_the_contract_component() {
    let mut app = test_app("wait 40\nassert.audio_playing from_module\nquit\n");
    app.world_mut().spawn((
        Name::new("from_module"),
        SoundPlayback::music("sounds/tone.wav"),
    ));
    for _ in 0..MAX_UPDATES {
        app.update();
        if app.should_exit().is_some() {
            break;
        }
    }
    assert_eq!(
        app.world().resource::<CommandScript>().failure(),
        None,
        "a spawned SoundPlayback must count as playing"
    );
}

#[test]
fn errors_name_the_command_the_bad_value_and_the_fix() {
    let failure = failure_of("audio.play x sounds/missing.wav 1 once\nquit\n");
    assert!(
        failure.starts_with("test.cmds:1: sound `x` could not load its file"),
        "{failure}"
    );
    let failure = failure_of("audio.play x sounds/beep.mp3 1 once\nquit\n");
    assert!(
        failure.contains("is not a sound; the file name must end in .wav or .ogg"),
        "{failure}"
    );
    let failure = failure_of("audio.play x ../beep.wav 1 once\nquit\n");
    assert!(failure.contains("leaves the assets folder"), "{failure}");
    let failure = failure_of("audio.play x sounds/beep.wav 3 once\nquit\n");
    assert!(
        failure.contains("volume must be from 0 to 2, got 3"),
        "{failure}"
    );
    let failure = failure_of("audio.play x sounds/beep.wav 1 twice\nquit\n");
    assert!(
        failure.contains("<once|loop> must be `once` or `loop`, got `twice`"),
        "{failure}"
    );
    let failure = failure_of("audio.play x sounds/beep.wav 1\nquit\n");
    assert!(
        failure.contains("usage: audio.play <name> <path> <volume> <once|loop>"),
        "{failure}"
    );
    let failure = failure_of("audio.stop nothing\nquit\n");
    assert!(
        failure.contains("no entity is named `nothing`"),
        "{failure}"
    );
    let failure = failure_of("assert.audio_playing nothing\nquit\n");
    assert!(
        failure.contains("no sound named `nothing` is playing"),
        "{failure}"
    );
    assert!(failure.contains("sounds playing: (none)"), "{failure}");
}

#[test]
fn a_name_that_is_already_used_is_rejected() {
    let failure = failure_of(
        "audio.play twin sounds/tone.wav 1 loop\naudio.play twin sounds/tone.wav 1 loop\nquit\n",
    );
    assert!(
        failure.contains("an entity is already named `twin`"),
        "{failure}"
    );
}

#[test]
fn a_non_sound_entity_cannot_be_stopped() {
    let mut app = test_app("audio.stop plain\nquit\n");
    app.world_mut().spawn(Name::new("plain"));
    for _ in 0..20 {
        app.update();
    }
    let failure = app
        .world()
        .resource::<CommandScript>()
        .failure()
        .map(String::from)
        .expect("stopping a non-sound must fail");
    assert!(
        failure.contains("`plain` is not playing a sound"),
        "{failure}"
    );
}
