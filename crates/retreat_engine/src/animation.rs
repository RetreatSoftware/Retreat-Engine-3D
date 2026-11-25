//! Animation playback, engine side: maps the contract's `AnimationPlayback` onto Bevy's
//! `AnimationPlayer` + `AnimationGraph`, in this one file.
//!
//! Each model (an entity with `ModelGltf`) gets an `AnimationRig` once its glTF file and scene
//! are loaded: a graph with one node per clip of the file (clips sorted by name, so node order
//! never depends on hash order) attached to the scene's `AnimationPlayer`.
//!
//! **Time is ours, not Bevy's.** Every clip keeps its own time in the rig. `advance_clip_times`
//! (`FixedUpdate`) adds `fixed tick * speed`; `apply_animation_poses` (`Update`) writes the pose
//! time into Bevy's `ActiveAnimation` with speed 0, so Bevy's own real-time advance never moves
//! it. Fixed time is paused while a script waits (see `command_script.rs`), so a pose depends only
//! on the tick: same picture on every machine. In interactive play the pose adds the fixed
//! clock's `overstep` (the part of a tick already elapsed) so motion is smooth between ticks;
//! under a script the overstep is zero.
//!
//! Commands (model found by its unique `Name`): `animation.list <model>`,
//! `animation.play <model> <clip> <once|loop> <speed>`, `animation.blend <model> <clipA> <clipB>
//! <weightB>`, `animation.stop <model>`, `assert.animation_playing <model> <clip>`,
//! `assert.animation_time <model> <clip> <seconds>`, `assert.animation_weight <model> <clip> <w>`.

use std::collections::BTreeMap;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::{AnimationClip, AnimationPlayer};
use bevy::gltf::Gltf;
use bevy::prelude::*;
use retreat_engine_api::{
    AnimationPlayback, AppCommandExt, ClipPlayback, CommandArgs, CommandError, CommandOutcome,
    CommandResult, NetReplaying,
};

use crate::CommandScriptSystems;
use crate::command_script::fail_running_script;
use crate::entity_names::find_unique_entity_by_name;
use crate::model::ModelGltf;

/// How far `assert.animation_time` lets the time differ from the expected one, in seconds.
const TIME_TOLERANCE: f32 = 1.0e-3;
/// A clip that does not loop holds this far before its end (Bevy wraps a seek time equal to the
/// duration back to zero).
const END_HOLD_MARGIN: f32 = 1.0e-4;

/// Adds the animation mapping and the `animation.*` commands.
pub struct AnimationCommandsPlugin;

impl Plugin for AnimationCommandsPlugin {
    fn build(&self, app: &mut App) {
        app.register_command("animation.list", "animation.list <model>", list_command)
            .register_command(
                "animation.play",
                "animation.play <model> <clip> <once|loop> <speed>",
                play_command,
            )
            .register_command(
                "animation.blend",
                "animation.blend <model> <clipA> <clipB> <weightB>",
                blend_command,
            )
            .register_command("animation.stop", "animation.stop <model>", stop_command)
            .register_command(
                "assert.animation_playing",
                "assert.animation_playing <model> <clip>",
                assert_playing_command,
            )
            .register_command(
                "assert.animation_time",
                "assert.animation_time <model> <clip> <seconds>",
                assert_time_command,
            )
            .register_command(
                "assert.animation_weight",
                "assert.animation_weight <model> <clip> <weight>",
                assert_weight_command,
            )
            .add_systems(
                Update,
                (build_animation_rigs, apply_animation_poses).chain(),
            )
            .add_systems(
                FixedUpdate,
                // After the script line of the tick, so a clip started by a line already advances
                // in that tick on every platform (no ambiguous system order).
                advance_clip_times.after(CommandScriptSystems),
            );
    }
}

