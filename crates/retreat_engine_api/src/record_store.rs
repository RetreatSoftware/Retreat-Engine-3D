//! Server records: the persistent store of an authoritative game server (accounts, characters,
//! later chests, balances, market listings). Decided 2026-11-27 (crawler plan, CE1).
//!
//! **Where it exists.** Only where the game's state is decided: a server, or a game without a
//! network. **Never on a network client**: the engine removes [`RecordStore`] when it installs a
//! client, so a module that finds no store is on a client and must not decide anything there.
//! Natively the store is a folder of files (`records/` in the working directory, or
//! `--records-dir <dir>`); in a browser game without a network it is in memory (lost on reload);
//! under a command script it is in memory unless the launch names `--records-dir`.
//!
//! **Keys.** A record is one versioned JSON document at a [`RecordKey`]: an **owner** (what the
//! record is about: `account/alice`, `character/alice-1`) and a **part** (the module id that owns
//! the document: `account`, `player_spawn`). Every module saves only its own part of an owner.
//! Owners are one to four segments of lowercase letters, digits, `_` and `-` joined by `/`; parts
//! are one such segment. Both become file and folder names, so nothing else is allowed.
//!
//! **Atomic writes.** [`RecordStore::write`] applies a [`RecordBatch`] (puts and deletes, also
//! "every part of this owner") completely or not at all, also across a crash: a file backend
//! writes the batch to a journal first and finishes it on the next start. Anything that must
//! change two records together (create a character and its name entry, a trade) is one batch.
//!
//! **Several servers, one store** (decided 2026-11-28, crawler plan: the town chest is one per
//! account across all worlds, and each world is its own server process). Every world server of a
//! game opens the same records, so two servers may change one record (an account's chest, the
//! account's character list, the item id counter) at the same moment. Two rules keep that safe:
//! a backend applies a commit and reads only while it holds the store's lock (the file backend
//! locks `<folder>/.lock`, shared by every process), and a batch may carry **expectations**
//! ([`RecordBatch::expect_unchanged`]): "this record is still exactly what I read" (or still
//! absent). If another server changed it in between, the whole batch is refused with
//! [`RecordError::Conflict`] and nothing is written; the module reads again and decides again.
//! A record only one server writes (a character bound to one world) needs no expectation.
//!
//! **Format** of one stored document (the engine reads only the envelope):
//!
//! ```json
//! { "format": 1, "owner": "character/alice-1", "part": "player_spawn", "version": 1, "data": {} }
//! ```
//!
//! `version` is the owning module's data version; it migrates older data itself
//! ([`StoredRecord::check_not_newer_than`]).

use std::collections::BTreeMap;
use std::fmt;

use bevy::prelude::*;
use serde_json::Value;

/// The record format this engine writes and reads.
pub const RECORD_FORMAT_VERSION: u32 = 1;

/// Most segments an owner may have (`kind/name/...`).
pub const MAX_OWNER_SEGMENTS: usize = 4;

/// Most characters in one segment of an owner or in a part.
pub const MAX_RECORD_SEGMENT_CHARS: usize = 64;

/// Where one record lives: what it is about (`owner`) and which module's document it is (`part`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordKey {
    pub owner: String,
    pub part: String,
}

impl RecordKey {
    pub fn new(owner: impl Into<String>, part: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            part: part.into(),
        }
    }
}

impl fmt::Display for RecordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.owner, self.part)
    }
}

/// One segment of an owner, or a part: lowercase letters, digits, `_` and `-`, 1 to
/// [`MAX_RECORD_SEGMENT_CHARS`] characters.
pub fn is_valid_record_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_RECORD_SEGMENT_CHARS
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// An owner: 1 to [`MAX_OWNER_SEGMENTS`] valid segments joined by `/`.
pub fn is_valid_record_owner(owner: &str) -> bool {
    let segments: Vec<&str> = owner.split('/').collect();
    segments.len() <= MAX_OWNER_SEGMENTS && segments.iter().all(|s| is_valid_record_segment(s))
}

/// One change a backend applies, already encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordChange {
    /// Store `text` at `key`, replacing what was there.
    Put { key: RecordKey, text: String },
    /// Remove `key` (removing a missing record is fine).
    Delete { key: RecordKey },
}

/// "The record at `key` is still exactly `expected` (`None`: still absent)", checked by a backend
/// at the moment of a commit ([`RecordBatch::expect_unchanged`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordExpectation {
    pub key: RecordKey,
    pub expected: Option<StoredRecord>,
}

