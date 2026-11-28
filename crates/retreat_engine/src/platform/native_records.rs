//! Server records as files (native only): `<directory>/<owner>/<part>.json`, one file per record.
//! Owners and parts are checked by the contract (lowercase letters, digits, `_`, `-`, owners
//! joined by `/`), so they are safe folder and file names on every system.
//!
//! **All or nothing, also across a crash.** A commit first writes every change to
//! `<directory>/journal.json` (a temporary file renamed into place, so the journal is whole or
//! absent), then applies the changes one by one (each record also through a temporary file and a
//! rename), then removes the journal. Opening the folder finishes a journal left by a crash, so a
//! batch is either fully applied or, if the journal never got written, not at all. Applying a
//! journal twice gives the same result, so a crash during recovery is harmless too.

use std::path::{Path, PathBuf};

use retreat_engine_api::serde_json::{self, Value, json};
use retreat_engine_api::{RecordBackend, RecordChange, RecordKey};

/// The journal of the commit in progress.
const JOURNAL_FILE: &str = "journal.json";

pub(crate) struct FileRecordBackend {
    directory: PathBuf,
}

impl FileRecordBackend {
    /// Opens (or starts) the records in `directory` and finishes a commit a crash interrupted.
    pub(crate) fn open(directory: PathBuf) -> Result<Self, String> {
        let backend = Self { directory };
        backend.finish_interrupted_commit()?;
        Ok(backend)
    }

    fn owner_folder(&self, owner: &str) -> PathBuf {
        owner
            .split('/')
            .fold(self.directory.clone(), |path, segment| path.join(segment))
    }

    fn file_for(&self, key: &RecordKey) -> PathBuf {
        self.owner_folder(&key.owner)
            .join(format!("{}.json", key.part))
    }

    fn journal(&self) -> PathBuf {
        self.directory.join(JOURNAL_FILE)
    }

    fn finish_interrupted_commit(&self) -> Result<(), String> {
        let journal = self.journal();
        let text = match std::fs::read_to_string(&journal) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("cannot read `{}`: {error}", journal.display())),
        };
        let changes = decode_journal(&text).map_err(|reason| {
            format!(
                "the unfinished commit in `{}` is damaged ({reason}); look at it, then delete it \
                 to drop that commit",
                journal.display()
            )
        })?;
        self.apply(&changes)?;
        remove_file(&journal)
    }

    fn apply(&self, changes: &[RecordChange]) -> Result<(), String> {
        for change in changes {
            match change {
                RecordChange::Put { key, text } => write_whole(&self.file_for(key), text)?,
                RecordChange::Delete { key } => remove_file(&self.file_for(key))?,
            }
        }
        Ok(())
    }
}

impl RecordBackend for FileRecordBackend {
    fn read(&self, key: &RecordKey) -> Result<Option<String>, String> {
        let file = self.file_for(key);
        match std::fs::read_to_string(&file) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot read `{}`: {error}", file.display())),
        }
    }

    fn parts(&self, owner: &str) -> Result<Vec<String>, String> {
        let folder = self.owner_folder(owner);
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("cannot list `{}`: {error}", folder.display())),
        };
        let mut parts: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.strip_suffix(".json").map(String::from)
            })
            .collect();
        parts.sort();
        Ok(parts)
    }

    fn commit(&mut self, changes: &[RecordChange]) -> Result<(), String> {
        write_whole(&self.journal(), &encode_journal(changes))?;
        self.apply(changes)?;
        remove_file(&self.journal())
    }

    fn location(&self) -> String {
        format!("the folder `{}`", self.directory.display())
    }
}

/// Writes `text` to `file` through a temporary file and a rename, so the file is whole or old.
fn write_whole(file: &Path, text: &str) -> Result<(), String> {
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("cannot create `{}`: {error}", folder.display()))?;
    }
    let temporary = file.with_extension("json.tmp");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, file))
        .map_err(|error| format!("cannot write `{}`: {error}", file.display()))
}

/// Removes `file`; a missing file is fine.
fn remove_file(file: &Path) -> Result<(), String> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot delete `{}`: {error}", file.display())),
    }
}

