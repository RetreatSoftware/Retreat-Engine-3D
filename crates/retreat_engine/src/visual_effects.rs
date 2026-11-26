//! Shows the contract's one-shot effects (`visual_effects.rs`): each id once, as a sphere that
//! grows over its lifetime. Render only.
//!
//! Requests are taken after every tick (replayed ones included) and remembered by id; ages advance on
//! live ticks only, so a replay neither restarts nor ages an effect.
//!
//! Commands: `effect.play <id> <x> <y> <z> <radius> <r> <g> <b> <ticks>`,
//! `assert.effects_played <count>`, `assert.effect_visible <id>`.

use std::collections::BTreeMap;

use bevy::app::FixedLast;
use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandOutcome, CommandResult, EFFECT_MEMORY_TICKS, EffectTime,
    NetReplaying, NetViewTick, SphereVisual, VisualEffect, VisualEffects,
};

use crate::net::client::NetClient;

/// Start of the name of an effect's entity.
pub const EFFECT_NAME_PREFIX: &str = "effect/";

/// What this machine knows about effects.
#[derive(Resource, Default)]
pub struct EffectLog {
    /// Ids seen, with the live tick they came at.
    seen: BTreeMap<String, u64>,
    /// Effects waiting for the view time to reach their tick.
    waiting: Vec<(u64, VisualEffect)>,
    live_ticks: u64,
    /// Effects shown so far.
    pub played: u64,
}

/// On an effect's entity.
#[derive(Component)]
struct ActiveEffect {
    effect: VisualEffect,
    age: u32,
}

pub struct VisualEffectsPlugin;

impl Plugin for VisualEffectsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VisualEffects>()
            .init_resource::<EffectLog>()
            .init_resource::<NetViewTick>()
            .init_resource::<NetReplaying>()
            // Before the client counts the tick: the tick number is the one being simulated.
            .add_systems(FixedPostUpdate, take_requests)
            .add_systems(FixedLast, (age_effects, start_due_effects).chain())
            .register_command(
                "effect.play",
                "effect.play <id> <x> <y> <z> <radius> <r> <g> <b> <ticks>",
                play_command,
            )
            .register_command(
                "assert.effects_played",
                "assert.effects_played <count>",
                assert_played_command,
            )
            .register_command(
                "assert.effect_visible",
                "assert.effect_visible <id>",
                assert_visible_command,
            );
    }
}

fn take_requests(
    client: Option<Res<NetClient>>,
    mut requests: ResMut<VisualEffects>,
    mut log: ResMut<EffectLog>,
) {
    if requests.requests.is_empty() {
        return;
    }
    let tick = client.map_or(0, |client| client.tick + 1);
    for effect in std::mem::take(&mut requests.requests) {
        let now = log.live_ticks;
        if log.seen.contains_key(&effect.id) {
            continue;
        }
        log.seen.insert(effect.id.clone(), now);
        let due = match effect.time {
            EffectTime::Present => 0,
            EffectTime::RemoteView => tick,
        };
        log.waiting.push((due, effect));
    }
}

fn start_due_effects(view: Res<NetViewTick>, mut log: ResMut<EffectLog>, mut commands: Commands) {
    let view = view.0.unwrap_or(f64::MAX);
    let (due, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut log.waiting)
        .into_iter()
        .partition(|(tick, _)| *tick as f64 <= view);
    log.waiting = waiting;
    for (_, effect) in due {
        log.played += 1;
        commands.spawn((
            Name::new(format!("{EFFECT_NAME_PREFIX}{}", effect.id)),
            Transform::from_translation(effect.position),
            sphere_at(&effect, 0),
            ActiveEffect { effect, age: 0 },
        ));
    }
}

/// The sphere of an effect at `age` ticks: it grows fast, then slower, to the full radius.
fn sphere_at(effect: &VisualEffect, age: u32) -> SphereVisual {
    let life = effect.ticks.max(1) as f32;
    let progress = ((age + 1) as f32 / life).min(1.0);
    SphereVisual {
        radius: effect.radius * progress.sqrt(),
        color: effect.color,
        glow: effect.glow,
    }
}

fn age_effects(
    replaying: Res<NetReplaying>,
    mut log: ResMut<EffectLog>,
    mut active: Query<(Entity, &mut ActiveEffect, &mut SphereVisual)>,
    mut commands: Commands,
) {
    if replaying.0 {
        return;
    }
    log.live_ticks += 1;
    let now = log.live_ticks;
    log.seen
        .retain(|_, seen| now - *seen <= EFFECT_MEMORY_TICKS);
    for (entity, mut effect, mut sphere) in &mut active {
        effect.age += 1;
        if effect.age >= effect.effect.ticks {
            commands.entity(entity).despawn();
        } else {
            *sphere = sphere_at(&effect.effect, effect.age);
        }
    }
}

fn play_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(9)?;
    let number = |index, label| args.parse::<f32>(index, label);
    let effect = VisualEffect {
        id: args.parse(0, "id")?,
        position: Vec3::new(number(1, "x")?, number(2, "y")?, number(3, "z")?),
        radius: number(4, "radius")?,
        color: Color::srgb(number(5, "r")?, number(6, "g")?, number(7, "b")?),
        glow: 1.0,
        ticks: args.parse(8, "ticks")?,
        time: EffectTime::Present,
    };
    if !(effect.position.is_finite() && effect.radius.is_finite() && effect.radius > 0.0) {
        return Err(args.failure("the position is finite and the radius positive"));
    }
    world.resource_mut::<VisualEffects>().play(effect);
    Ok(CommandOutcome::Continue)
}

fn assert_played_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let expected: u64 = args.parse(0, "count")?;
    let played = world.resource::<EffectLog>().played;
    if played == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("{played} effects were played, expected {expected}")))
    }
}

fn assert_visible_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(1)?;
    let id: String = args.parse(0, "id")?;
    let found = world
        .query::<&ActiveEffect>()
        .iter(world)
        .any(|active| active.effect.id == id);
    if found {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!("no effect `{id}` is showing")))
    }
}