impl RecordExpectation {
    /// Whether `stored` (the text at the key now, as a backend holds it) is what was expected. A
    /// damaged record never matches. Pure; every backend calls it under its lock.
    pub fn holds(&self, stored: Option<&str>) -> bool {
        match (stored, &self.expected) {
            (None, None) => true,
            (Some(text), Some(expected)) => {
                decode_record(&self.key, text).is_ok_and(|now| now == *expected)
            }
            _ => false,
        }
    }
}

/// Why a backend did not commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordCommitError {
    /// An expectation did not hold (another server changed the record): nothing was written.
    Conflict(RecordKey),
    /// The storage failed (the message says how).
    Backend(String),
}

/// Where records physically live. The engine has a file backend (native),
/// [`MemoryRecordBackend`] and [`SharedMemoryRecordBackend`]. Keys reaching a backend are always
/// valid. A backend that several servers share applies [`RecordBackend::commit`] and reads under
/// one lock that all of them respect (see "Several servers, one store" in the module docs).
pub trait RecordBackend: Send + Sync {
    /// The stored text at `key`, or `None`.
    fn read(&self, key: &RecordKey) -> Result<Option<String>, String>;
    /// The parts stored for `owner`, sorted.
    fn parts(&self, owner: &str) -> Result<Vec<String>, String>;
    /// When every expectation holds, applies every change or none of them, in order (a later
    /// change to the same key wins); checking and applying happen under the store's lock.
    fn commit(
        &mut self,
        expected: &[RecordExpectation],
        changes: &[RecordChange],
    ) -> Result<(), RecordCommitError>;
    /// Where the records are, for messages (`records/` or `memory`).
    fn location(&self) -> String;
}

/// Records in memory (tests, command scripts, a browser game without a network).
#[derive(Default, Debug, Clone)]
pub struct MemoryRecordBackend {
    records: BTreeMap<RecordKey, String>,
}

impl RecordBackend for MemoryRecordBackend {
    fn read(&self, key: &RecordKey) -> Result<Option<String>, String> {
        Ok(self.records.get(key).cloned())
    }

    fn parts(&self, owner: &str) -> Result<Vec<String>, String> {
        Ok(self
            .records
            .keys()
            .filter(|key| key.owner == owner)
            .map(|key| key.part.clone())
            .collect())
    }

    fn commit(
        &mut self,
        expected: &[RecordExpectation],
        changes: &[RecordChange],
    ) -> Result<(), RecordCommitError> {
        let now = |key: &RecordKey| self.records.get(key).map(String::as_str);
        if let Some(failed) = expected
            .iter()
            .find(|expectation| !expectation.holds(now(&expectation.key)))
        {
            return Err(RecordCommitError::Conflict(failed.key.clone()));
        }
        // Nothing can fail half way in memory, so applying in order is all-or-nothing.
        for change in changes {
            match change {
                RecordChange::Put { key, text } => {
                    self.records.insert(key.clone(), text.clone());
                }
                RecordChange::Delete { key } => {
                    self.records.remove(key);
                }
            }
        }
        Ok(())
    }

    fn location(&self) -> String {
        "memory".to_string()
    }
}

/// Records in memory that several stores share, like several world servers on one record folder
/// (tests of modules whose records more than one server writes). Clones share the same records;
/// the mutex is the store's lock.
#[derive(Default, Debug, Clone)]
pub struct SharedMemoryRecordBackend {
    records: std::sync::Arc<std::sync::Mutex<MemoryRecordBackend>>,
}

