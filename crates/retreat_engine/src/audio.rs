//! Audio playback, engine side: maps the contract's `SoundPlayback` onto Bevy's audio, in this
//! one file, and keeps the *logical* playing set that scripts assert on.
//!
//! A sound is an entity with `SoundPlayback` and a unique `Name`. `start_sound_playback` gives it
//! Bevy's `AudioPlayer` + `PlaybackSettings` (real sound, when an audio device exists) and a
//! `SoundState` (when it logically started, how long it lasts). `end_finished_sounds` despawns a
//! one-shot sound once its duration has passed on the *fixed* simulation clock, so whether a
//! sound is playing never depends on a sound card, on loading speed or on the platform. A
//! looping sound plays until stopped. Starting music stops the previous music.
//!
//! Commands: `audio.play <name> <path> <volume> <once|loop>` (effect; holds a script until the
//! file has loaded), `audio.music <name> <path> <volume>` (looping music), `audio.stop <name>`,
//! `audio.stop_all`, `audio.volume <name> <volume>`, `audio.status`,
//! `assert.audio_playing <name>`, `assert.audio_not_playing <name>`,
//! `assert.audio_volume <name> <volume>`.
//!
//! Browsers keep audio silent until a user gesture (`platform::AUDIO_NEEDS_USER_GESTURE`);
//! `web/index.html` resumes the page's audio contexts on the first click or key press.

use std::time::Duration;

use bevy::asset::LoadState;
use bevy::audio::{
    AudioLoader, AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, Decodable, PlaybackMode,
    PlaybackSettings, Source, Volume,
};
use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, SoundKind,
    SoundPlayback,
};

use crate::PendingCommandWork;
use crate::asset_paths::{load_asset, resolve_asset_location};
use crate::command_script::fail_running_script;
use crate::entity_names::{ensure_name_is_free, find_unique_entity_by_name};
use crate::platform::{AUDIO_NEEDS_USER_GESTURE, HAS_FILE_SYSTEM};

/// File endings `SoundPlayback::path` may have.
pub const SOUND_FILE_EXTENSIONS: [&str; 2] = ["wav", "ogg"];

/// How far `assert.audio_volume` lets the volume differ from the expected one.
const VOLUME_TOLERANCE: f32 = 1.0e-4;

/// Adds the audio mapping, the logical clock and the `audio.*` commands. It does not add Bevy's
/// `AudioPlugin` (`DefaultPlugins` does); tests add only the asset type and loader.
pub struct AudioCommandsPlugin;

impl Plugin for AudioCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.register_command(
            "audio.play",
            "audio.play <name> <path> <volume> <once|loop>",
            play_command,
        )
        .register_command(
            "audio.music",
            "audio.music <name> <path> <volume>",
            music_command,
        )
        .register_command("audio.stop", "audio.stop <name>", stop_command)
        .register_command("audio.stop_all", "audio.stop_all", stop_all_command)
        .register_command(
            "audio.volume",
            "audio.volume <name> <volume>",
            volume_command,
        )
        .register_command("audio.status", "audio.status", status_command)
        .register_command(
            "assert.audio_playing",
            "assert.audio_playing <name>",
            assert_playing_command,
        )
        .register_command(
            "assert.audio_not_playing",
            "assert.audio_not_playing <name>",
            assert_not_playing_command,
        )
        .register_command(
            "assert.audio_volume",
            "assert.audio_volume <name> <volume>",
            assert_volume_command,
        )
        .add_systems(
            Update,
            (
                start_sound_playback,
                measure_loaded_sounds,
                apply_volume_changes,
            )
                .chain(),
        )
        .add_systems(FixedUpdate, end_finished_sounds);
    }
}

/// How long a sound lasts, once known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SoundLength {
    /// The file has not loaded yet.
    Loading,
    /// The file says how long it is.
    Known(Duration),
    /// The file loaded but does not say (some streams): the sound plays until stopped.
    Unknown,
}

/// Engine bookkeeping on a sound entity.
#[derive(Component, Debug)]
struct SoundState {
    handle: Handle<AudioSource>,
    /// `Time<Fixed>` elapsed when the sound started: the logical clock.
    started_at: Duration,
    length: SoundLength,
}