/// What the engine built for one model: the graph node of each clip and each clip's own time.
#[derive(Component, Debug)]
pub(crate) struct AnimationRig {
    /// The scene's `AnimationPlayer` entity.
    player: Entity,
    /// Clip name -> graph node.
    nodes: BTreeMap<String, AnimationNodeIndex>,
    /// Clip name -> length in seconds.
    durations: BTreeMap<String, f32>,
    /// Clip name -> time in seconds since the clip joined the playback list.
    times: BTreeMap<String, f32>,
}

impl AnimationRig {
    /// A rig from already known parts (tests build one without a glTF file).
    pub(crate) fn new(
        player: Entity,
        nodes: BTreeMap<String, AnimationNodeIndex>,
        durations: BTreeMap<String, f32>,
    ) -> Self {
        Self {
            player,
            nodes,
            durations,
            times: BTreeMap::new(),
        }
    }

    fn clip_names(&self) -> String {
        self.nodes.keys().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// The time to show for a clip whose own time is `time`: looping clips wrap around their length,
/// others hold just before the end. Pure, so it is tested without an app.
pub(crate) fn clip_pose_time(time: f32, duration: f32, looping: bool) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    if looping {
        time.rem_euclid(duration)
    } else {
        time.clamp(0.0, (duration - END_HOLD_MARGIN).max(0.0))
    }
}

/// True when a clip that does not loop has played to its end.
fn one_shot_finished(time: f32, duration: f32, looping: bool) -> bool {
    !looping && time >= duration
}

/// The entity under `root` (including it) that has an `AnimationPlayer`.
fn find_player(world: &World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if world.get::<AnimationPlayer>(entity).is_some() {
            return Some(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

/// Builds the rig of `model` if it has none yet. `Ok(())` when a rig exists afterwards; `Err`
/// says why not (the file is still loading, or it has no animations).
fn ensure_rig(world: &mut World, model: Entity) -> Result<(), String> {
    if world.get::<AnimationRig>(model).is_some() {
        return Ok(());
    }
    let Some(gltf_handle) = world.get::<ModelGltf>(model).map(|gltf| gltf.0.clone()) else {
        return Err("the entity is not a glTF model made by `model.spawn`".to_string());
    };
    let clips: Vec<(String, Handle<AnimationClip>)> = {
        let Some(gltfs) = world.get_resource::<Assets<Gltf>>() else {
            return Err("the app has no glTF support".to_string());
        };
        let Some(gltf) = gltfs.get(&gltf_handle) else {
            return Err("its glTF file has not finished loading".to_string());
        };
        let mut clips: Vec<(String, Handle<AnimationClip>)> = gltf
            .named_animations
            .iter()
            .map(|(name, handle)| (name.to_string(), handle.clone()))
            .collect();
        clips.sort_by(|(left, _), (right, _)| left.cmp(right));
        clips
    };
    if clips.is_empty() {
        return Err("its glTF file has no named animation clips".to_string());
    }
    let Some(player) = find_player(world, model) else {
        return Err("its scene has not been spawned yet (no AnimationPlayer found)".to_string());
    };
    let (graph, node_indices) = AnimationGraph::from_clips(clips.iter().map(|(_, h)| h.clone()));
    let graph_handle = world.resource_mut::<Assets<AnimationGraph>>().add(graph);
    let mut nodes = BTreeMap::new();
    let mut durations = BTreeMap::new();
    for ((name, handle), node) in clips.iter().zip(node_indices) {
        let duration = world
            .resource::<Assets<AnimationClip>>()
            .get(handle)
            .map(AnimationClip::duration)
            .ok_or_else(|| format!("clip `{name}` has not loaded"))?;
        nodes.insert(name.clone(), node);
        durations.insert(name.clone(), duration);
    }
    world
        .entity_mut(player)
        .insert(AnimationGraphHandle(graph_handle));
    world
        .entity_mut(model)
        .insert(AnimationRig::new(player, nodes, durations));
    Ok(())
}

/// Builds the rig of every model that has none yet, as soon as its file and scene are ready.
fn build_animation_rigs(world: &mut World) {
    let mut models = world.query_filtered::<Entity, (With<ModelGltf>, Without<AnimationRig>)>();
    let waiting: Vec<Entity> = models.iter(world).collect();
    for model in waiting {
        // Not ready yet is normal; commands report it when someone asks for the animation.
        let _ = ensure_rig(world, model);
    }
}

/// Advances each listed clip by one fixed tick, ends finished one-shots, and drops (with a
/// script failure) clips the model's file does not have.
fn advance_clip_times(
    mut commands: Commands,
    fixed_time: Res<Time<Fixed>>,
    replaying: Option<Res<NetReplaying>>,
    mut models: Query<(&Name, &mut AnimationPlayback, &mut AnimationRig)>,
) {
    // Poses are not rewound by a client's correction, so its replay must not advance them again.
    if replaying.is_some_and(|replaying| replaying.0) {
        return;
    }
    let tick = fixed_time.delta_secs();
    for (name, mut playback, mut rig) in &mut models {
        let mut unknown: Vec<String> = Vec::new();
        let mut finished: Vec<String> = Vec::new();
        for clip in &playback.clips {
            let Some(duration) = rig.durations.get(&clip.clip).copied() else {
                unknown.push(clip.clip.clone());
                continue;
            };
            let time = rig.times.entry(clip.clip.clone()).or_insert(0.0);
            *time += tick * clip.speed;
            if one_shot_finished(*time, duration, clip.looping) {
                finished.push(clip.clip.clone());
            }
        }
        if !unknown.is_empty() {
            let message = format!(
                "model `{name}` has no animation clip {}; its clips: {}",
                unknown
                    .iter()
                    .map(|clip| format!("`{clip}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                rig.clip_names()
            );
            error!("{message}");
            commands.queue(move |world: &mut World| fail_running_script(world, message));
        }
        if !unknown.is_empty() || !finished.is_empty() {
            // A finished one-shot leaves the list but keeps its last pose (its time stays in the
            // rig until another clip replaces it).
            playback
                .clips
                .retain(|clip| !unknown.contains(&clip.clip) && !finished.contains(&clip.clip));
        }
        let listed: Vec<String> = playback
            .clips
            .iter()
            .map(|clip| clip.clip.clone())
            .collect();
        rig.times
            .retain(|clip, _| listed.contains(clip) || finished.contains(clip));
    }
}

/// Writes the pose into Bevy's `AnimationPlayer`: listed clips play with their weight at their
/// pose time (speed 0 so Bevy never advances them), other clips stop.
fn apply_animation_poses(
    fixed_time: Res<Time<Fixed>>,
    models: Query<(&AnimationPlayback, &AnimationRig)>,
    mut players: Query<&mut AnimationPlayer>,
) {
    let overstep = fixed_time.overstep().as_secs_f32();
    for (playback, rig) in &models {
        let Ok(mut player) = players.get_mut(rig.player) else {
            continue;
        };
        for (name, node) in &rig.nodes {
            let listed: Option<&ClipPlayback> = playback.clips.iter().find(|c| &c.clip == name);
            match listed {
                Some(clip) => {
                    let duration = rig.durations.get(name).copied().unwrap_or(0.0);
                    let time = rig.times.get(name).copied().unwrap_or(0.0) + overstep * clip.speed;
                    if !player.is_playing_animation(*node) {
                        player.play(*node);
                    }
                    if let Some(active) = player.animation_mut(*node) {
                        active
                            .set_weight(clip.weight)
                            .set_speed(0.0)
                            .set_seek_time(clip_pose_time(time, duration, clip.looping));
                    }
                }
                None => {
                    if player.is_playing_animation(*node) {
                        player.stop(*node);
                    }
                }
            }
        }
    }
}

/// The model named by argument 0, with its rig built (commands may run before the update that
/// would build it) and an `AnimationPlayback` present.
fn model_with_rig(world: &mut World, args: &CommandArgs) -> Result<(String, Entity), CommandError> {
    let name: String = args.parse(0, "model")?;
    let entity = find_unique_entity_by_name(world, args, &name)?;
    ensure_rig(world, entity)
        .map_err(|reason| args.failure(format!("model `{name}` cannot animate: {reason}")))?;
    if world.get::<AnimationPlayback>(entity).is_none() {
        world
            .entity_mut(entity)
            .insert(AnimationPlayback::default());
    }
    Ok((name, entity))
}

/// Replaces the model's playback with `clips`, validating them against the file's clips, and
/// restarts every clip's time.
fn set_playback(
    world: &mut World,
    args: &CommandArgs,
    entity: Entity,
    clips: Vec<ClipPlayback>,
) -> CommandResult {
    let playback = AnimationPlayback { clips };
    playback
        .validate()
        .map_err(|message| args.failure(message))?;
    let rig = world.get::<AnimationRig>(entity).expect("rig ensured");
    for clip in &playback.clips {
        if !rig.nodes.contains_key(&clip.clip) {
            return Err(args.failure(format!(
                "this model has no animation clip `{}`; its clips: {}",
                clip.clip,
                rig.clip_names()
            )));
        }
    }
    world
        .get_mut::<AnimationRig>(entity)
        .expect("rig ensured")
        .times
        .clear();
    world.entity_mut(entity).insert(playback);
    Ok(CommandOutcome::Continue)
}

/// `animation.list <model>`: logs the clips and their lengths.
fn list_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let (name, entity) = model_with_rig(world, args)?;
    let rig = world.get::<AnimationRig>(entity).expect("rig ensured");
    for (clip, duration) in &rig.durations {
        info!("model `{name}` clip `{clip}`: {duration:.3} s");
    }
    Ok(CommandOutcome::Continue)
}

/// `animation.play <model> <clip> <once|loop> <speed>`: plays one clip, replacing the rest.
fn play_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let clip: String = args.parse(1, "clip")?;
    let mode: String = args.parse(2, "once|loop")?;
    let speed: f32 = args.parse(3, "speed")?;
    let looping = match mode.as_str() {
        "once" => false,
        "loop" => true,
        other => {
            return Err(args.failure(format!(
                "<once|loop> must be `once` or `loop`, got `{other}`"
            )));
        }
    };
    let (_, entity) = model_with_rig(world, args)?;
    let playback = ClipPlayback {
        speed,
        looping,
        ..ClipPlayback::once(clip)
    };
    set_playback(world, args, entity, vec![playback])
}

/// `animation.blend <model> <clipA> <clipB> <weightB>`: two looping clips, weights
/// `1 - weightB` and `weightB`.
fn blend_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let first: String = args.parse(1, "clipA")?;
    let second: String = args.parse(2, "clipB")?;
    let weight: f32 = args.parse(3, "weightB")?;
    let (_, entity) = model_with_rig(world, args)?;
    let clips = vec![
        ClipPlayback {
            weight: 1.0 - weight,
            ..ClipPlayback::looping(first)
        },
        ClipPlayback {
            weight,
            ..ClipPlayback::looping(second)
        },
    ];
    set_playback(world, args, entity, clips)
}

/// `animation.stop <model>`: stops every clip; the model keeps its pose.
fn stop_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let (_, entity) = model_with_rig(world, args)?;
    set_playback(world, args, entity, Vec::new())
}

/// The listed clip `clip` of the model named by argument 0, plus the rig's time for it.
fn listed_clip(
    world: &mut World,
    args: &CommandArgs,
    clip: &str,
) -> Result<(ClipPlayback, f32, f32), CommandError> {
    let (name, entity) = model_with_rig(world, args)?;
    let playback = world.get::<AnimationPlayback>(entity).expect("ensured");
    let rig = world.get::<AnimationRig>(entity).expect("ensured");
    let Some(found) = playback.clips.iter().find(|c| c.clip == clip) else {
        let listed: Vec<&str> = playback.clips.iter().map(|c| c.clip.as_str()).collect();
        return Err(args.failure(format!(
            "model `{name}` is not playing `{clip}`; playing: {}",
            if listed.is_empty() {
                "(nothing)".to_string()
            } else {
                listed.join(", ")
            }
        )));
    };
    let duration = rig.durations.get(clip).copied().unwrap_or(0.0);
    let time = rig.times.get(clip).copied().unwrap_or(0.0);
    Ok((found.clone(), time, duration))
}

/// `assert.animation_playing <model> <clip>`.
fn assert_playing_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let clip: String = args.parse(1, "clip")?;
    listed_clip(world, args, &clip)?;
    Ok(CommandOutcome::Continue)
}

/// `assert.animation_time <model> <clip> <seconds>`: the clip's pose time (wrapped for loops).
fn assert_time_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let clip: String = args.parse(1, "clip")?;
    let expected: f32 = args.parse(2, "seconds")?;
    let (found, time, duration) = listed_clip(world, args, &clip)?;
    let actual = clip_pose_time(time, duration, found.looping);
    if (actual - expected).abs() <= TIME_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "clip `{clip}` is at {actual} s, expected {expected} s (tolerance {TIME_TOLERANCE})"
        )))
    }
}