impl SharedMemoryRecordBackend {
    fn locked(&self) -> std::sync::MutexGuard<'_, MemoryRecordBackend> {
        // A memory commit cannot stop half way, so even a lock poisoned by a panic holds whole
        // records.
        self.records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl RecordBackend for SharedMemoryRecordBackend {
    fn read(&self, key: &RecordKey) -> Result<Option<String>, String> {
        self.locked().read(key)
    }

    fn parts(&self, owner: &str) -> Result<Vec<String>, String> {
        self.locked().parts(owner)
    }

    fn commit(
        &mut self,
        expected: &[RecordExpectation],
        changes: &[RecordChange],
    ) -> Result<(), RecordCommitError> {
        self.locked().commit(expected, changes)
    }

    fn location(&self) -> String {
        "shared memory".to_string()
    }
}

/// What went wrong with a record. Every message names the record and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    /// The owner or the part is not a valid name (see the module docs).
    InvalidKey { owner: String, part: String },
    /// The backend (file system) failed.
    Backend { location: String, message: String },
    /// The stored text is not a valid record.
    Corrupt { key: RecordKey, reason: String },
    /// The record was written by a newer engine.
    FormatTooNew { key: RecordKey, found: u32 },
    /// The record's data is newer than the module reading it.
    VersionTooNew {
        key: RecordKey,
        found: u32,
        supported: u32,
    },
    /// The batch expected the record unchanged ([`RecordBatch::expect_unchanged`]), but another
    /// server changed it since it was read: nothing was written.
    Conflict { key: RecordKey },
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::InvalidKey { owner, part } => write!(
                f,
                "record owner `{owner}` / part `{part}` is not valid; an owner is 1 to \
                 {MAX_OWNER_SEGMENTS} segments joined by `/` and a part is one segment, each of \
                 lowercase letters, digits, `_` and `-` (at most {MAX_RECORD_SEGMENT_CHARS})"
            ),
            RecordError::Backend { location, message } => {
                write!(f, "the record store in {location} failed: {message}")
            }
            RecordError::Corrupt { key, reason } => write!(
                f,
                "record `{key}` is damaged ({reason}); delete it with \
                 `records.delete {} {}` or fix the stored text",
                key.owner, key.part
            ),
            RecordError::FormatTooNew { key, found } => write!(
                f,
                "record `{key}` has format {found}, but this engine reads up to \
                 {RECORD_FORMAT_VERSION}; update the engine"
            ),
            RecordError::VersionTooNew {
                key,
                found,
                supported,
            } => write!(
                f,
                "record `{key}` holds data version {found}, but its module reads up to \
                 {supported}; update the module"
            ),
            RecordError::Conflict { key } => write!(
                f,
                "record `{key}` was changed by another server since it was read, so nothing was \
                 written; read it again and decide again"
            ),
        }
    }
}

impl std::error::Error for RecordError {}

/// One record as read.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRecord {
    /// The owning module's data version, as written.
    pub version: u32,
    /// The module's data; the engine never looks inside.
    pub data: Value,
}

impl StoredRecord {
    /// Fails with [`RecordError::VersionTooNew`] when the data is newer than `current`.
    pub fn check_not_newer_than(&self, key: &RecordKey, current: u32) -> Result<(), RecordError> {
        if self.version > current {
            Err(RecordError::VersionTooNew {
                key: key.clone(),
                found: self.version,
                supported: current,
            })
        } else {
            Ok(())
        }
    }
}

/// The text stored for one record (see the module docs for the shape).
pub fn encode_record(key: &RecordKey, version: u32, data: &Value) -> String {
    serde_json::json!({
        "format": RECORD_FORMAT_VERSION,
        "owner": key.owner,
        "part": key.part,
        "version": version,
        "data": data,
    })
    .to_string()
}

/// Reads stored text back. Pure, so every failure is unit tested.
pub fn decode_record(key: &RecordKey, text: &str) -> Result<StoredRecord, RecordError> {
    let corrupt = |reason: String| RecordError::Corrupt {
        key: key.clone(),
        reason,
    };
    let document: Value =
        serde_json::from_str(text).map_err(|error| corrupt(format!("not valid JSON: {error}")))?;
    let field_u32 = |name: &str| -> Result<u32, RecordError> {
        document
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| corrupt(format!("`{name}` is missing or not a whole number")))
    };
    let format = field_u32("format")?;
    if format > RECORD_FORMAT_VERSION {
        return Err(RecordError::FormatTooNew {
            key: key.clone(),
            found: format,
        });
    }
    for (field, expected) in [("owner", &key.owner), ("part", &key.part)] {
        let stored = document
            .get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| corrupt(format!("`{field}` is missing or not text")))?;
        if stored != expected {
            return Err(corrupt(format!(
                "its `{field}` is `{stored}`, not `{expected}` (copied or moved file?)"
            )));
        }
    }
    let version = field_u32("version")?;
    let data = document
        .get("data")
        .cloned()
        .ok_or_else(|| corrupt("`data` is missing".to_string()))?;
    Ok(StoredRecord { version, data })
}

/// One step of a [`RecordBatch`], before encoding.
#[derive(Debug, Clone, PartialEq)]
enum BatchStep {
    Put {
        key: RecordKey,
        version: u32,
        data: Value,
    },
    Delete {
        key: RecordKey,
    },
    DeleteOwner {
        owner: String,
    },
}

/// Changes that happen together or not at all ([`RecordStore::write`]). Steps apply in order.
/// Expectations are checked first; one that fails refuses the whole batch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordBatch {
    steps: Vec<BatchStep>,
    expectations: Vec<RecordExpectation>,
}

