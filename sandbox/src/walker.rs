//! A demo character controller, showing the engine capabilities a character module uses: analog
//! input sent as a command line per tick, an upright character body that climbs steps and slopes
//! (`CharacterStepping`) and whose velocity the game sets, synced state, and looks derived from it
//! on each machine (a color per player, smoothing of the other players). It is not the arena's
//! controller.
//!
//! - `sandbox.walker_spawn <x> <z>`: the sender's character (`walker_<client id>`, or
//!   `walker_local` without a network).
//! - `sandbox.walker_input <move_x> <move_z> <yaw> <jump 0|1>`: the sender's input for a tick.
//! - Keys W A S D (axes `walk_x`, `walk_z`), Space (`walk_jump`), mouse look turns.

use bevy::prelude::*;
use retreat_engine_api::{
    AnalogInput, AppAxisExt, AppCommandExt, AppInputExt, AppNetExt, AppNetStateExt,
    CharacterGround, CharacterStepping, CharacterVisual, CommandArgs, CommandOutcome,
    CommandResult, EntityColor, HELD_ONLY, InputAction, InputActionSystems, InputActions,
    InputAxis, InputKey, NetCommandSender, NetInputQueue, NetLocalClient, NetState, PhysicsBody,
    PhysicsControlSystems, PhysicsVelocity, PhysicsVelocitySet, RemoteInterpolation, RetiredEntity,
};
use serde::{Deserialize, Serialize};

use crate::walker_combat::{FULL_HEALTH, WalkerHealth};

const HALF_HEIGHT: f32 = 0.5;
const RADIUS: f32 = 0.4;
const SPEED: f32 = 4.0;
const STEP_HEIGHT: f32 = 0.35;
const MAX_SLOPE_DEGREES: f32 = 45.0;
const GRAVITY: f32 = 19.6;
/// Upward speed of a jump (about 1.5 m high under [`GRAVITY`]).
const JUMP_SPEED: f32 = 7.67;
/// Radians of yaw per count of mouse movement.
const LOOK_SENSITIVITY: f32 = 0.003;

/// A character's synced state.
#[derive(Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Walker {
    pub owner: Option<u32>,
    pub move_x: f32,
    pub move_z: f32,
    pub yaw: f32,
    pub jump: bool,
    pub grounded: bool,
}

impl NetState for Walker {
    const ID: &'static str = "sandbox.walker";
}

/// This machine's view angle (not synced: it travels with every input).
#[derive(Resource, Default)]
pub struct WalkerAim {
    pub yaw: f32,
}

pub struct WalkerPlugin;

impl Plugin for WalkerPlugin {
    fn build(&self, app: &mut App) {
        app.register_net_state::<Walker>()
            .init_resource::<WalkerAim>()
            .register_command(
                "sandbox.walker_spawn",
                "sandbox.walker_spawn <x> <z>",
                spawn_command,
            )
            .register_command(
                "sandbox.walker_input",
                "sandbox.walker_input <move_x> <move_z> <yaw> <jump>",
                input_command,
            )
            .allow_client_command("sandbox.walker_spawn")
            .allow_client_command("sandbox.walker_input")
            .register_action(held("walk_left", &[InputKey::A]))
            .register_action(held("walk_right", &[InputKey::D]))
            .register_action(held("walk_forward", &[InputKey::W]))
            .register_action(held("walk_back", &[InputKey::S]))
            .register_action(held("walk_jump", &[InputKey::Space]))
            .register_axis(InputAxis {
                id: "walk_x",
                negative_action: "walk_left",
                positive_action: "walk_right",
            })
            .register_axis(InputAxis {
                id: "walk_z",
                negative_action: "walk_forward",
                positive_action: "walk_back",
            })
            .add_systems(Update, dress_walkers)
            .add_systems(
                FixedUpdate,
                (
                    send_input.after(InputActionSystems),
                    move_walkers.in_set(PhysicsControlSystems),
                )
                    .chain(),
            );
    }
}

fn held(id: &'static str, default_keys: &'static [InputKey]) -> InputAction {
    InputAction {
        id,
        command_line: HELD_ONLY,
        default_keys,
    }
}

fn walker_name(owner: Option<u32>) -> String {
    owner.map_or_else(|| "walker_local".to_string(), |id| format!("walker_{id}"))
}

/// The live walker of `owner`.
pub fn walker_of(world: &mut World, owner: Option<u32>) -> Option<Entity> {
    world
        .query_filtered::<(Entity, &Walker), Without<RetiredEntity>>()
        .iter(world)
        .find(|(_, walker)| walker.owner == owner)
        .map(|(entity, _)| entity)
}

fn spawn_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let x: f32 = args.parse(0, "x")?;
    let z: f32 = args.parse(1, "z")?;
    if !(x.is_finite() && z.is_finite()) {
        return Err(args.failure("<x> and <z> must be finite"));
    }
    let owner = world.resource::<NetCommandSender>().0;
    spawn_walker(world, args, owner, x, z)
}

