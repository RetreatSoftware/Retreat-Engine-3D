//! Saving: a small key/value store modules use to keep their own data between runs, without
//! knowing where it lives.
//!
//! Each module owns **one JSON document**, saved under its module id. The engine decides where
//! the document goes: a file on native builds, the browser's `localStorage` on the web (both
//! behind the engine's platform layer), memory under tests and command scripts. A module only
//! sees [`SaveStore`].
//!
//! **Format (explicit and versioned).** What is stored is this JSON text:
//!
//! ```json
//! { "format": 1, "module": "highscore", "version": 3, "data": { "score": 120 } }
//! ```
//!
//! - `format` is the engine's save format ([`SAVE_FORMAT_VERSION`]). A document with a newer
//!   format is refused with [`SaveError::FormatTooNew`] instead of being half read.
//! - `module` must equal the id it was saved under (a copied or renamed file is caught).
//! - `version` is the **module's own** data version. The module chooses it and migrates older
//!   data itself when [`SavedData::version`] is lower than its current one; the engine never
//!   interprets `data`.
//!
//! Saving is local convenience. Server authority is unchanged: games never mint currency or
//! change balances, and nothing in a save is trusted by the platform.

use std::collections::BTreeMap;
use std::fmt;

pub use serde_json;
use serde_json::Value;

use bevy::prelude::*;

/// The save format this engine writes and reads.
pub const SAVE_FORMAT_VERSION: u32 = 1;

/// Where saved documents physically live. The engine implements it for files and for the
/// browser; [`MemorySaveBackend`] is the in-memory one. Keys are module ids.
pub trait SaveBackend: Send + Sync {
    /// The stored text for `key`, or `None` if nothing is stored.
    fn read(&self, key: &str) -> Result<Option<String>, String>;
    /// Stores `text` under `key`, replacing what was there.
    fn write(&mut self, key: &str, text: &str) -> Result<(), String>;
    /// Removes `key`; `true` if something was stored.
    fn remove(&mut self, key: &str) -> Result<bool, String>;
    /// Every stored key, sorted.
    fn keys(&self) -> Result<Vec<String>, String>;
}

/// A backend that keeps everything in memory (tests, command scripts, headless runs).
#[derive(Default, Debug)]
pub struct MemorySaveBackend {
    documents: BTreeMap<String, String>,
}

impl SaveBackend for MemorySaveBackend {
    fn read(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.documents.get(key).cloned())
    }

    fn write(&mut self, key: &str, text: &str) -> Result<(), String> {
        self.documents.insert(key.to_string(), text.to_string());
        Ok(())
    }

    fn remove(&mut self, key: &str) -> Result<bool, String> {
        Ok(self.documents.remove(key).is_some())
    }

    fn keys(&self) -> Result<Vec<String>, String> {
        Ok(self.documents.keys().cloned().collect())
    }
}

/// What went wrong with a save or load. Every message says where and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The module id is empty or has characters other than letters, digits, `_` and `-`.
    InvalidModuleId(String),
    /// The backend (file system, browser storage) failed.
    Backend { module: String, message: String },
    /// The stored text is not a valid save document.
    Corrupt { module: String, reason: String },
    /// The document was written by a newer engine.
    FormatTooNew {
        module: String,
        found: u32,
        supported: u32,
    },
    /// The module's data is newer than the module that tries to read it.
    VersionTooNew {
        module: String,
        found: u32,
        supported: u32,
    },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::InvalidModuleId(id) => write!(
                f,
                "`{id}` is not a valid module id for saving; use letters, digits, `_` and `-` only"
            ),
            SaveError::Backend { module, message } => {
                write!(
                    f,
                    "saving for module `{module}` failed in the storage backend: {message}"
                )
            }
            SaveError::Corrupt { module, reason } => write!(
                f,
                "the save of module `{module}` is damaged ({reason}); delete it with \
                 `save.delete {module}` or fix the stored text"
            ),
            SaveError::FormatTooNew {
                module,
                found,
                supported,
            } => write!(
                f,
                "the save of module `{module}` has format {found}, but this engine reads up to \
                 {supported}; update the engine"
            ),
            SaveError::VersionTooNew {
                module,
                found,
                supported,
            } => write!(
                f,
                "the data of module `{module}` is version {found}, but this module reads up to \
                 {supported}; update the module"
            ),
        }
    }
}

impl std::error::Error for SaveError {}

/// One loaded document.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedData {
    /// The module's own data version, as saved.
    pub version: u32,
    /// The module's data; the engine never looks inside.
    pub data: Value,
}