/// Marks a sound that an `audio.play` command is waiting for: the script holds until its file
/// has loaded (or failed).
#[derive(Component)]
struct HoldsScript;

/// True when a sound that started at `started_at` is over at `now`. Looping sounds and sounds
/// of unknown length never end by themselves. Pure, so it is tested without an app.
fn sound_has_ended(
    started_at: Duration,
    now: Duration,
    length: SoundLength,
    looping: bool,
) -> bool {
    match length {
        SoundLength::Known(length) => !looping && now.saturating_sub(started_at) >= length,
        SoundLength::Loading | SoundLength::Unknown => false,
    }
}

/// A `SoundPlayback` added since the last run that the engine has not started yet.
type NewSound = (Added<SoundPlayback>, Without<SoundState>);

/// Starts every newly added `SoundPlayback`: validates it, stops the previous music when this is
/// music, and begins loading and playing. A bad request fails the running script.
fn start_sound_playback(
    mut commands: Commands,
    new_sounds: Query<(Entity, &Name, &SoundPlayback), NewSound>,
    playing: Query<(Entity, &SoundPlayback), With<SoundState>>,
    asset_server: Res<AssetServer>,
    fixed_time: Res<Time<Fixed>>,
) {
    for (entity, name, sound) in &new_sounds {
        let checked = sound.validate().and_then(|()| {
            resolve_asset_location(
                &sound.path,
                "sound",
                &SOUND_FILE_EXTENSIONS,
                HAS_FILE_SYSTEM,
            )
        });
        let location = match checked {
            Ok(location) => location,
            Err(message) => {
                let message = format!("sound `{name}`: {message}");
                error!("{message}");
                commands.queue(move |world: &mut World| fail_running_script(world, message));
                commands.entity(entity).despawn();
                continue;
            }
        };
        if sound.kind == SoundKind::Music {
            for (other, other_sound) in &playing {
                if other_sound.kind == SoundKind::Music {
                    commands.entity(other).despawn();
                }
            }
        }
        let is_outside = location.is_outside_asset_folder();
        let handle: Handle<AudioSource> =
            load_asset(&asset_server, location.into_asset_path(), is_outside);
        commands.entity(entity).insert((
            AudioPlayer::new(handle.clone()),
            playback_settings(sound),
            SoundState {
                handle,
                started_at: fixed_time.elapsed(),
                length: SoundLength::Loading,
            },
        ));
    }
}

/// Bevy's playback settings for a request.
fn playback_settings(sound: &SoundPlayback) -> PlaybackSettings {
    PlaybackSettings {
        mode: if sound.looping {
            PlaybackMode::Loop
        } else {
            PlaybackMode::Once
        },
        volume: Volume::Linear(sound.volume),
        ..default()
    }
}

/// Once a sound's file has loaded, reads how long it is; finishes (or fails) the pending work of
/// an `audio.play` command waiting for it.
fn measure_loaded_sounds(
    mut commands: Commands,
    mut sounds: Query<(Entity, &Name, &mut SoundState, Has<HoldsScript>)>,
    sources: Res<Assets<AudioSource>>,
    asset_server: Res<AssetServer>,
    mut pending: ResMut<PendingCommandWork>,
) {
    for (entity, name, mut state, holds_script) in &mut sounds {
        if state.length != SoundLength::Loading {
            continue;
        }
        if let Some(source) = sources.get(&state.handle) {
            state.length = match source.decoder().total_duration() {
                Some(length) => SoundLength::Known(length),
                None => SoundLength::Unknown,
            };
            if holds_script {
                commands.entity(entity).remove::<HoldsScript>();
                pending.finish();
            }
        } else if let Some(LoadState::Failed(error)) = asset_server.get_load_state(&state.handle) {
            let message = format!(
                "sound `{name}` could not load its file (relative paths start in the app's \
                 assets folder): {error}"
            );
            if holds_script {
                pending.fail(message);
            } else {
                error!("{message}");
                commands.queue(move |world: &mut World| fail_running_script(world, message));
            }
            commands.entity(entity).despawn();
        }
    }
}

