//! Text fields of UI panels, engine side (decided 2026-11-27, crawler plan CE2; contract in
//! `retreat_engine_api/src/ui_panel.rs`). `ui_panels.rs` places them; this file makes and runs them.
//!
//! A field is Bevy's own text input widget (`bevy::ui_widgets::TextInput` on an `EditableText`):
//! natively it takes the keyboard, IME composition, selection and clipboard as Bevy does. **In a
//! browser** (`platform::TEXT_FIELDS_ON_PAGE`) the page puts a real `<input>` over each shown
//! field (`web/index.html`, `window.__retreatText`), because only a real input element opens a
//! phone's on-screen keyboard and receives IME text there; the Bevy widget is then read-only and
//! only draws what the page input holds (so snapshots show it).
//!
//! The widget is the truth for what a field holds; `mirror_text_fields` copies it into the
//! contract's `UiTextFields` (and which field has the focus) after Bevy applied the frame's edits,
//! so modules and assertions read it there. Focus is Bevy's `InputFocus`.
//!
//! Submitting (Enter on the focused field, or `ui.submit_field`) runs
//! `<submit_command> <text>` through `dispatch_command_line` on the next `Update`, never inside
//! another command. An empty field submits nothing.
//!
//! Commands: `ui.focus_field <panel> <field>`, `ui.unfocus_field`, `ui.type_text <text...>` (types
//! into the focused field, as a keyboard would), `ui.set_field <panel> <field> [text...]`,
//! `ui.submit_field <panel> <field>`, `assert.ui_field <panel> <field> [text...]`,
//! `assert.ui_focused <panel> <field>`, `assert.ui_not_focused`.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input_focus::{FocusCause, FocusedInput, InputFocus};
use bevy::picking::events::PointerPress;
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextSystems, TextEdit, TextReadWriteMode};
use bevy::ui_widgets::TextInput;
use retreat_engine_api::{
    AppCommandExt, CommandArgs, CommandError, CommandOutcome, CommandResult, PanelRect, UiPanels,
    UiTextField, UiTextFields, fit_text_field, text_field_submit_line,
};

use crate::command_dispatch::dispatch_command_line;
use crate::platform;

const FIELD_COLOR: Color = Color::srgb(0.08, 0.09, 0.11);
const FIELD_FOCUSED_COLOR: Color = Color::srgb(0.12, 0.14, 0.20);
const PLACEHOLDER_COLOR: Color = Color::srgb(0.55, 0.57, 0.62);
const FIELD_TEXT_SIZE: f32 = 16.0;
/// Space between a field's edge and its text, in logical pixels.
const FIELD_PADDING: f32 = 8.0;

/// Adds text field behavior and the commands.
pub struct UiTextFieldsPlugin;

impl Plugin for UiTextFieldsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiTextFields>()
            .init_resource::<InputFocus>()
            .init_resource::<PendingSubmits>()
            .register_command(
                "ui.focus_field",
                "ui.focus_field <panel> <field>",
                focus_command,
            )
            .register_command("ui.unfocus_field", "ui.unfocus_field", unfocus_command)
            .register_command("ui.type_text", "ui.type_text <text...>", type_command)
            .register_command(
                "ui.set_field",
                "ui.set_field <panel> <field> [text...]",
                set_command,
            )
            .register_command(
                "ui.submit_field",
                "ui.submit_field <panel> <field>",
                submit_command,
            )
            .register_command(
                "assert.ui_field",
                "assert.ui_field <panel> <field> [text...]",
                assert_field_command,
            )
            .register_command(
                "assert.ui_focused",
                "assert.ui_focused <panel> <field>",
                assert_focused_command,
            )
            .register_command(
                "assert.ui_not_focused",
                "assert.ui_not_focused",
                assert_not_focused_command,
            )
            // App-wide observers (fields are rebuilt with their panels; see `ui_panels.rs`).
            .add_observer(focus_field_on_press)
            .add_observer(submit_field_on_enter)
            .add_systems(
                Update,
                (submit_pending_fields, platform::sync_page_text_fields),
            )
            .add_systems(
                PostUpdate,
                (mirror_text_fields, show_placeholders_and_focus)
                    .chain()
                    .after(EditableTextSystems),
            );
    }
}