fn encode_journal(changes: &[RecordChange]) -> String {
    let steps: Vec<Value> = changes
        .iter()
        .map(|change| match change {
            RecordChange::Put { key, text } => {
                json!({"put": {"owner": key.owner, "part": key.part, "text": text}})
            }
            RecordChange::Delete { key } => {
                json!({"delete": {"owner": key.owner, "part": key.part}})
            }
        })
        .collect();
    Value::Array(steps).to_string()
}

fn decode_journal(text: &str) -> Result<Vec<RecordChange>, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("not valid JSON: {error}"))?;
    let steps = value.as_array().ok_or("not a list of changes")?;
    let text_of = |step: &Value, field: &str| -> Result<String, String> {
        step.get(field)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| format!("a change has no text `{field}`"))
    };
    steps
        .iter()
        .map(|step| {
            if let Some(put) = step.get("put") {
                Ok(RecordChange::Put {
                    key: RecordKey::new(text_of(put, "owner")?, text_of(put, "part")?),
                    text: text_of(put, "text")?,
                })
            } else if let Some(delete) = step.get("delete") {
                Ok(RecordChange::Delete {
                    key: RecordKey::new(text_of(delete, "owner")?, text_of(delete, "part")?),
                })
            } else {
                Err("a change is neither `put` nor `delete`".to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use retreat_engine_api::{RecordBatch, RecordStore};

    /// A fresh empty folder under the system temp folder, removed at the end of the test.
    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("retreat_records_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store(directory: &TempDirectory) -> RecordStore {
        RecordStore::new(Box::new(
            FileRecordBackend::open(directory.0.clone()).unwrap(),
        ))
    }

    #[test]
    fn records_survive_a_restart_and_a_deleted_owner_loses_every_part() {
        let directory = TempDirectory::new("restart");
        let alice = RecordKey::new("account/alice", "account");
        let place = RecordKey::new("character/alice-1", "player_spawn");
        store(&directory)
            .write(
                RecordBatch::new()
                    .put(alice.clone(), 1, json!({"characters": 1}))
                    .put(place.clone(), 1, json!({"x": 3.5}))
                    .put(RecordKey::new("character/alice-1", "account"), 1, json!({})),
            )
            .unwrap();
        assert!(
            directory
                .0
                .join("character/alice-1/player_spawn.json")
                .exists()
        );
        assert!(
            !directory.0.join(JOURNAL_FILE).exists(),
            "the journal is gone"
        );
        // A new store on the same folder is the server started again.
        let mut again = store(&directory);
        assert_eq!(again.read(&place).unwrap().unwrap().data, json!({"x": 3.5}));
        again
            .write(RecordBatch::new().delete_owner("character/alice-1"))
            .unwrap();
        assert!(again.parts("character/alice-1").unwrap().is_empty());
        assert!(again.read(&alice).unwrap().is_some());
    }

    #[test]
    fn a_commit_interrupted_by_a_crash_is_finished_on_the_next_start() {
        let directory = TempDirectory::new("crash");
        let first = RecordKey::new("account/bob", "account");
        let second = RecordKey::new("world/dev", "account_names");
        // What a crash right after writing the journal leaves behind: the journal, no records.
        let changes = [
            RecordChange::Put {
                key: first.clone(),
                text: retreat_engine_api::encode_record(&first, 1, &json!({"n": 1})),
            },
            RecordChange::Put {
                key: second.clone(),
                text: retreat_engine_api::encode_record(&second, 1, &json!({"bob": "bob-1"})),
            },
        ];
        std::fs::create_dir_all(&directory.0).unwrap();
        std::fs::write(directory.0.join(JOURNAL_FILE), encode_journal(&changes)).unwrap();
        let recovered = store(&directory);
        assert!(recovered.read(&first).unwrap().is_some());
        assert!(recovered.read(&second).unwrap().is_some());
        assert!(!directory.0.join(JOURNAL_FILE).exists());
    }

    #[test]
    fn a_damaged_journal_stops_the_start_with_a_message_that_says_what_to_do() {
        let directory = TempDirectory::new("damaged");
        std::fs::create_dir_all(&directory.0).unwrap();
        std::fs::write(directory.0.join(JOURNAL_FILE), "[{\"put\":{}}]").unwrap();
        let error = FileRecordBackend::open(directory.0.clone())
            .err()
            .expect("a damaged journal is an error");
        assert!(
            error.contains("is damaged") && error.contains("delete it"),
            "{error}"
        );
    }
}
