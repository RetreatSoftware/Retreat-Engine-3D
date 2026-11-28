//! Browser text fields: the Rust half of `window.__retreatText` (`web/index.html`).
//!
//! Only a real page `<input>` opens a phone's on-screen keyboard and receives IME text, so the
//! page keeps one input over each shown text field (positioned from the panel layout, transparent
//! text, visible caret) and the Bevy widget, read-only here, only draws what it holds. Each frame:
//!
//! 1. `poll()` returns what the player did since the last frame, as JSON: `{kind: "text", key,
//!    text}` (the input's whole value), `{kind: "submit", key}` (Enter), `{kind: "focus", key}`,
//!    `{kind: "blur", key}`. `key` is `<panel>/<field>`.
//! 2. `sync(json)` sends the shown fields (`key`, `left`, `top`, `width`, `height` in logical
//!    pixels of the canvas, `text`, `maxLength`, `focused`), only when something changed. The page
//!    creates, moves and removes inputs to match, sets a value the game changed (a cleared chat
//!    line) and gives the page focus to the field the game focused (`ui.focus_field`).
//!
//! Events of one frame are all applied before the state is sent, and JavaScript cannot run in
//! between, so the page never receives text older than what it reported.

use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use retreat_engine_api::serde_json::{self, Value, json};
use wasm_bindgen::prelude::*;

use crate::ui_text_fields::{PanelTextField, PendingSubmits, replace_widget_text, widget_text};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__retreatText"], js_name = sync)]
    fn retreat_text_sync(fields_json: &str);
    #[wasm_bindgen(js_namespace = ["window", "__retreatText"], js_name = poll)]
    fn retreat_text_poll() -> String;
}

fn field_key(field: &PanelTextField) -> String {
    format!("{}/{}", field.panel, field.field.id)
}

/// Applies the page's events, then sends the fields' state (see the module docs).
pub(crate) fn sync_page_text_fields(
    mut fields: Query<(
        Entity,
        &PanelTextField,
        &mut EditableText,
        &ComputedNode,
        &UiGlobalTransform,
    )>,
    mut focus: ResMut<InputFocus>,
    mut pending: ResMut<PendingSubmits>,
    mut last_sent: Local<String>,
) {
    let events: Vec<Value> = serde_json::from_str(&retreat_text_poll()).unwrap_or_default();
    for event in events {
        let key = event.get("key").and_then(Value::as_str).unwrap_or("");
        let Some((entity, field, mut editable, ..)) = fields
            .iter_mut()
            .find(|(_, field, ..)| field_key(field) == key)
        else {
            continue;
        };
        match event.get("kind").and_then(Value::as_str) {
            Some("text") => {
                let text = event.get("text").and_then(Value::as_str).unwrap_or("");
                replace_widget_text(&field.field, &mut editable, text);
            }
            Some("submit") => pending.0.push((field.panel, field.field.id)),
            Some("focus") => focus.set(entity, FocusCause::Pressed),
            Some("blur") if focus.get() == Some(entity) => focus.clear(),
            _ => {}
        }
    }
    let shown: Vec<Value> = fields
        .iter()
        .map(|(entity, field, editable, node, transform)| {
            // Bevy keeps UI sizes in physical pixels; the page wants logical (CSS) pixels.
            let scale = node.inverse_scale_factor();
            let size = node.size() * scale;
            let center = transform.translation * scale;
            json!({
                "key": field_key(field),
                "left": center.x - size.x / 2.0,
                "top": center.y - size.y / 2.0,
                "width": size.x,
                "height": size.y,
                "text": widget_text(editable),
                "maxLength": field.field.max_chars,
                "focused": focus.get() == Some(entity),
            })
        })
        .collect();
    let state = Value::Array(shown).to_string();
    if *last_sent != state {
        retreat_text_sync(&state);
        *last_sent = state;
    }
}