impl RecordBatch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores `data` (the module's data `version`) at `key`.
    pub fn put(mut self, key: RecordKey, version: u32, data: Value) -> Self {
        self.steps.push(BatchStep::Put { key, version, data });
        self
    }

    /// Removes the record at `key` (a missing one is fine).
    pub fn delete(mut self, key: RecordKey) -> Self {
        self.steps.push(BatchStep::Delete { key });
        self
    }

    /// Removes every part of `owner` (a character that is deleted loses every module's record).
    pub fn delete_owner(mut self, owner: impl Into<String>) -> Self {
        self.steps.push(BatchStep::DeleteOwner {
            owner: owner.into(),
        });
        self
    }

    /// Writes the batch only if the record at `key` is still `expected` (what this server read;
    /// `None`: still absent). Use it for every record another server may also change.
    pub fn expect_unchanged(mut self, key: RecordKey, expected: Option<StoredRecord>) -> Self {
        self.expectations.push(RecordExpectation { key, expected });
        self
    }

    /// True when the batch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// The server's records (a Bevy resource; see the module docs for where it exists).
#[derive(Resource)]
pub struct RecordStore {
    backend: Box<dyn RecordBackend>,
}

impl RecordStore {
    pub fn new(backend: Box<dyn RecordBackend>) -> Self {
        Self { backend }
    }

    /// A store in memory (tests and scripts).
    pub fn in_memory() -> Self {
        Self::new(Box::<MemoryRecordBackend>::default())
    }

    /// Switches to another backend. Records in the old one stay there.
    pub fn replace_backend(&mut self, backend: Box<dyn RecordBackend>) {
        self.backend = backend;
    }

    /// Where the records are, for messages.
    pub fn location(&self) -> String {
        self.backend.location()
    }

    /// The record at `key`, or `None`.
    pub fn read(&self, key: &RecordKey) -> Result<Option<StoredRecord>, RecordError> {
        check_key(key)?;
        let text = self
            .backend
            .read(key)
            .map_err(|message| self.backend_error(message))?;
        text.map(|text| decode_record(key, &text)).transpose()
    }

    /// The parts stored for `owner`, sorted.
    pub fn parts(&self, owner: &str) -> Result<Vec<String>, RecordError> {
        check_key(&RecordKey::new(owner, "parts"))?;
        self.backend
            .parts(owner)
            .map_err(|message| self.backend_error(message))
    }

    /// Applies the whole batch or nothing (see the module docs).
    pub fn write(&mut self, batch: RecordBatch) -> Result<(), RecordError> {
        for expectation in &batch.expectations {
            check_key(&expectation.key)?;
        }
        let mut changes = Vec::new();
        for step in batch.steps {
            match step {
                BatchStep::Put { key, version, data } => {
                    check_key(&key)?;
                    let text = encode_record(&key, version, &data);
                    changes.push(RecordChange::Put { key, text });
                }
                BatchStep::Delete { key } => {
                    check_key(&key)?;
                    changes.push(RecordChange::Delete { key });
                }
                BatchStep::DeleteOwner { owner } => {
                    for part in self.parts(&owner)? {
                        changes.push(RecordChange::Delete {
                            key: RecordKey::new(owner.clone(), part),
                        });
                    }
                }
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        match self.backend.commit(&batch.expectations, &changes) {
            Ok(()) => Ok(()),
            Err(RecordCommitError::Conflict(key)) => Err(RecordError::Conflict { key }),
            Err(RecordCommitError::Backend(message)) => Err(self.backend_error(message)),
        }
    }

    /// Writes one record (a batch of one).
    pub fn put(&mut self, key: RecordKey, version: u32, data: Value) -> Result<(), RecordError> {
        self.write(RecordBatch::new().put(key, version, data))
    }

    fn backend_error(&self, message: String) -> RecordError {
        RecordError::Backend {
            location: self.backend.location(),
            message,
        }
    }
}

fn check_key(key: &RecordKey) -> Result<(), RecordError> {
    if is_valid_record_owner(&key.owner) && is_valid_record_segment(&key.part) {
        Ok(())
    } else {
        Err(RecordError::InvalidKey {
            owner: key.owner.clone(),
            part: key.part.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(owner: &str, part: &str) -> RecordKey {
        RecordKey::new(owner, part)
    }

    #[test]
    fn owners_and_parts_are_lowercase_segments() {
        assert!(is_valid_record_owner("account/alice"));
        assert!(is_valid_record_owner("character/alice-1"));
        assert!(is_valid_record_owner("world"));
        assert!(!is_valid_record_owner("account/Alice"));
        assert!(!is_valid_record_owner("account//alice"));
        assert!(!is_valid_record_owner("../etc"));
        assert!(!is_valid_record_owner("a/b/c/d/e"));
        assert!(!is_valid_record_segment(""));
        assert!(!is_valid_record_segment("a b"));
        assert!(!is_valid_record_segment(
            &"x".repeat(MAX_RECORD_SEGMENT_CHARS + 1)
        ));
    }

    #[test]
    fn a_record_round_trips_and_its_envelope_is_checked() {
        let at = key("character/alice-1", "player_spawn");
        let text = encode_record(&at, 2, &json!({"x": 1}));
        let record = decode_record(&at, &text).unwrap();
        assert_eq!(record.version, 2);
        assert_eq!(record.data, json!({"x": 1}));
        let moved = decode_record(&key("character/bob-1", "player_spawn"), &text);
        assert!(matches!(moved, Err(RecordError::Corrupt { .. })));
        let newer = text.replace("\"format\":1", "\"format\":9");
        assert!(matches!(
            decode_record(&at, &newer),
            Err(RecordError::FormatTooNew { found: 9, .. })
        ));
        assert!(matches!(
            decode_record(&at, "{"),
            Err(RecordError::Corrupt { .. })
        ));
        let error = record.check_not_newer_than(&at, 1).unwrap_err();
        assert!(error.to_string().contains("reads up to 1"));
    }

    #[test]
    fn a_batch_applies_puts_and_deletes_together() {
        let mut store = RecordStore::in_memory();
        store
            .write(
                RecordBatch::new()
                    .put(key("account/alice", "account"), 1, json!({"n": 1}))
                    .put(key("character/alice-1", "account"), 1, json!({}))
                    .put(key("character/alice-1", "player_spawn"), 1, json!({})),
            )
            .unwrap();
        assert_eq!(
            store.parts("character/alice-1").unwrap(),
            ["account", "player_spawn"]
        );
        store
            .write(RecordBatch::new().delete_owner("character/alice-1").put(
                key("account/alice", "account"),
                1,
                json!({"n": 2}),
            ))
            .unwrap();
        assert!(store.parts("character/alice-1").unwrap().is_empty());
        let account = store.read(&key("account/alice", "account")).unwrap();
        assert_eq!(account.unwrap().data, json!({"n": 2}));
    }

    #[test]
    fn a_batch_whose_expectation_another_server_broke_writes_nothing() {
        let shared = SharedMemoryRecordBackend::default();
        let mut world_one = RecordStore::new(Box::new(shared.clone()));
        let mut world_two = RecordStore::new(Box::new(shared));
        let chest = key("account/alice", "town_chest");
        let other = key("character/alice-1", "bag");
        assert!(world_one.read(&chest).unwrap().is_none());
        // World two writes the chest after world one read it (absent).
        world_two
            .write(
                RecordBatch::new()
                    .expect_unchanged(chest.clone(), None)
                    .put(chest.clone(), 1, json!({"items": [1]})),
            )
            .unwrap();
        let stale = world_one.write(
            RecordBatch::new()
                .expect_unchanged(chest.clone(), None)
                .put(chest.clone(), 1, json!({"items": [2]}))
                .put(other.clone(), 1, json!({})),
        );
        let error = stale.unwrap_err();
        assert_eq!(error, RecordError::Conflict { key: chest.clone() });
        assert!(error.to_string().contains("read it again"), "{error}");
        assert!(world_one.read(&other).unwrap().is_none(), "nothing written");
        // Read again, decide again: now it holds.
        let now = world_one.read(&chest).unwrap();
        assert_eq!(now.as_ref().unwrap().data, json!({"items": [1]}));
        world_one
            .write(RecordBatch::new().expect_unchanged(chest.clone(), now).put(
                chest.clone(),
                1,
                json!({"items": [1, 2]}),
            ))
            .unwrap();
        assert_eq!(
            world_two.read(&chest).unwrap().unwrap().data,
            json!({"items": [1, 2]})
        );
    }

    #[test]
    fn an_invalid_key_refuses_the_whole_batch() {
        let mut store = RecordStore::in_memory();
        let result = store.write(
            RecordBatch::new()
                .put(key("account/alice", "account"), 1, json!({}))
                .put(key("Account/Bob", "account"), 1, json!({})),
        );
        let error = result.unwrap_err();
        assert!(matches!(error, RecordError::InvalidKey { .. }));
        assert!(error.to_string().contains("Account/Bob"));
        assert!(
            store
                .read(&key("account/alice", "account"))
                .unwrap()
                .is_none()
        );
    }
}