/// Applies a changed `volume` to a sound that is already playing.
fn apply_volume_changes(
    mut sounds: Query<(&SoundPlayback, &mut AudioSink), Changed<SoundPlayback>>,
) {
    for (sound, mut sink) in &mut sounds {
        sink.set_volume(Volume::Linear(sound.volume));
    }
}

/// Ends one-shot sounds whose duration has passed on the fixed clock.
fn end_finished_sounds(
    mut commands: Commands,
    sounds: Query<(Entity, &SoundPlayback, &SoundState)>,
    fixed_time: Res<Time<Fixed>>,
) {
    for (entity, sound, state) in &sounds {
        if sound_has_ended(
            state.started_at,
            fixed_time.elapsed(),
            state.length,
            sound.looping,
        ) {
            commands.entity(entity).despawn();
        }
    }
}

/// Parses a volume argument and checks it with the contract's rule.
fn parse_volume(args: &CommandArgs, index: usize) -> Result<f32, CommandError> {
    let volume: f32 = args.parse(index, "volume")?;
    let probe = SoundPlayback {
        volume,
        ..SoundPlayback::effect("x.wav")
    };
    probe.validate().map_err(|message| args.failure(message))?;
    Ok(volume)
}

/// Spawns a sound entity named `name` and makes the script wait for its file.
fn spawn_sound(
    world: &mut World,
    args: &CommandArgs,
    name: String,
    sound: SoundPlayback,
) -> CommandResult {
    ensure_name_is_free(world, args, &name)?;
    sound.validate().map_err(|message| args.failure(message))?;
    resolve_asset_location(
        &sound.path,
        "sound",
        &SOUND_FILE_EXTENSIONS,
        HAS_FILE_SYSTEM,
    )
    .map_err(|message| args.failure(message))?;
    world.resource_mut::<PendingCommandWork>().start();
    world.spawn((Name::new(name), sound, HoldsScript));
    Ok(CommandOutcome::Continue)
}

/// `audio.play <name> <path> <volume> <once|loop>`: plays an effect.
fn play_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let name: String = args.parse(0, "name")?;
    let path: String = args.parse(1, "path")?;
    let volume = parse_volume(args, 2)?;
    let mode: String = args.parse(3, "once|loop")?;
    let looping = match mode.as_str() {
        "once" => false,
        "loop" => true,
        other => {
            return Err(args.failure(format!(
                "<once|loop> must be `once` or `loop`, got `{other}`"
            )));
        }
    };
    let sound = SoundPlayback {
        volume,
        looping,
        ..SoundPlayback::effect(path)
    };
    spawn_sound(world, args, name, sound)
}

/// `audio.music <name> <path> <volume>`: plays looping music, replacing the current music.
fn music_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let path: String = args.parse(1, "path")?;
    let volume = parse_volume(args, 2)?;
    let sound = SoundPlayback {
        volume,
        ..SoundPlayback::music(path)
    };
    spawn_sound(world, args, name, sound)
}

/// The entity named by argument 0 if it is a sound, otherwise an error.
fn sound_entity(world: &mut World, args: &CommandArgs) -> Result<(String, Entity), CommandError> {
    let name: String = args.parse(0, "name")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    if world.get::<SoundPlayback>(entity).is_none() {
        return Err(args.failure(format!(
            "`{name}` is not playing a sound (no entity with that name has a SoundPlayback); \
             run `audio.status` to list the sounds"
        )));
    }
    Ok((name, entity))
}

/// `audio.stop <name>`.
fn stop_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let (_, entity) = sound_entity(world, args)?;
    world.despawn(entity);
    Ok(CommandOutcome::Continue)
}

/// `audio.stop_all`.
fn stop_all_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let mut sounds = world.query_filtered::<Entity, With<SoundPlayback>>();
    let all: Vec<Entity> = sounds.iter(world).collect();
    for entity in all {
        world.despawn(entity);
    }
    Ok(CommandOutcome::Continue)
}

/// `audio.volume <name> <volume>`.
fn volume_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let volume = parse_volume(args, 1)?;
    let (_, entity) = sound_entity(world, args)?;
    if let Some(mut sound) = world.get_mut::<SoundPlayback>(entity) {
        sound.volume = volume;
    }
    Ok(CommandOutcome::Continue)
}

