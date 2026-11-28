//! The platform layer: the only place in the engine that checks `target_arch = "wasm32"`.
//! Other modules call these items instead of writing their own `cfg` checks.
//!
//! `native.rs` and `wasm.rs` provide the same items with the same signatures; exactly one of
//! them is compiled. Each provides:
//! - `AUDIO_NEEDS_USER_GESTURE`: browsers keep audio silent until the player clicks or presses a
//!   key on the page (`web/index.html` resumes the audio contexts then); native output starts at once.
//! - `SAVE_LOCATION` and `platform_save_backend()`: where saves live (files natively, `localStorage`
//!   in the browser) and the `SaveBackend` that stores them.
//! - `TEXT_FIELDS_ON_PAGE` and `sync_page_text_fields`: the browser puts a real page `<input>` over
//!   each shown text field (on-screen keyboards and IME need one); natively Bevy's widget types.
//! - `platform_record_backend(folder)`: the server records as files (native; the browser has no
//!   file system and answers with an error, see `LaunchOptions::record_location`).
//! - `HAS_FILE_SYSTEM`: native builds can read and write files; the browser cannot.
//! - `SCRIPT_END_EXITS_APP`: on native, `quit` and a failed script exit the process (so the
//!   exit code reaches CI); in the browser the page stays up so `__retreat.status()` can be read.
//! - `launch(window_title, add_game_plugins)`: reads launch options (process arguments or the
//!   page URL), loads the command script (file or HTTP fetch), builds and runs the app.
//! - `primary_window(title)`: the window settings (a canvas element in the browser).
//! - `asset_meta_check()`: whether the asset server looks for `.meta` files.
//! - `store_snapshot(name, png)`: keeps a snapshot (a file, or memory for the bridge).
//! - `pointer_is_locked()`: whether the browser holds the pointer lock; `None` natively, where the
//!   window grants every request.
//! - `PlatformPlugin`: platform-only systems (the browser bridge `window.__retreat`).

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::launch;
#[cfg(not(target_arch = "wasm32"))]
pub use native::native_transport::{
    WebTransportClient, WebTransportServer, parse_certificate_hash,
};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::{
    AUDIO_NEEDS_USER_GESTURE, HAS_FILE_SYSTEM, PlatformPlugin, SAVE_LOCATION, SCRIPT_END_EXITS_APP,
    TEXT_FIELDS_ON_PAGE, asset_meta_check, platform_record_backend, platform_save_backend,
    pointer_is_locked, primary_window, store_snapshot, sync_page_text_fields,
};

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::launch;
#[cfg(target_arch = "wasm32")]
pub(crate) use wasm::{
    AUDIO_NEEDS_USER_GESTURE, HAS_FILE_SYSTEM, PlatformPlugin, SAVE_LOCATION, SCRIPT_END_EXITS_APP,
    TEXT_FIELDS_ON_PAGE, asset_meta_check, platform_record_backend, platform_save_backend,
    pointer_is_locked, primary_window, store_snapshot, sync_page_text_fields,
};