/// Spawns the walker of `owner` standing at (x, z).
pub fn spawn_walker(
    world: &mut World,
    args: &CommandArgs,
    owner: Option<u32>,
    x: f32,
    z: f32,
) -> CommandResult {
    let name = walker_name(owner);
    if world
        .query::<&Name>()
        .iter(world)
        .any(|found| found.as_str() == name)
    {
        return Err(args.failure(format!("`{name}` exists already")));
    }
    world.spawn((
        Name::new(name),
        PhysicsBody::character(HALF_HEIGHT, RADIUS),
        Transform::from_xyz(x, HALF_HEIGHT + RADIUS, z),
        Walker {
            owner,
            ..Walker::default()
        },
        WalkerHealth {
            health: FULL_HEALTH,
            hits: 0,
            down_ticks: 0,
            spawn: Vec3::new(x, HALF_HEIGHT + RADIUS + 0.01, z),
        },
        CharacterStepping {
            max_step_height: STEP_HEIGHT,
            max_slope_degrees: MAX_SLOPE_DEGREES,
        },
    ));
    Ok(CommandOutcome::Continue)
}

fn input_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(4)?;
    let move_x: f32 = args.parse(0, "move_x")?;
    let move_z: f32 = args.parse(1, "move_z")?;
    let yaw: f32 = args.parse(2, "yaw")?;
    let jump: u8 = args.parse(3, "jump")?;
    if !(move_x.is_finite() && move_z.is_finite() && yaw.is_finite()) {
        return Err(args.failure("the numbers must be finite"));
    }
    let owner = world.resource::<NetCommandSender>().0;
    let Some(entity) = walker_of(world, owner) else {
        return Err(args.failure(format!("there is no `{}`", walker_name(owner))));
    };
    let mut walker = world
        .get_mut::<Walker>(entity)
        .expect("found by its Walker");
    walker.move_x = move_x.clamp(-1.0, 1.0);
    walker.move_z = move_z.clamp(-1.0, 1.0);
    walker.yaw = yaw;
    walker.jump = jump != 0;
    Ok(CommandOutcome::Continue)
}

/// Turns this machine's input into its walker's input: a command line for the server (and the
/// prediction) on a client, written directly without a network.
fn send_input(
    analog: Res<AnalogInput>,
    actions: Res<InputActions>,
    local: Res<NetLocalClient>,
    mut aim: ResMut<WalkerAim>,
    mut queue: ResMut<NetInputQueue>,
    mut walkers: Query<&mut Walker, Without<RetiredEntity>>,
) {
    aim.yaw -= analog.look().x * LOOK_SENSITIVITY;
    let (move_x, move_z) = (analog.axis("walk_x"), analog.axis("walk_z"));
    let jump = actions.is_held("walk_jump");
    for mut walker in &mut walkers {
        if walker.owner != local.0 {
            continue;
        }
        if queue.accepting {
            queue.push(format!(
                "sandbox.walker_input {move_x} {move_z} {} {}",
                aim.yaw,
                u8::from(jump)
            ));
        } else {
            walker.move_x = move_x;
            walker.move_z = move_z;
            walker.yaw = aim.yaw;
            walker.jump = jump;
        }
    }
}

/// Moves every walker: the module sets the velocity it wants, with its own gravity; the engine's
/// stepping lays it along the ground.
fn move_walkers(
    time: Res<Time<Fixed>>,
    mut walkers: Query<
        (Entity, &mut Walker, &CharacterGround, &PhysicsVelocity),
        Without<RetiredEntity>,
    >,
    mut commands: Commands,
) {
    let dt = time.timestep().as_secs_f32();
    for (entity, mut walker, ground, velocity) in &mut walkers {
        walker.grounded = ground.grounded;
        let vertical = match (ground.grounded, walker.jump) {
            (true, true) => JUMP_SPEED,
            (true, false) => 0.0,
            (false, _) => velocity.linear.y - GRAVITY * dt,
        };
        let heading = Quat::from_rotation_y(walker.yaw);
        let horizontal =
            heading * Vec3::new(walker.move_x, 0.0, walker.move_z).clamp_length_max(1.0) * SPEED;
        commands.entity(entity).insert(PhysicsVelocitySet(Vec3::new(
            horizontal.x,
            vertical,
            horizontal.z,
        )));
    }
}

/// The color of each player's walker.
const OWNER_COLORS: [Color; 4] = [
    Color::srgb(0.9, 0.75, 0.2),
    Color::srgb(0.2, 0.7, 0.85),
    Color::srgb(0.85, 0.3, 0.55),
    Color::srgb(0.45, 0.8, 0.3),
];

/// Looks, on each machine: a client creates other players' walkers from the server's state, so
/// this runs on every new `Walker`. Other players are also drawn smoothly.
fn dress_walkers(
    walkers: Query<(Entity, &Walker), Added<Walker>>,
    local: Res<NetLocalClient>,
    mut commands: Commands,
) {
    for (entity, walker) in &walkers {
        let color = OWNER_COLORS[walker.owner.unwrap_or(0) as usize % OWNER_COLORS.len()];
        commands
            .entity(entity)
            .insert((EntityColor(color), CharacterVisual::default()));
        if local.0.is_some() && walker.owner != local.0 {
            // Drawn as the server saw it, which is what shots are judged against.
            commands.entity(entity).insert(RemoteInterpolation);
        }
    }
}
