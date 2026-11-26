//! A demo of what a weapon module uses (it is not the arena's rifle): a lag-compensated shot that
//! carries the shooter's view tick, health as synced state, a dead walker hidden until it
//! respawns with a teleport, and a health readout, hit counter and crosshair.
//!
//! - `sandbox.walker_fire <yaw> <pitch> <view_tick>`: the sender's walker shoots (left mouse
//!   button). Two hits put a walker down for [`DOWN_TICKS`] ticks.
//! - `sandbox.walker_dummy <x> <z>`: a walker nobody controls, to shoot at.
//! - `assert.walker_health <name> <health>`.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, AppHudExt, AppInputExt, AppNetExt, AppNetStateExt, CommandArgs, CommandOutcome,
    CommandResult, HELD_ONLY, HideVisuals, HudAnchor, HudKind, HudWidget, InputAction,
    InputActionSystems, InputActions, InputKey, Intangible, NetCommandSender, NetInputQueue,
    NetLocalClient, NetReplaying, NetState, NetViewTick, PhysicsTeleport, RayCast, RetiredEntity,
    UiHud, cast_ray_at, run_command_line,
};
use serde::{Deserialize, Serialize};

use crate::walker::{Walker, WalkerAim, spawn_walker, walker_of};

pub const FULL_HEALTH: u32 = 100;
const SHOT_DAMAGE: u32 = 50;
/// How long a walker that lost its health stays down (2 seconds).
pub const DOWN_TICKS: u32 = 128;
/// Height of the eyes above the walker's center.
pub const EYE_HEIGHT: f32 = 0.5;
const SHOT_RANGE: f32 = 100.0;
/// The owner number of a dummy walker (no client has it).
const DUMMY_OWNER: u32 = 99;

const HEALTH_WIDGET: &str = "walker_health";
const HITS_WIDGET: &str = "walker_hits";
const CROSSHAIR_WIDGET: &str = "walker_crosshair";

/// A walker's health: synced state.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WalkerHealth {
    pub health: u32,
    /// Shots of this walker that hit someone.
    pub hits: u32,
    /// Ticks left until it gets up; 0 while it stands.
    pub down_ticks: u32,
    /// Where it gets up.
    pub spawn: Vec3,
}

impl NetState for WalkerHealth {
    const ID: &'static str = "sandbox.walker_health";
}

pub struct WalkerCombatPlugin;

impl Plugin for WalkerCombatPlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<WalkerHealth>()
            .register_command(
                "sandbox.walker_fire",
                "sandbox.walker_fire <yaw> <pitch> <view_tick>",
                fire_command,
            )
            .register_command(
                "sandbox.walker_dummy",
                "sandbox.walker_dummy <x> <z>",
                dummy_command,
            )
            .register_command(
                "assert.walker_health",
                "assert.walker_health <name> <health>",
                assert_health_command,
            )
            .allow_client_command("sandbox.walker_fire")
            .register_action(InputAction {
                id: "walk_fire",
                command_line: HELD_ONLY,
                default_keys: &[InputKey::MouseLeft],
            })
            .register_hud_widget(widget(HITS_WIDGET, HudAnchor::TopLeft, 20.0))
            .register_hud_widget(widget(HEALTH_WIDGET, HudAnchor::BottomLeft, 32.0))
            .register_hud_widget(HudWidget {
                id: CROSSHAIR_WIDGET,
                anchor: HudAnchor::Center,
                kind: HudKind::Crosshair { size: 24.0 },
                visible: false,
            })
            .add_systems(
                FixedUpdate,
                (send_fire.after(InputActionSystems), get_up_again),
            )
            .add_systems(Update, (show_readouts, hide_downed_walkers));
    }
}

fn widget(id: &'static str, anchor: HudAnchor, font_size: f32) -> HudWidget {
    HudWidget {
        id,
        anchor,
        kind: HudKind::Text { font_size },
        visible: false,
    }
}

/// Turns a new press of the fire button into one shot command with the view tick.
fn send_fire(world: &mut World, mut held_before: Local<bool>) {
    // A rewind replays recorded inputs; the button's edge belongs to the present.
    if world.resource::<NetReplaying>().0 {
        return;
    }
    let held = world.resource::<InputActions>().is_held("walk_fire");
    let pressed = held && !*held_before;
    *held_before = held;
    let local = world.resource::<NetLocalClient>().0;
    if !pressed || walker_of(world, local).is_none() {
        return;
    }
    let yaw = world.resource::<WalkerAim>().yaw;
    let view_tick = world.resource::<NetViewTick>().for_input();
    let line = format!("sandbox.walker_fire {yaw} 0 {view_tick}");
    if world.resource::<NetInputQueue>().accepting {
        world.resource_mut::<NetInputQueue>().push(line);
    } else if let Err(error) = run_command_line(world, &line) {
        warn!("{error}");
    }
}

