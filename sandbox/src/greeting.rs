//! A demo of UI text fields (crawler plan CE2): a hidden panel `greeting` at the top right with a
//! name field and a "Greet" button; submitting (Enter, the button, or `ui.submit_field greeting
//! name`) runs `sandbox.greet <name>`, which shows "Hello, <name>!" in a HUD line. Checked by
//! `scripts/verify/text_field.cmds`.

use bevy::prelude::*;
use retreat_engine_api::{
    AppCommandExt, AppHudExt, AppUiExt, CommandArgs, CommandOutcome, CommandResult, HudAnchor,
    HudKind, HudWidget, PanelAnchor, PanelDirection, UiButtonEntry, UiHud, UiPanel, UiTextField,
};

const GREETING_PANEL: &str = "greeting";
const GREETING_WIDGET: &str = "greeting_text";

const GREETING_FIELDS: &[UiTextField] = &[UiTextField {
    id: "name",
    placeholder: "Your name",
    max_chars: 12,
    submit_command: "sandbox.greet",
    clear_on_submit: false,
}];

const GREETING_ENTRIES: &[UiButtonEntry] = &[UiButtonEntry {
    id: "greet",
    label: "Greet",
    command_line: "ui.submit_field greeting name",
}];

pub struct GreetingPlugin;

impl Plugin for GreetingPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(UiPanel {
            id: GREETING_PANEL,
            title: Some("Greeting"),
            anchor: PanelAnchor::TopRight,
            direction: PanelDirection::Column,
            entries: GREETING_ENTRIES,
            visible: false,
        })
        .register_text_fields(GREETING_PANEL, GREETING_FIELDS)
        .register_hud_widget(HudWidget {
            id: GREETING_WIDGET,
            anchor: HudAnchor::BottomCenter,
            kind: HudKind::Text { font_size: 28.0 },
            visible: true,
        })
        .register_command("sandbox.greet", "sandbox.greet <name...>", greet_command);
    }
}

/// `sandbox.greet <name...>`: shows the greeting.
fn greet_command(world: &mut World, args: &CommandArgs) -> CommandResult {
    let name = args.rest(0).join(" ");
    if name.is_empty() {
        return Err(args.failure("give a name to greet"));
    }
    world
        .resource_mut::<UiHud>()
        .set_text(GREETING_WIDGET, format!("Hello, {name}!"))
        .map_err(|message| args.failure(message))?;
    Ok(CommandOutcome::Continue)
}