/// A panel's text field widget.
#[derive(Component, Clone, Copy)]
pub(crate) struct PanelTextField {
    pub(crate) panel: &'static str,
    pub(crate) field: UiTextField,
}

/// The grey hint drawn over an empty field.
#[derive(Component, Clone, Copy)]
struct FieldPlaceholder {
    panel: &'static str,
    field: &'static str,
}

/// Fields asked to submit (Enter or `ui.submit_field`), handled on the next `Update`: by panel and
/// field id, not by entity, because the panel may be rebuilt in between.
#[derive(Resource, Default)]
pub(crate) struct PendingSubmits(pub(crate) Vec<(&'static str, &'static str)>);

/// Entity name of a field (the same namespace as the panel's buttons: ids are unique per panel).
pub(crate) fn panel_field_name(panel: &str, field: &str) -> String {
    format!("ui/{panel}/{field}")
}

/// Spawns one field (and its placeholder) under a panel root at `rect`, holding `text`. Returns
/// the field's entity.
pub(crate) fn spawn_text_field(
    root: &mut ChildSpawnerCommands,
    panel: &'static str,
    field: UiTextField,
    rect: PanelRect,
    text: &str,
) -> Entity {
    let mut editable = EditableText::new(fit_text_field(&field, text));
    editable.max_characters = Some(field.max_chars);
    let mode = if platform::TEXT_FIELDS_ON_PAGE {
        // The page's <input> edits; the widget only draws.
        TextReadWriteMode::ReadOnly
    } else {
        TextReadWriteMode::Editable
    };
    let node = Node {
        position_type: PositionType::Absolute,
        left: px(rect.left),
        top: px(rect.top),
        width: px(rect.width),
        height: px(rect.height),
        padding: UiRect::axes(px(FIELD_PADDING), px(FIELD_PADDING)),
        ..default()
    };
    let entity = root
        .spawn((
            Name::new(panel_field_name(panel, field.id)),
            PanelTextField { panel, field },
            crate::UiClickTarget,
            TextInput,
            editable,
            mode,
            TextFont::from_font_size(FIELD_TEXT_SIZE),
            TextColor(Color::WHITE),
            node.clone(),
            BackgroundColor(FIELD_COLOR),
        ))
        .id();
    root.spawn((
        FieldPlaceholder {
            panel,
            field: field.id,
        },
        node,
        Pickable::IGNORE,
        Visibility::Hidden,
    ))
    .with_child((
        Text::new(field.placeholder),
        TextFont::from_font_size(FIELD_TEXT_SIZE),
        TextColor(PLACEHOLDER_COLOR),
        Pickable::IGNORE,
    ));
    entity
}

/// A click (or tap) on a field gives it the keyboard focus.
fn focus_field_on_press(
    press: On<PointerPress>,
    fields: Query<(), With<PanelTextField>>,
    mut focus: ResMut<InputFocus>,
) {
    if fields.contains(press.entity) {
        focus.set(press.entity, FocusCause::Pressed);
    }
}

/// Enter on the focused field submits it.
fn submit_field_on_enter(
    input: On<FocusedInput<KeyboardInput>>,
    fields: Query<&PanelTextField>,
    mut pending: ResMut<PendingSubmits>,
) {
    let Ok(field) = fields.get(input.focused_entity) else {
        return;
    };
    if input.input.state.is_pressed() && input.input.logical_key == Key::Enter {
        pending.0.push((field.panel, field.field.id));
    }
}

/// The text a field widget holds now.
pub(crate) fn widget_text(editable: &EditableText) -> String {
    editable.value().to_string()
}

