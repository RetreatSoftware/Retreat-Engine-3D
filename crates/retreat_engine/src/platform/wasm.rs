//! Browser (wasm32) side of the platform layer. See `mod.rs` for the contract.
//!
//! **The bridge.** JavaScript drives the engine through four functions exported with
//! `wasm-bindgen`; `web/index.html` gathers them into `window.__retreat`:
//! - `retreatCommand(line) -> Promise`: runs one command line, like a script line. Resolves with
//!   `"ok"` once the command ran and any work it started (snapshot, model load) has finished;
//!   rejects with the error message. Refused while a launch script is running.
//! - `retreatStatus() -> string`: JSON from `bridge_status::script_status_json`.
//! - `retreatSnapshotPng(name) -> Uint8Array | undefined`: the PNG a `snapshot <name>` stored.
//! - `retreatSnapshotNames() -> string[]`.
//!
//! The browser runs everything on one thread, so the shared state lives in a `thread_local`.
//! JavaScript only queues requests; a Bevy system serves them (one per frame), so commands
//! always run inside the app's own frame, never in the middle of a system.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use bevy::asset::AssetMetaCheck;
use bevy::prelude::*;
use js_sys::{Array, Function, Promise, Uint8Array};
use retreat_engine_api::run_command_line;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[path = "wasm_transport.rs"]
pub mod wasm_transport;

use crate::bridge_status::script_status_json;
use crate::command_script::ScriptProgress;
use crate::{CommandScript, LaunchOptions, PendingCommandWork, RetreatEnginePlugin};

/// Browsers keep audio silent until the first click or key press on the page; `web/index.html`
/// resumes the page's audio contexts then.
pub(crate) const AUDIO_NEEDS_USER_GESTURE: bool = true;

pub(crate) const HAS_FILE_SYSTEM: bool = false;

pub(crate) const SCRIPT_END_EXITS_APP: bool = false;

/// What `retreatStatus` says before the app published its first status (the script is still
/// being fetched, or the first frame has not run).
const STARTING_STATUS: &str = r#"{"script":"starting"}"#;

/// The canvas the game draws into; `web/index.html` provides it.
const CANVAS_SELECTOR: &str = "#retreat-canvas";

thread_local! {
    static BRIDGE: RefCell<BridgeState> = RefCell::new(BridgeState::default());
}

/// Everything JavaScript and the app share.
#[derive(Default)]
struct BridgeState {
    /// Command lines from `retreatCommand`, oldest first.
    requests: VecDeque<BridgeRequest>,
    /// The request whose command ran but whose work (snapshot, load) is still in flight.
    waiting_for_work: Option<BridgeRequest>,
    /// PNGs stored by `snapshot <name>`.
    snapshots: BTreeMap<String, Vec<u8>>,
    /// The latest `retreatStatus` JSON; `None` until the app publishes one.
    status: Option<String>,
}

/// One `retreatCommand` call: the line and the Promise's resolve / reject functions.
struct BridgeRequest {
    line: String,
    resolve: Function,
    reject: Function,
}

/// Reads the page URL (`?commands=...&seed=...`), fetches the script, then builds and runs the
/// app. Fetching is asynchronous, so the app starts a moment after `main` returns; `main`'s
/// return value means nothing in the browser.
pub fn launch(window_title: &str, add_game_plugins: fn(&mut App)) -> AppExit {
    let title = window_title.to_string();
    wasm_bindgen_futures::spawn_local(async move {
        match load_engine_plugin(&title).await {
            Ok((engine, options)) => {
                let mut app = App::new();
                app.add_plugins(engine);
                add_game_plugins(&mut app);
                if let Err(message) = wasm_transport::install_net(&mut app, &options.net) {
                    let message = format!("{title}: {message}");
                    web_sys::console::error_1(&JsValue::from_str(&message));
                    set_status(script_status_json(Some(&ScriptProgress::Failed(message))));
                    return;
                }
                app.run();
            }
            Err(message) => {
                let message = format!("{title}: {message}");
                web_sys::console::error_1(&JsValue::from_str(&message));
                set_status(script_status_json(Some(&ScriptProgress::Failed(message))));
            }
        }
    });
    AppExit::Success
}

async fn load_engine_plugin(title: &str) -> Result<(RetreatEnginePlugin, LaunchOptions), String> {
    let window = web_sys::window().ok_or("there is no browser window")?;
    let query = window.location().search().map_err(describe_js_error)?;
    let options = LaunchOptions::from_url_query_pairs(url_query_pairs(&query)?)?;
    let script = match &options.commands_file {
        Some(path) => {
            let path = path.to_string_lossy().to_string();
            let text = fetch_text(&window, &path).await?;
            Some(CommandScript::from_text(path, &text))
        }
        None => None,
    };
    let engine = RetreatEnginePlugin::from_launch_options(title, &options, script);
    Ok((engine, options))
}

