//! The platform layer: the only place in the engine that checks `target_arch = "wasm32"`.
//! Other modules call these items instead of writing their own `cfg` checks.
//!
//! `native.rs` and `wasm.rs` provide the same items with the same signatures; exactly one of
//! them is compiled. Each provides:
//! - `HAS_FILE_SYSTEM`: native builds can read and write files; the browser cannot.
//! - `SCRIPT_END_EXITS_APP`: on native, `quit` and a failed script exit the process (so the
//!   exit code reaches CI); in the browser the page stays up so `__retreat.status()` can be read.
//! - `launch(window_title, add_game_plugins)`: reads launch options (process arguments or the
//!   page URL), loads the command script (file or HTTP fetch), builds and runs the app.
//! - `primary_window(title)`: the window settings (a canvas element in the browser).
//! - `asset_meta_check()`: whether the asset server looks for `.meta` files.
//! - `store_snapshot(name, png)`: keeps a snapshot (a file, or memory for the bridge).
//! - `PlatformPlugin`: platform-only systems (the browser bridge `window.__retreat`).

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::launch;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{
    HAS_FILE_SYSTEM, PlatformPlugin, SCRIPT_END_EXITS_APP, asset_meta_check, primary_window,
    store_snapshot,
};

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::launch;
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{
    HAS_FILE_SYSTEM, PlatformPlugin, SCRIPT_END_EXITS_APP, asset_meta_check, primary_window,
    store_snapshot,
};
