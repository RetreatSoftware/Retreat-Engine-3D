//! Continuous input: mouse look, analog axes and cursor capture. The engine fills these from the
//! devices (`retreat_engine/src/analog_input.rs`); modules only read them.
//!
//! **Reaching the server.** Inputs are command lines. A client module reads [`AnalogInput`] in
//! `FixedUpdate` (`.after(InputActionSystems)`), turns it into numbers and pushes a command line into
//! [`crate::NetInputQueue`] each tick, for example `character.input <move_x> <move_z> <yaw> <pitch>
//! <buttons>`. The engine runs that same line on the client (prediction), on the server and in every
//! replay, so all see the same values (an `f32` printed with `{}` parses back to the same bits).
//! Send absolute view angles, not deltas: a lost input then costs nothing. The command validates
//! its numbers (finite, in range): `inf` and `NaN` parse as `f32`.
//!
//! **Look** is the mouse movement in raw counts since the previous fixed tick (x right, y down),
//! counted only while the cursor is captured (or added by the script command `input.look`). The
//! module owns sensitivity and the view angles.
//!
//! **Axes** are named values in -1..=1, declared with [`AppAxisExt::register_axis`] from two held
//! actions (negative and positive), so keys are rebound with `input.bind` as usual. Scripts and the
//! browser bridge override one with `input.set_axis`.

use std::collections::BTreeMap;

use bevy::prelude::*;

/// What a module declares for one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputAxis {
    /// Unique id, one word (`walk_x`).
    pub id: &'static str,
    /// The [`crate::InputAction`] that pushes towards -1.
    pub negative_action: &'static str,
    /// The [`crate::InputAction`] that pushes towards +1.
    pub positive_action: &'static str,
}

#[derive(Clone, Debug)]
struct AxisEntry {
    declared: InputAxis,
    value: f32,
    forced: Option<f32>,
}

/// Mouse look and axes of the current fixed tick (a resource).
#[derive(Resource, Default, Debug)]
pub struct AnalogInput {
    pending_look: Vec2,
    look: Vec2,
    axes: BTreeMap<&'static str, AxisEntry>,
}

impl AnalogInput {
    /// Mouse movement since the previous fixed tick.
    pub fn look(&self) -> Vec2 {
        self.look
    }

    /// The axis value in -1..=1; 0 for an unknown id.
    pub fn axis(&self, id: &str) -> f32 {
        self.axes.get(id).map_or(0.0, |axis| axis.value)
    }

    /// Every declared axis, in id order.
    pub fn declared_axes(&self) -> impl Iterator<Item = InputAxis> {
        self.axes.values().map(|axis| axis.declared)
    }

    /// # Panics
    /// If the id is empty, has spaces or is already registered.
    pub fn register(&mut self, axis: InputAxis) {
        assert!(
            !axis.id.is_empty() && !axis.id.contains(char::is_whitespace),
            "AnalogInput::register: invalid axis id `{}`; an id is one word like `walk_x`",
            axis.id
        );
        let entry = AxisEntry {
            declared: axis,
            value: 0.0,
            forced: None,
        };
        assert!(
            self.axes.insert(axis.id, entry).is_none(),
            "AnalogInput::register: axis `{}` is registered twice; axis ids must be unique",
            axis.id
        );
    }

    /// Adds mouse movement that the next tick reports. Called by the engine.
    pub fn add_look(&mut self, delta: Vec2) {
        self.pending_look += delta;
    }

    /// Overrides an axis until `None` is given.
    pub fn force_axis(&mut self, id: &str, value: Option<f32>) -> Result<(), String> {
        if let Some(value) = value
            && !(-1.0..=1.0).contains(&value)
        {
            return Err(format!("an axis value is between -1 and 1, got {value}"));
        }
        let ids: Vec<&str> = self.axes.keys().copied().collect();
        let Some(axis) = self.axes.get_mut(id) else {
            return Err(format!("no axis `{id}`; axes: {}", ids.join(", ")));
        };
        axis.forced = value;
        Ok(())
    }

    /// Starts a tick: the pending movement becomes [`AnalogInput::look`] and every axis is
    /// recomputed. Called by the engine once per live fixed tick.
    pub fn update(&mut self, is_action_held: impl Fn(&str) -> bool) {
        self.look = std::mem::take(&mut self.pending_look);
        for axis in self.axes.values_mut() {
            let held = |action| f32::from(u8::from(is_action_held(action)));
            axis.value = axis.forced.unwrap_or_else(|| {
                held(axis.declared.positive_action) - held(axis.declared.negative_action)
            });
        }
    }
}

/// Adds `register_axis` to Bevy's `App`.
pub trait AppAxisExt {
    /// Declares an axis. Its two actions must be registered by the time all plugins are built.
    fn register_axis(&mut self, axis: InputAxis) -> &mut Self;
}

impl AppAxisExt for App {
    fn register_axis(&mut self, axis: InputAxis) -> &mut Self {
        self.init_resource::<AnalogInput>();
        self.world_mut()
            .resource_mut::<AnalogInput>()
            .register(axis);
        self
    }
}

/// Cursor capture for mouse look (a resource): set [`CursorCapture::wanted`] (or run
/// `input.capture_cursor` / `input.release_cursor`); the engine hides and locks the cursor.
///
/// A browser grants the lock only after a click: while it is wanted and not granted, the next
/// click asks again. When the platform takes the lock away (Escape in a browser, the window losing
/// focus), the engine sets `wanted` back to false.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorCapture {
    /// What the game wants.
    pub wanted: bool,
    /// Whether the cursor is captured now (written by the engine). Without a window (headless) it
    /// follows `wanted`.
    pub captured: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALK: InputAxis = InputAxis {
        id: "walk_x",
        negative_action: "left",
        positive_action: "right",
    };

    #[test]
    fn look_is_reported_for_exactly_one_tick() {
        let mut input = AnalogInput::default();
        input.add_look(Vec2::new(3.0, 1.0));
        input.add_look(Vec2::new(1.0, 0.0));
        assert_eq!(input.look(), Vec2::ZERO);
        input.update(|_| false);
        assert_eq!(input.look(), Vec2::new(4.0, 1.0));
        input.update(|_| false);
        assert_eq!(input.look(), Vec2::ZERO);
    }

    #[test]
    fn an_axis_follows_its_actions_unless_forced() {
        let mut input = AnalogInput::default();
        input.register(WALK);
        input.update(|action| action == "left");
        assert_eq!(input.axis("walk_x"), -1.0);
        input.update(|_| true);
        assert_eq!(input.axis("walk_x"), 0.0);
        input.force_axis("walk_x", Some(0.5)).unwrap();
        input.update(|_| false);
        assert_eq!(input.axis("walk_x"), 0.5);
        input.force_axis("walk_x", None).unwrap();
        input.update(|_| false);
        assert_eq!(input.axis("walk_x"), 0.0);
        assert!(input.force_axis("walk_x", Some(2.0)).is_err());
        assert!(
            input
                .force_axis("nope", None)
                .unwrap_err()
                .contains("walk_x")
        );
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn an_axis_id_is_unique() {
        let mut input = AnalogInput::default();
        input.register(WALK);
        input.register(WALK);
    }
}