/// The decoded `(key, value)` pairs of a URL query such as `?commands=a.cmds&seed=3`.
fn url_query_pairs(query: &str) -> Result<Vec<(String, String)>, String> {
    let params = web_sys::UrlSearchParams::new_with_str(query).map_err(describe_js_error)?;
    let entries = js_sys::try_iter(params.as_ref())
        .map_err(describe_js_error)?
        .ok_or("URL query parameters are not iterable")?;
    let mut pairs = Vec::new();
    for entry in entries {
        let entry: Array = entry.map_err(describe_js_error)?.unchecked_into();
        let key = entry.get(0).as_string().unwrap_or_default();
        let value = entry.get(1).as_string().unwrap_or_default();
        pairs.push((key, value));
    }
    Ok(pairs)
}

/// Fetches a text file relative to the page (the command script).
async fn fetch_text(window: &web_sys::Window, path: &str) -> Result<String, String> {
    let response = JsFuture::from(window.fetch_with_str(path))
        .await
        .map_err(|error| {
            format!(
                "cannot fetch command script `{path}`: {}",
                describe_js_error(error)
            )
        })?;
    let response: web_sys::Response = response.unchecked_into();
    if !response.ok() {
        return Err(format!(
            "cannot fetch command script `{path}`: HTTP {} (paths are relative to the page)",
            response.status()
        ));
    }
    let text = JsFuture::from(response.text().map_err(describe_js_error)?)
        .await
        .map_err(describe_js_error)?;
    text.as_string()
        .ok_or_else(|| format!("command script `{path}` is not text"))
}

fn describe_js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

/// The game draws into the page's canvas and fills its parent element.
pub(crate) fn primary_window(title: &str) -> Window {
    Window {
        title: title.to_string(),
        canvas: Some(CANVAS_SELECTOR.to_string()),
        fit_canvas_to_parent: true,
        ..default()
    }
}

/// The browser would request a `.meta` file for every asset and get a 404 for each; the sandbox
/// has none.
pub(crate) fn asset_meta_check() -> AssetMetaCheck {
    AssetMetaCheck::Never
}

/// Keeps the PNG in memory for `retreatSnapshotPng(name)`.
pub(crate) fn store_snapshot(name: &str, png: Vec<u8>) -> Result<String, String> {
    BRIDGE.with_borrow_mut(|bridge| bridge.snapshots.insert(name.to_string(), png));
    Ok(format!("browser memory (__retreat.snapshot(\"{name}\"))"))
}

fn set_status(status: String) {
    BRIDGE.with_borrow_mut(|bridge| bridge.status = Some(status));
}

/// Serves bridge requests and publishes the script status.
pub(crate) struct PlatformPlugin;

impl Plugin for PlatformPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, serve_bridge_requests)
            .add_systems(Last, publish_script_status);
    }
}

/// Runs at most one queued `retreatCommand` per frame, and settles its Promise: at once, or,
/// when the command started work, on the first frame that work is done.
fn serve_bridge_requests(world: &mut World) {
    let waiting = BRIDGE.with_borrow_mut(|bridge| bridge.waiting_for_work.take());
    if let Some(request) = waiting {
        if world.resource::<PendingCommandWork>().is_busy() {
            BRIDGE.with_borrow_mut(|bridge| bridge.waiting_for_work = Some(request));
            return;
        }
        match world.resource_mut::<PendingCommandWork>().take_failure() {
            Some(failure) => reject(&request, &failure),
            None => resolve(&request),
        }
        return;
    }
    let Some(request) = BRIDGE.with_borrow_mut(|bridge| bridge.requests.pop_front()) else {
        return;
    };
    if let Some(script) = world.get_resource::<CommandScript>() {
        let busy = world.resource::<PendingCommandWork>().is_busy();
        if let ScriptProgress::Running { .. } = script.progress(busy) {
            reject(
                &request,
                "a launch script is running; wait until __retreat.status() reports it finished",
            );
            return;
        }
    }
    match run_command_line(world, &request.line) {
        Err(error) => reject(&request, &error.to_string()),
        Ok(_) if world.resource::<PendingCommandWork>().is_busy() => {
            BRIDGE.with_borrow_mut(|bridge| bridge.waiting_for_work = Some(request));
        }
        Ok(_) => resolve(&request),
    }
}

fn resolve(request: &BridgeRequest) {
    // Promise callbacks run later (as microtasks), never inside this call.
    let _ = request
        .resolve
        .call1(&JsValue::NULL, &JsValue::from_str("ok"));
}