impl SavedData {
    /// Fails with [`SaveError::VersionTooNew`] when the data is newer than `current`, the newest
    /// version the module understands. Older data is returned for the module to migrate.
    pub fn check_not_newer_than(&self, module: &str, current: u32) -> Result<(), SaveError> {
        if self.version > current {
            Err(SaveError::VersionTooNew {
                module: module.to_string(),
                found: self.version,
                supported: current,
            })
        } else {
            Ok(())
        }
    }
}

/// Module ids are used as file names and storage keys: letters, digits, `_` and `-` only.
pub fn is_valid_module_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The text to store for a module's document (see the module docs for the shape).
pub fn encode_save(module: &str, version: u32, data: &Value) -> String {
    let document = serde_json::json!({
        "format": SAVE_FORMAT_VERSION,
        "module": module,
        "version": version,
        "data": data,
    });
    document.to_string()
}

/// Reads stored text back. Pure, so every failure is unit tested.
pub fn decode_save(module: &str, text: &str) -> Result<SavedData, SaveError> {
    let corrupt = |reason: String| SaveError::Corrupt {
        module: module.to_string(),
        reason,
    };
    let document: Value =
        serde_json::from_str(text).map_err(|error| corrupt(format!("not valid JSON: {error}")))?;
    let field_u32 = |name: &str| -> Result<u32, SaveError> {
        document
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| corrupt(format!("`{name}` is missing or not a whole number")))
    };
    let format = field_u32("format")?;
    if format > SAVE_FORMAT_VERSION {
        return Err(SaveError::FormatTooNew {
            module: module.to_string(),
            found: format,
            supported: SAVE_FORMAT_VERSION,
        });
    }
    let stored_module = document
        .get("module")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupt("`module` is missing or not text".to_string()))?;
    if stored_module != module {
        return Err(corrupt(format!(
            "it belongs to module `{stored_module}`, not `{module}`"
        )));
    }
    let version = field_u32("version")?;
    let data = document
        .get("data")
        .cloned()
        .ok_or_else(|| corrupt("`data` is missing".to_string()))?;
    Ok(SavedData { version, data })
}

/// The app's save store (a Bevy resource). The engine creates it; modules call it.
#[derive(Resource)]
pub struct SaveStore {
    backend: Box<dyn SaveBackend>,
}

impl SaveStore {
    /// A store on `backend`.
    pub fn new(backend: Box<dyn SaveBackend>) -> Self {
        Self { backend }
    }

    /// A store in memory (tests and scripts).
    pub fn in_memory() -> Self {
        Self::new(Box::<MemorySaveBackend>::default())
    }

    /// Switches to another backend. Data in the old one stays there.
    pub fn replace_backend(&mut self, backend: Box<dyn SaveBackend>) {
        self.backend = backend;
    }

    /// Saves `data` as module `module`'s document, with the module's data `version`.
    pub fn save(&mut self, module: &str, version: u32, data: &Value) -> Result<(), SaveError> {
        check_module_id(module)?;
        self.backend
            .write(module, &encode_save(module, version, data))
            .map_err(|message| backend_error(module, message))
    }

    /// The module's document, or `None` if it never saved.
    pub fn load(&self, module: &str) -> Result<Option<SavedData>, SaveError> {
        check_module_id(module)?;
        let text = self
            .backend
            .read(module)
            .map_err(|message| backend_error(module, message))?;
        text.map(|text| decode_save(module, &text)).transpose()
    }

    /// Deletes the module's document; `true` if there was one.
    pub fn delete(&mut self, module: &str) -> Result<bool, SaveError> {
        check_module_id(module)?;
        self.backend
            .remove(module)
            .map_err(|message| backend_error(module, message))
    }

    /// Ids of every module that has a document, sorted.
    pub fn modules(&self) -> Result<Vec<String>, SaveError> {
        self.backend
            .keys()
            .map_err(|message| backend_error("(all)", message))
    }
}

fn check_module_id(module: &str) -> Result<(), SaveError> {
    if is_valid_module_id(module) {
        Ok(())
    } else {
        Err(SaveError::InvalidModuleId(module.to_string()))
    }
}

