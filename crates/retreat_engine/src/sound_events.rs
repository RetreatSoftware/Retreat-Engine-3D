//! Plays the contract's one-shot sounds (`sound_events.rs`): each id once, as a `SoundPlayback`
//! entity, and makes sounds with a `SoundPosition` quieter with distance and come from their side.
//!
//! Distance is this engine's volume factor (`spatial_mix`); direction uses Bevy's spatial output
//! with the source moved to one unit from the camera, so the audio library's own distance rule has no
//! effect. **The ears are swapped on purpose:** rodio 0.22 gives the channel of the *farther* ear the
//! louder share, so the listener's "left ear" sits on the camera's right. With a tiny ear gap a sound
//! straight to one side plays at about 0.98 there and 0.5 on the other side, and 0.75 on both when
//! ahead.
//!
//! Commands: `audio.event <id> <path> <volume> <x> <y> <z> <reach>`,
//! `assert.audio_events <count>`, `assert.audio_mix <name> <gain> <pan>` (within 0.02).

use bevy::app::FixedLast;
use bevy::audio::SpatialListener;
use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, EffectTime, GameCamera,
    NetReplaying, NetViewTick, SoundEvent, SoundEvents, SoundListener, SoundMix, SoundPlayback,
    SoundPosition, spatial_mix,
};

use crate::net::client::NetClient;
use crate::once_queue::OnceQueue;

/// Start of the name of an event's sound entity.
pub const SOUND_EVENT_NAME_PREFIX: &str = "sound/";
/// Distance between the listener's ears (see the module docs).
const EAR_GAP: f32 = 0.02;
const MIX_TOLERANCE: f32 = 0.02;

/// What this machine knows about sound events.
#[derive(Resource, Default)]
pub struct SoundEventLog {
    queue: OnceQueue<SoundEvent>,
    /// Events heard so far.
    pub played: u64,
}

pub struct SoundEventsPlugin;

impl Plugin for SoundEventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoundEvents>()
            .init_resource::<SoundEventLog>()
            .init_resource::<NetViewTick>()
            .init_resource::<NetReplaying>()
            // Before the client counts the tick, as for effects.
            .add_systems(FixedPostUpdate, take_requests)
            .add_systems(FixedLast, start_due_sounds)
            .add_systems(Update, attach_listener)
            .register_command(
                "audio.event",
                "audio.event <id> <path> <volume> <x> <y> <z> <reach>",
                event_command,
            )
            .register_command(
                "assert.audio_events",
                "assert.audio_events <count>",
                assert_events_command,
            )
            .register_command(
                "assert.audio_mix",
                "assert.audio_mix <name> <gain> <pan>",
                assert_mix_command,
            );
    }
}

fn take_requests(
    client: Option<Res<NetClient>>,
    mut requests: ResMut<SoundEvents>,
    mut log: ResMut<SoundEventLog>,
) {
    if requests.requests.is_empty() {
        return;
    }
    let tick = client.map_or(0, |client| client.tick + 1);
    for event in std::mem::take(&mut requests.requests) {
        let (id, time) = (event.id.clone(), event.time);
        log.queue.add(&id, time, tick, event);
    }
}

fn start_due_sounds(
    replaying: Res<NetReplaying>,
    view: Res<NetViewTick>,
    mut log: ResMut<SoundEventLog>,
    mut commands: Commands,
) {
    if !replaying.0 {
        log.queue.live_tick();
    }
    for event in log.queue.take_due(view.0) {
        log.played += 1;
        let mut sound = commands.spawn((
            Name::new(format!("{SOUND_EVENT_NAME_PREFIX}{}", event.id)),
            SoundPlayback {
                volume: event.volume,
                ..SoundPlayback::effect(event.path)
            },
        ));
        if let Some(position) = event.at {
            sound.insert(position);
        }
    }
}

/// The camera that listens: the first `GameCamera` by name.
fn listening_camera<'a>(
    cameras: impl Iterator<Item = (Entity, &'a Name, &'a GameCamera)>,
) -> Option<(Entity, SoundListener)> {
    cameras
        .min_by(|left, right| left.1.as_str().cmp(right.1.as_str()))
        .map(|(entity, _, camera)| {
            let forward = (camera.target - camera.position).normalize_or(Vec3::NEG_Z);
            let listener = SoundListener {
                position: camera.position,
                right: forward.cross(camera.up).normalize_or(Vec3::X),
            };
            (entity, listener)
        })
}

/// The listener now (`None` without a camera).
pub(crate) fn current_listener(
    cameras: &Query<(Entity, &Name, &GameCamera)>,
) -> Option<SoundListener> {
    listening_camera(cameras.iter()).map(|(_, listener)| listener)
}

/// How a sound at `source` reaches `listener`; without a listener it is heard in full, centered.
pub(crate) fn mix_for(listener: Option<&SoundListener>, source: &SoundPosition) -> SoundMix {
    listener.map_or(
        SoundMix {
            gain: 1.0,
            pan: 0.0,
        },
        |listener| spatial_mix(listener, source),
    )
}