fn fire_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(3)?;
    let yaw: f32 = args.parse(0, "yaw")?;
    let pitch: f32 = args.parse(1, "pitch")?;
    let view_tick: f64 = args.parse(2, "view_tick")?;
    if !(yaw.is_finite() && pitch.is_finite() && view_tick.is_finite()) {
        return Err(args.failure("the numbers must be finite"));
    }
    let owner = world.resource::<NetCommandSender>().0;
    let Some(shooter) = walker_of(world, owner) else {
        return Err(args.failure("the sender has no walker"));
    };
    if world
        .get::<WalkerHealth>(shooter)
        .is_some_and(|health| health.down_ticks > 0)
    {
        return Err(args.failure("a downed walker cannot shoot"));
    }
    let origin = world
        .get::<Transform>(shooter)
        .map_or(Vec3::ZERO, |transform| transform.translation)
        + Vec3::Y * EYE_HEIGHT;
    let aim = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch) * Vec3::NEG_Z;
    let cast = RayCast {
        origin,
        direction: aim,
        max_distance: SHOT_RANGE,
        // Downed walkers are intangible: the cast does not meet them.
        exclude: vec![shooter],
    };
    let Some(hit) = cast_ray_at(world, &cast, view_tick) else {
        return Ok(CommandOutcome::Continue);
    };
    // The present health decides; only the aim looked into the past.
    if hurt_walker(world, hit.entity, SHOT_DAMAGE)
        && let Some(mut own) = world.get_mut::<WalkerHealth>(shooter)
    {
        own.hits += 1;
    }
    Ok(CommandOutcome::Continue)
}

/// Damages a standing walker; one that runs out of health goes down and becomes intangible.
/// False when `target` is not a standing walker.
pub fn hurt_walker(world: &mut World, target: Entity, damage: u32) -> bool {
    let Some(mut health) = world.get_mut::<WalkerHealth>(target) else {
        return false;
    };
    if health.down_ticks > 0 {
        return false;
    }
    health.health = health.health.saturating_sub(damage);
    if health.health == 0 {
        health.down_ticks = DOWN_TICKS;
        world.entity_mut(target).insert(Intangible);
    }
    true
}

fn get_up_again(
    mut walkers: Query<(Entity, &mut WalkerHealth), Without<RetiredEntity>>,
    mut commands: Commands,
) {
    for (entity, mut health) in &mut walkers {
        if health.down_ticks == 0 {
            continue;
        }
        health.down_ticks -= 1;
        if health.down_ticks == 0 {
            health.health = FULL_HEALTH;
            commands
                .entity(entity)
                .insert(PhysicsTeleport(health.spawn))
                .remove::<Intangible>();
        }
    }
}

/// Looks only: a downed walker is not drawn.
fn hide_downed_walkers(
    walkers: Query<(Entity, &WalkerHealth, Has<HideVisuals>)>,
    mut commands: Commands,
) {
    for (entity, health, hidden) in &walkers {
        match (health.down_ticks > 0, hidden) {
            (true, false) => {
                commands.entity(entity).insert(HideVisuals);
            }
            (false, true) => {
                commands.entity(entity).remove::<HideVisuals>();
            }
            _ => {}
        }
    }
}

/// Shows the readouts once this machine has a walker, and keeps their text current.
fn show_readouts(
    local: Res<NetLocalClient>,
    walkers: Query<(&Walker, Ref<WalkerHealth>)>,
    mut hud: ResMut<UiHud>,
) {
    let Some((_, health)) = walkers.iter().find(|(walker, _)| walker.owner == local.0) else {
        return;
    };
    let texts = [
        (HEALTH_WIDGET, format!("health {}", health.health)),
        (HITS_WIDGET, format!("hits {}", health.hits)),
    ];
    for (id, text) in texts {
        if !hud.has_text(id, &text) {
            hud.set_text(id, text).expect("registered above");
        }
    }
    if health.is_added() {
        for id in [HEALTH_WIDGET, HITS_WIDGET, CROSSHAIR_WIDGET] {
            hud.set_visible(id, true).expect("registered above");
        }
    }
}

fn dummy_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let x: f32 = args.parse(0, "x")?;
    let z: f32 = args.parse(1, "z")?;
    spawn_walker(world, args, Some(DUMMY_OWNER), x, z)?;
    Ok(CommandOutcome::Continue)
}

fn assert_health_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let name: String = args.parse(0, "name")?;
    let expected: u32 = args.parse(1, "health")?;
    let found = world
        .query::<(&Name, &WalkerHealth)>()
        .iter(world)
        .find(|(found, _)| found.as_str() == name)
        .map(|(_, health)| health.health);
    match found {
        Some(health) if health == expected => Ok(CommandOutcome::Continue),
        Some(health) => {
            Err(args.failure(format!("`{name}` has health {health}, expected {expected}")))
        }
        None => Err(args.failure(format!("there is no walker `{name}`"))),
    }
}