fn backend_error(module: &str, message: String) -> SaveError {
    SaveError::Backend {
        module: module.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_saved_document_loads_back_with_its_version() {
        let mut store = SaveStore::in_memory();
        store.save("highscore", 2, &json!({"score": 120})).unwrap();
        let loaded = store.load("highscore").unwrap().expect("saved");
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.data, json!({"score": 120}));
    }

    #[test]
    fn the_stored_text_has_the_documented_shape() {
        let text = encode_save("highscore", 3, &json!({"score": 1}));
        let document: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            document,
            json!({"format": 1, "module": "highscore", "version": 3, "data": {"score": 1}})
        );
    }

    #[test]
    fn nothing_saved_loads_as_none_and_delete_reports_it() {
        let mut store = SaveStore::in_memory();
        assert_eq!(store.load("a").unwrap(), None);
        store.save("a", 1, &json!(null)).unwrap();
        assert_eq!(store.delete("a"), Ok(true));
        assert_eq!(store.delete("a"), Ok(false));
        assert_eq!(store.load("a").unwrap(), None);
    }

    #[test]
    fn modules_lists_the_saved_ids_sorted() {
        let mut store = SaveStore::in_memory();
        store.save("zed", 1, &json!(1)).unwrap();
        store.save("alpha", 1, &json!(2)).unwrap();
        assert_eq!(store.modules().unwrap(), vec!["alpha", "zed"]);
    }

    #[test]
    fn module_ids_must_be_safe_for_file_names() {
        let mut store = SaveStore::in_memory();
        for bad in ["", "a b", "../x", "a/b", "ünï"] {
            let error = store.save(bad, 1, &json!(1)).unwrap_err();
            assert_eq!(error, SaveError::InvalidModuleId(bad.to_string()));
            assert!(error.to_string().contains("letters, digits"));
        }
        assert!(is_valid_module_id("fox_walk-2"));
    }

    #[test]
    fn damaged_documents_are_reported_with_the_reason() {
        let cases = [
            ("not json", "not valid JSON"),
            (
                r#"{"module":"m","version":1,"data":1}"#,
                "`format` is missing",
            ),
            (
                r#"{"format":1,"version":1,"data":1}"#,
                "`module` is missing",
            ),
            (
                r#"{"format":1,"module":"m","data":1}"#,
                "`version` is missing",
            ),
            (
                r#"{"format":1,"module":"m","version":1}"#,
                "`data` is missing",
            ),
            (
                r#"{"format":1,"module":"other","version":1,"data":1}"#,
                "belongs to module `other`",
            ),
            (
                r#"{"format":"x","module":"m","version":1,"data":1}"#,
                "`format` is missing or not a whole number",
            ),
        ];
        for (text, reason) in cases {
            let error = decode_save("m", text).unwrap_err();
            let message = error.to_string();
            assert!(message.contains(reason), "{text}: {message}");
            assert!(message.contains("save.delete m"), "{message}");
        }
    }

    #[test]
    fn a_newer_format_is_refused() {
        let text = r#"{"format":2,"module":"m","version":1,"data":1}"#;
        let error = decode_save("m", text).unwrap_err();
        assert_eq!(
            error,
            SaveError::FormatTooNew {
                module: "m".into(),
                found: 2,
                supported: 1
            }
        );
        assert!(error.to_string().contains("update the engine"));
    }

    #[test]
    fn a_module_reading_newer_data_is_refused_but_older_data_is_returned() {
        let mut store = SaveStore::in_memory();
        store.save("m", 5, &json!(1)).unwrap();
        let saved = store.load("m").unwrap().unwrap();
        assert_eq!(saved.check_not_newer_than("m", 5), Ok(()));
        assert_eq!(saved.check_not_newer_than("m", 9), Ok(()));
        let error = saved.check_not_newer_than("m", 4).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("version 5, but this module reads up to 4")
        );
    }

    #[test]
    fn a_failing_backend_names_the_module() {
        struct Broken;
        impl SaveBackend for Broken {
            fn read(&self, _: &str) -> Result<Option<String>, String> {
                Err("disk unplugged".into())
            }
            fn write(&mut self, _: &str, _: &str) -> Result<(), String> {
                Err("disk unplugged".into())
            }
            fn remove(&mut self, _: &str) -> Result<bool, String> {
                Err("disk unplugged".into())
            }
            fn keys(&self) -> Result<Vec<String>, String> {
                Err("disk unplugged".into())
            }
        }
        let mut store = SaveStore::new(Box::new(Broken));
        let error = store.save("m", 1, &json!(1)).unwrap_err();
        assert!(error.to_string().contains("module `m`"));
        assert!(error.to_string().contains("disk unplugged"));
        assert!(store.load("m").is_err());
    }
}