/// Replaces what a field holds (cut to what it can hold), caret at the end.
pub(crate) fn replace_widget_text(field: &UiTextField, editable: &mut EditableText, text: &str) {
    let fitted = fit_text_field(field, text);
    editable.editor.set_text(&fitted);
    editable.queue_edit(TextEdit::TextEnd(false));
}

/// Runs each submitted field's command (outside any command handler).
fn submit_pending_fields(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<PendingSubmits>().0);
    for (panel, field_id) in pending {
        let mut fields = world.query::<(Entity, &PanelTextField)>();
        let Some((entity, field)) = fields
            .iter(world)
            .find(|(_, field)| field.panel == panel && field.field.id == field_id)
            .map(|(entity, field)| (entity, *field))
        else {
            warn!(
                "text field `{field_id}` of panel `{panel}` is no longer shown; nothing submitted"
            );
            continue;
        };
        let Some(mut editable) = world.get_mut::<EditableText>(entity) else {
            continue;
        };
        let text = widget_text(&editable);
        if field.field.clear_on_submit {
            replace_widget_text(&field.field, &mut editable, "");
        }
        let Some(line) = text_field_submit_line(&field.field, &text) else {
            continue;
        };
        let source = format!("text field `{}` of panel `{}`", field.field.id, field.panel);
        dispatch_command_line(world, &source, &line);
    }
}

/// Copies every shown field's text and the focus into `UiTextFields` (only when they changed, so
/// readers see a change only when there is one). Fields of hidden panels keep their last text.
fn mirror_text_fields(
    fields: Query<(Entity, &PanelTextField, &EditableText)>,
    focus: Res<InputFocus>,
    mut state: ResMut<UiTextFields>,
) {
    let mut texts = state.texts.clone();
    let mut focused = None;
    for (entity, field, editable) in &fields {
        texts.insert((field.panel, field.field.id), widget_text(editable));
        if focus.get() == Some(entity) {
            focused = Some((field.panel, field.field.id));
        }
    }
    if state.texts != texts {
        state.texts = texts;
    }
    if state.focused != focused {
        state.focused = focused;
    }
}

/// Shows a field's hint while it is empty and marks the focused field.
fn show_placeholders_and_focus(
    state: Res<UiTextFields>,
    mut placeholders: Query<(&FieldPlaceholder, &mut Visibility)>,
    mut backgrounds: Query<(&PanelTextField, &mut BackgroundColor)>,
) {
    for (placeholder, mut visibility) in &mut placeholders {
        let wanted = if state.text(placeholder.panel, placeholder.field).is_empty() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        visibility.set_if_neq(wanted);
    }
    for (field, mut color) in &mut backgrounds {
        let wanted = if state.focused == Some((field.panel, field.field.id)) {
            FIELD_FOCUSED_COLOR
        } else {
            FIELD_COLOR
        };
        if color.0 != wanted {
            color.0 = wanted;
        }
    }
}

/// The entity of a shown field, or an error that says why there is none.
fn field_entity(
    world: &mut World,
    args: &CommandArgs,
) -> Result<(Entity, UiTextField), CommandError> {
    let panel: String = args.parse(0, "panel")?;
    let field_id: String = args.parse(1, "field")?;
    let field = world
        .resource::<UiPanels>()
        .text_field(&panel, &field_id)
        .map_err(|message| args.failure(message))?;
    let name = panel_field_name(&panel, &field_id);
    let mut fields = world.query_filtered::<(Entity, &Name), With<PanelTextField>>();
    fields
        .iter(world)
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| (entity, field))
        .ok_or_else(|| {
            args.failure(format!(
                "text field `{field_id}` of panel `{panel}` is not shown (the panel is hidden, \
                 or the UI is not built yet)"
            ))
        })
}

/// The words of the command from `from` on, joined by single spaces (the text of a field).
fn text_argument(args: &CommandArgs, from: usize) -> String {
    args.rest(from).join(" ")
}

/// `ui.focus_field <panel> <field>`: gives the field the keyboard focus, as a click would.
fn focus_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let (entity, _) = field_entity(world, args)?;
    world
        .resource_mut::<InputFocus>()
        .set(entity, FocusCause::Navigated);
    Ok(CommandOutcome::Continue)
}