/// `assert.animation_weight <model> <clip> <weight>`.
fn assert_weight_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let clip: String = args.parse(1, "clip")?;
    let expected: f32 = args.parse(2, "weight")?;
    let (found, _, _) = listed_clip(world, args, &clip)?;
    if (found.weight - expected).abs() <= TIME_TOLERANCE {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "clip `{clip}` has weight {}, expected {expected}",
            found.weight
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looping_clips_wrap_and_one_shots_hold_before_the_end() {
        assert!((clip_pose_time(2.5, 1.0, true) - 0.5).abs() < 1e-6);
        assert_eq!(clip_pose_time(0.25, 1.0, false), 0.25);
        assert!(clip_pose_time(5.0, 1.0, false) < 1.0);
        assert_eq!(clip_pose_time(5.0, 0.0, true), 0.0);
        assert_eq!(clip_pose_time(-1.0, 2.0, false), 0.0);
    }

    #[test]
    fn only_non_looping_clips_finish() {
        assert!(one_shot_finished(1.0, 1.0, false));
        assert!(!one_shot_finished(0.99, 1.0, false));
        assert!(!one_shot_finished(10.0, 1.0, true));
    }

    use crate::{CommandScript, RetreatCommandsPlugin};

    /// A model with a hand-made rig: clips `Short` (0.25 s) and `Long` (1 s), no glTF file.
    fn app_with_model(script: &str) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<AnimationClip>()
            .init_asset::<AnimationGraph>()
            .add_plugins(RetreatCommandsPlugin {
                seed: 0,
                command_script: Some(CommandScript::from_text("test.cmds", script)),
            })
            .add_plugins(AnimationCommandsPlugin);
        let clips = {
            let mut assets = app.world_mut().resource_mut::<Assets<AnimationClip>>();
            [
                assets.add(AnimationClip::default()),
                assets.add(AnimationClip::default()),
            ]
        };
        let (graph, indices) = AnimationGraph::from_clips(clips);
        let graph = app
            .world_mut()
            .resource_mut::<Assets<AnimationGraph>>()
            .add(graph);
        let player = app
            .world_mut()
            .spawn((AnimationPlayer::default(), AnimationGraphHandle(graph)))
            .id();
        let nodes = BTreeMap::from([
            ("Short".to_string(), indices[0]),
            ("Long".to_string(), indices[1]),
        ]);
        let durations = BTreeMap::from([("Short".to_string(), 0.25), ("Long".to_string(), 1.0)]);
        let model = app
            .world_mut()
            .spawn((
                Name::new("fox"),
                AnimationRig::new(player, nodes, durations),
                AnimationPlayback::default(),
            ))
            .id();
        (app, model)
    }

    fn run(script: &str) -> (App, Entity, Option<String>) {
        let (mut app, model) = app_with_model(script);
        for _ in 0..500 {
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
        (app, model, failure)
    }

    #[test]
    fn a_looping_clip_advances_one_tick_of_time_per_script_line() {
        // 1/64 s per tick. `animation.play` runs on tick 1 and the clip advances later in that
        // tick; the assert on tick 3 runs before tick 3's advance, so it sees two ticks.
        let (_, _, failure) = run("animation.play fox Long loop 1
assert.animation_playing fox Long
             assert.animation_time fox Long 0.03125
quit
");
        assert_eq!(failure, None);
    }

    #[test]
    fn speed_scales_time_and_loops_wrap() {
        let (_, _, failure) = run("animation.play fox Long loop 2
wait 64
assert.animation_time fox Long 0.0625
quit
");
        // 66 ticks at 2x = 2.0625 s -> wraps to 0.0625 s on a 1 s clip.
        assert_eq!(failure, None);
    }

    #[test]
    fn a_one_shot_leaves_the_list_when_it_ends() {
        let (_, _, failure) = run("animation.play fox Short once 1
wait 8
assert.animation_playing fox Short
wait 16
assert.animation_playing fox Short
quit
");
        let failure = failure.expect("the 0.25 s clip must be over after 24 ticks");
        assert!(
            failure.contains("is not playing `Short`; playing: (nothing)"),
            "{failure}"
        );
    }

    #[test]
    fn a_blend_lists_both_clips_with_their_weights() {
        let (_, _, failure) = run("animation.blend fox Short Long 0.25
assert.animation_weight fox Short 0.75
             assert.animation_weight fox Long 0.25
animation.stop fox
quit
");
        assert_eq!(failure, None);
    }

    #[test]
    fn bevys_player_gets_the_pose_time_with_speed_zero() {
        let (mut app, model, failure) = run("animation.play fox Long loop 1
wait 15
quit
");
        assert_eq!(failure, None);
        app.update();
        let rig_player = app.world().get::<AnimationRig>(model).unwrap().player;
        let node = app.world().get::<AnimationRig>(model).unwrap().nodes["Long"];
        let player = app.world().get::<AnimationPlayer>(rig_player).unwrap();
        let active = player.animation(node).expect("the clip is active");
        assert_eq!(
            active.speed(),
            0.0,
            "Bevy must never advance the clip itself"
        );
        assert!(
            active.seek_time() > 0.2 && active.seek_time() < 1.0,
            "{}",
            active.seek_time()
        );
    }

    #[test]
    fn errors_name_the_clips_and_the_choices() {
        let (_, _, failure) = run("animation.play fox Fly loop 1
quit
");
        let failure = failure.expect("unknown clip must fail");
        assert!(
            failure.contains("no animation clip `Fly`; its clips: Long, Short"),
            "{failure}"
        );
        let (_, _, failure) = run("animation.play fox Long twice 1
quit
");
        assert!(
            failure
                .expect("bad mode")
                .contains("must be `once` or `loop`")
        );
        let (_, _, failure) = run("animation.play fox Long loop 9
quit
");
        assert!(
            failure
                .expect("bad speed")
                .contains("speed of clip `Long` must be from 0 to 4")
        );
        let (_, _, failure) = run("animation.blend fox Short Long 2
quit
");
        assert!(failure.expect("bad weight").contains("weight of clip"));
        let (_, _, failure) = run("animation.play nobody Long loop 1
quit
");
        assert!(
            failure
                .expect("no model")
                .contains("no entity is named `nobody`")
        );
    }

    #[test]
    fn an_entity_that_is_not_a_model_cannot_animate() {
        let (mut app, _) = app_with_model(
            "animation.play plain Long loop 1
quit
",
        );
        app.world_mut().spawn(Name::new("plain"));
        for _ in 0..10 {
            app.update();
        }
        let failure = app
            .world()
            .resource::<CommandScript>()
            .failure()
            .map(String::from);
        let failure = failure.expect("a plain entity must be rejected");
        assert!(failure.contains("not a glTF model"), "{failure}");
    }
}