fn reject(request: &BridgeRequest, message: &str) {
    let error = js_sys::Error::new(&format!("`{}`: {message}", request.line));
    let _ = request.reject.call1(&JsValue::NULL, &error);
}

/// Publishes the launch script's progress for `retreatStatus` (every frame; cheap).
fn publish_script_status(world: &mut World) {
    let busy = world.resource::<PendingCommandWork>().is_busy();
    let progress = world
        .get_resource::<CommandScript>()
        .map(|script| script.progress(busy));
    set_status(script_status_json(progress.as_ref()));
}

/// `__retreat.command(line)`: see the module docs.
#[wasm_bindgen(js_name = retreatCommand)]
pub fn retreat_command(line: String) -> Promise {
    let mut line = Some(line);
    Promise::new(&mut |resolve, reject| {
        let line = line.take().unwrap_or_default();
        BRIDGE.with_borrow_mut(|bridge| {
            bridge.requests.push_back(BridgeRequest {
                line,
                resolve,
                reject,
            });
        });
    })
}

/// `__retreat.status()`: see `bridge_status.rs`.
#[wasm_bindgen(js_name = retreatStatus)]
pub fn retreat_status() -> String {
    BRIDGE.with_borrow(|bridge| {
        bridge
            .status
            .clone()
            .unwrap_or_else(|| STARTING_STATUS.to_string())
    })
}

/// The PNG bytes stored by `snapshot <name>`, if any.
#[wasm_bindgen(js_name = retreatSnapshotPng)]
pub fn retreat_snapshot_png(name: String) -> Option<Uint8Array> {
    BRIDGE.with_borrow(|bridge| {
        bridge
            .snapshots
            .get(&name)
            .map(|png| Uint8Array::from(png.as_slice()))
    })
}

/// Names of all stored snapshots, sorted.
#[wasm_bindgen(js_name = retreatSnapshotNames)]
pub fn retreat_snapshot_names() -> Array {
    BRIDGE.with_borrow(|bridge| {
        bridge
            .snapshots
            .keys()
            .map(|name| JsValue::from_str(name))
            .collect()
    })
}

/// What `save.status` tells the player about where saves live.
pub(crate) const SAVE_LOCATION: &str =
    "the browser's localStorage (keys start with `retreat.save.`)";

/// The save backend of this platform: the page's `localStorage`.
pub(crate) fn platform_save_backend() -> Box<dyn retreat_engine_api::SaveBackend> {
    Box::new(LocalStorageSaveBackend)
}

/// Saves in `localStorage` under `retreat.save.<key>`. `localStorage` holds text and belongs to
/// the page's origin, so saves survive reloads but are not shared between sites. It can be
/// unavailable (private windows, blocked site data): every call then returns an error that says
/// so, never a panic.
pub(crate) struct LocalStorageSaveBackend;

const LOCAL_STORAGE_PREFIX: &str = "retreat.save.";

fn local_storage() -> Result<web_sys::Storage, String> {
    web_sys::window()
        .ok_or("there is no browser window")?
        .local_storage()
        .map_err(describe_js_error)?
        .ok_or_else(|| {
            "localStorage is not available (private window or blocked site data?)".to_string()
        })
}

impl retreat_engine_api::SaveBackend for LocalStorageSaveBackend {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        local_storage()?
            .get_item(&format!("{LOCAL_STORAGE_PREFIX}{key}"))
            .map_err(describe_js_error)
    }

    fn write(&mut self, key: &str, text: &str) -> Result<(), String> {
        local_storage()?
            .set_item(&format!("{LOCAL_STORAGE_PREFIX}{key}"), text)
            .map_err(|error| {
                format!(
                    "cannot store `{key}` (storage full?): {}",
                    describe_js_error(error)
                )
            })
    }

    fn remove(&mut self, key: &str) -> Result<bool, String> {
        let storage = local_storage()?;
        let full_key = format!("{LOCAL_STORAGE_PREFIX}{key}");
        let existed = storage
            .get_item(&full_key)
            .map_err(describe_js_error)?
            .is_some();
        storage.remove_item(&full_key).map_err(describe_js_error)?;
        Ok(existed)
    }

    fn keys(&self) -> Result<Vec<String>, String> {
        let storage = local_storage()?;
        let count = storage.length().map_err(describe_js_error)?;
        let mut keys = Vec::new();
        for index in 0..count {
            if let Some(full_key) = storage.key(index).map_err(describe_js_error)?
                && let Some(key) = full_key.strip_prefix(LOCAL_STORAGE_PREFIX)
            {
                keys.push(key.to_string());
            }
        }
        keys.sort();
        Ok(keys)
    }
}