/// `ui.unfocus_field`: no field has the keyboard focus (keys work as input actions again).
fn unfocus_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    world.resource_mut::<InputFocus>().clear();
    Ok(CommandOutcome::Continue)
}

/// `ui.type_text <text...>`: adds the text to the focused field, as typing would (at most what
/// the field can hold).
fn type_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let typed = text_argument(args, 0);
    if typed.is_empty() {
        return Err(args.failure("nothing to type; give the text after the command"));
    }
    let Some(entity) = world.resource::<InputFocus>().get() else {
        return Err(args.failure(
            "no text field has the keyboard focus; use `ui.focus_field <panel> <field>` first",
        ));
    };
    let Some(field) = world.get::<PanelTextField>(entity).copied() else {
        return Err(args.failure("the focused element is not a text field"));
    };
    let mut editable = world
        .get_mut::<EditableText>(entity)
        .expect("a text field has an EditableText");
    let text = widget_text(&editable) + &typed;
    replace_widget_text(&field.field, &mut editable, &text);
    Ok(CommandOutcome::Continue)
}

/// `ui.set_field <panel> <field> [text...]`: replaces what the field holds (no text empties it).
fn set_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    if args.rest(0).len() < 2 {
        return Err(args.failure("expected a panel and a field, then the text (none empties it)"));
    }
    let (entity, field) = field_entity(world, args)?;
    let text = text_argument(args, 2);
    let mut editable = world
        .get_mut::<EditableText>(entity)
        .expect("a text field has an EditableText");
    replace_widget_text(&field, &mut editable, &text);
    Ok(CommandOutcome::Continue)
}

/// `ui.submit_field <panel> <field>`: submits the field as Enter would; its command runs on this
/// frame's `Update`. An empty field is an error here (it would submit nothing).
fn submit_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let (entity, field) = field_entity(world, args)?;
    let text = world
        .get::<EditableText>(entity)
        .map(widget_text)
        .unwrap_or_default();
    if text_field_submit_line(&field, &text).is_none() {
        return Err(args.failure(format!(
            "text field `{}` is empty, so there is nothing to submit; type into it first",
            field.id
        )));
    }
    let panel = world
        .get::<PanelTextField>(entity)
        .expect("found as a text field")
        .panel;
    world
        .resource_mut::<PendingSubmits>()
        .0
        .push((panel, field.id));
    Ok(CommandOutcome::Continue)
}

/// `assert.ui_field <panel> <field> [text...]`: what the field holds (no text: it is empty).
fn assert_field_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    if args.rest(0).len() < 2 {
        return Err(args.failure("expected a panel and a field, then the text (none: empty)"));
    }
    let (entity, field) = field_entity(world, args)?;
    let expected = text_argument(args, 2);
    let actual = world
        .get::<EditableText>(entity)
        .map(widget_text)
        .unwrap_or_default();
    if actual == expected {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "text field `{}` holds `{actual}`, expected `{expected}`",
            field.id
        )))
    }
}

/// `assert.ui_focused <panel> <field>`.
fn assert_focused_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(2)?;
    let (entity, field) = field_entity(world, args)?;
    if world.resource::<InputFocus>().get() == Some(entity) {
        Ok(CommandOutcome::Continue)
    } else {
        Err(args.failure(format!(
            "text field `{}` does not have the keyboard focus",
            field.id
        )))
    }
}

/// `assert.ui_not_focused`: no text field has the keyboard focus.
fn assert_not_focused_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    args.expect_count(0)?;
    let focused = world.resource::<InputFocus>().get();
    match focused.and_then(|entity| world.get::<PanelTextField>(entity).copied()) {
        None => Ok(CommandOutcome::Continue),
        Some(field) => Err(args.failure(format!(
            "text field `{}` of panel `{}` has the keyboard focus",
            field.field.id, field.panel
        ))),
    }
}