/// Where the audio library is told the source is: one unit from the camera, towards the source.
pub(crate) fn emitter_position(listener: Option<&SoundListener>, source: &SoundPosition) -> Vec3 {
    listener.map_or(source.position, |listener| {
        let towards = (source.position - listener.position)
            // At the listener: neither side.
            .normalize_or(listener.right.any_orthonormal_vector());
        listener.position + towards
    })
}

/// Gives the listening camera its ears.
fn attach_listener(
    cameras: Query<(Entity, &Name, &GameCamera)>,
    listeners: Query<Entity, With<SpatialListener>>,
    mut commands: Commands,
) {
    let wanted = listening_camera(cameras.iter()).map(|(entity, _)| entity);
    for entity in &listeners {
        if Some(entity) != wanted {
            commands.entity(entity).remove::<SpatialListener>();
        }
    }
    if let Some(entity) = wanted
        && !listeners.contains(entity)
    {
        commands.entity(entity).insert(SpatialListener {
            // Swapped: see the module docs.
            left_ear_offset: Vec3::X * (EAR_GAP / 2.0),
            right_ear_offset: Vec3::X * (-EAR_GAP / 2.0),
        });
    }
}

fn event_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(7)?;
    let number = |index, label| args.parse::<f32>(index, label);
    let at = SoundPosition {
        position: Vec3::new(number(3, "x")?, number(4, "y")?, number(5, "z")?),
        reach: number(6, "reach")?,
    };
    let event = SoundEvent {
        id: args.parse(0, "id")?,
        path: args.parse(1, "path")?,
        volume: number(2, "volume")?,
        at: Some(at),
        time: EffectTime::Present,
    };
    let sound = SoundPlayback {
        volume: event.volume,
        ..SoundPlayback::effect(event.path.clone())
    };
    sound
        .validate()
        .and_then(|()| at.validate())
        .map_err(|message| args.failure(message))?;
    world.resource_mut::<SoundEvents>().play(event);
    Ok(CommandOutcome::Continue)
}

fn assert_events_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected: u64 = args.parse(0, "count")?;
    let played = world.resource::<SoundEventLog>().played;
    if played == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "{played} sound events were played, expected {expected}"
        )))
    }
}

fn assert_mix_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let name: String = args.parse(0, "name")?;
    let gain: f32 = args.parse(1, "gain")?;
    let pan: f32 = args.parse(2, "pan")?;
    let source = world
        .query::<(&Name, &SoundPosition)>()
        .iter(world)
        .find(|(found, _)| found.as_str() == name)
        .map(|(_, source)| *source)
        .ok_or_else(|| args.failure(format!("no positional sound `{name}` is playing")))?;
    let mut cameras = world.query::<(Entity, &Name, &GameCamera)>();
    let listener = listening_camera(cameras.iter(world)).map(|(_, listener)| listener);
    let mix = mix_for(listener.as_ref(), &source);
    if (mix.gain - gain).abs() <= MIX_TOLERANCE && (mix.pan - pan).abs() <= MIX_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "`{name}` has gain {:.3} and pan {:.3}, expected {gain} and {pan}",
            mix.gain, mix.pan
        )))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use rodio::source::Spatial;

    /// Loudness of the (left, right) speaker for a source at `source`, heard through the audio
    /// library with the ears and emitter this module gives it.
    fn speakers(source: Vec3) -> (f32, f32) {
        let listener = SoundListener {
            position: Vec3::new(3.0, 2.0, 1.0),
            right: Vec3::X,
        };
        let at = SoundPosition {
            position: listener.position + source,
            reach: 50.0,
        };
        let ear = |offset: f32| (listener.position + Vec3::X * offset).to_array();
        let one = SamplesBuffer::new(
            std::num::NonZero::new(1).unwrap(),
            std::num::NonZero::new(44_100).unwrap(),
            vec![1.0; 4],
        );
        let mut out = Spatial::new(
            one,
            emitter_position(Some(&listener), &at).to_array(),
            ear(EAR_GAP / 2.0),
            ear(-EAR_GAP / 2.0),
        );
        (out.next().unwrap(), out.next().unwrap())
    }

    #[test]
    fn a_source_to_one_side_is_louder_on_that_side_however_far() {
        for distance in [0.3, 5.0, 40.0] {
            let (left, right) = speakers(Vec3::X * distance);
            assert!(
                (right - 0.98).abs() < 0.02 && (left - 0.5).abs() < 0.02,
                "{left} {right}"
            );
            let (left, right) = speakers(Vec3::NEG_X * distance);
            assert!(
                (left - 0.98).abs() < 0.02 && (right - 0.5).abs() < 0.02,
                "{left} {right}"
            );
            let (left, right) = speakers(Vec3::NEG_Z * distance);
            assert!((left - right).abs() < 1.0e-3, "{left} {right}");
            assert!((left - 0.75).abs() < 0.02, "{left}");
        }
        let (left, right) = speakers(Vec3::ZERO);
        assert!((left - right).abs() < 1.0e-3, "{left} {right}");
    }
}
