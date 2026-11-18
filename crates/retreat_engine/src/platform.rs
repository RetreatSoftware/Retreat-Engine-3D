//! The platform layer: the only place in the engine that checks `target_arch = "wasm32"`.
//! Other modules read these facts instead of writing their own `cfg` checks.

/// Native builds can read and write files; the browser has no file system.
pub(crate) const HAS_FILE_SYSTEM: bool = cfg!(not(target_arch = "wasm32"));