/// `audio.status`: logs the platform's audio note and every playing sound.
fn status_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    if AUDIO_NEEDS_USER_GESTURE {
        info!(
            "audio: the browser keeps sound silent until the first click or key press on the page"
        );
    } else {
        info!("audio: native output");
    }
    let mut sounds = world.query::<(&Name, &SoundPlayback)>();
    let mut lines: Vec<String> = sounds
        .iter(world)
        .map(|(name, sound)| {
            format!(
                "sound `{name}`: {} ({:?}, volume {}, {})",
                sound.path,
                sound.kind,
                sound.volume,
                if sound.looping { "loop" } else { "once" }
            )
        })
        .collect();
    lines.sort();
    if lines.is_empty() {
        info!("audio: no sounds playing");
    }
    for line in lines {
        info!("{line}");
    }
    Ok(CommandOutcome::Continue)
}

/// `assert.audio_playing <name>`: fails unless the sound is logically playing.
fn assert_playing_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    if is_playing(world, &name) {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "no sound named `{name}` is playing (it may have finished, or never started); \
             sounds playing: {}",
            playing_names(world)
        )))
    }
}

/// `assert.audio_not_playing <name>`: fails if the sound is logically playing.
fn assert_not_playing_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let name: String = args.parse(0, "name")?;
    if is_playing(world, &name) {
        Err(args.failure(format!("sound `{name}` is still playing")))
    } else {
        Ok(CommandOutcome::Continue)
    }
}

/// `assert.audio_volume <name> <volume>`: fails unless the playing sound has this volume.
fn assert_volume_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let expected: f32 = args.parse(1, "volume")?;
    let (name, entity) = sound_entity(world, args)?;
    let actual = world
        .get::<SoundPlayback>(entity)
        .map(|sound| sound.volume)
        .unwrap_or_default();
    if (actual - expected).abs() <= VOLUME_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "sound `{name}` has volume {actual}, expected {expected} (tolerance {VOLUME_TOLERANCE})"
        )))
    }
}

/// Whether an entity named `name` has a `SoundPlayback`.
fn is_playing(world: &mut World, name: &str) -> bool {
    let mut sounds = world.query_filtered::<&Name, With<SoundPlayback>>();
    sounds
        .iter(world)
        .any(|sound_name| sound_name.as_str() == name)
}

/// Sorted names of every playing sound, or `(none)`, for error messages.
fn playing_names(world: &mut World) -> String {
    let mut sounds = world.query_filtered::<&Name, With<SoundPlayback>>();
    let mut names: Vec<String> = sounds.iter(world).map(|name| name.to_string()).collect();
    names.sort();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Registers what loading a sound needs when Bevy's `AudioPlugin` is absent (tests): the asset
/// type and its loader. Real apps get both from `DefaultPlugins`.
pub fn add_sound_assets_without_audio_device(app: &mut App) {
    app.init_asset::<AudioSource>()
        .init_asset_loader::<AudioLoader>();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn a_one_shot_ends_when_its_length_has_passed() {
        let length = SoundLength::Known(SECOND);
        assert!(!sound_has_ended(
            Duration::ZERO,
            Duration::from_millis(999),
            length,
            false
        ));
        assert!(sound_has_ended(Duration::ZERO, SECOND, length, false));
        assert!(sound_has_ended(SECOND, SECOND * 3, length, false));
    }

    #[test]
    fn looping_loading_and_unknown_sounds_never_end_by_themselves() {
        let long_after = SECOND * 1000;
        let known = SoundLength::Known(SECOND);
        assert!(!sound_has_ended(Duration::ZERO, long_after, known, true));
        assert!(!sound_has_ended(
            Duration::ZERO,
            long_after,
            SoundLength::Loading,
            false
        ));
        assert!(!sound_has_ended(
            Duration::ZERO,
            long_after,
            SoundLength::Unknown,
            false
        ));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_end_the_sound() {
        let length = SoundLength::Known(SECOND);
        assert!(!sound_has_ended(SECOND, Duration::ZERO, length, false));
    }
}
