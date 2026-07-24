use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::console::Console;

use super::error::{CacheReadError, CacheWriteError};
use super::key::CacheKey;

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn sanitize_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct CacheStore {
    root: PathBuf,
    console: Console,
}

impl CacheStore {
    pub fn new(root: impl Into<PathBuf>, console: Console) -> Self {
        Self {
            root: root.into(),
            console,
        }
    }

    fn path_for(&self, key: &CacheKey) -> PathBuf {
        self.root
            .join(CacheKey::version())
            .join(sanitize_path_segment(&key.provider))
            .join(sanitize_path_segment(&key.model))
            .join(sanitize_path_segment(&key.namespace))
            .join(&key.params_hash[..2])
            .join(format!("{}.json", key.params_hash))
    }

    pub fn read_json<T>(&self, key: &CacheKey) -> Result<Option<T>, CacheReadError>
    where
        T: DeserializeOwned,
    {
        let path = self.path_for(key);
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                self.console
                    .debug(format_args!("cache miss: {}", path.display()));
                return Ok(None);
            }
            Err(source) => {
                self.console
                    .debug(format_args!("cannot read {}: {source:?}", path.display()));
                return Err(CacheReadError::Read { path, source });
            }
        };
        let value = serde_json::from_str(&content).map_err(|source| {
            self.console
                .debug(format_args!("cannot parse {}: {source:?}", path.display()));
            CacheReadError::Deserialize {
                path: path.clone(),
                source,
            }
        })?;
        self.console
            .debug(format_args!("cache hit: {}", path.display()));
        Ok(Some(value))
    }

    pub fn write_json<T>(&self, key: &CacheKey, value: &T) -> Result<(), CacheWriteError>
    where
        T: Serialize,
    {
        let content = serde_json::to_vec_pretty(value)?;
        let path = self.path_for(key);
        let parent = path
            .parent()
            .expect("cache value path always has a parent directory");
        std::fs::create_dir_all(parent).map_err(|source| CacheWriteError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;

        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary =
            path.with_extension(format!("json.{}.{}.tmp", std::process::id(), sequence));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .and_then(|mut file| {
                file.write_all(&content)?;
                file.sync_all()?;
                Ok(file)
            });
        if let Err(source) = file {
            let _ = std::fs::remove_file(&temporary);
            return Err(CacheWriteError::Write {
                path: temporary,
                source,
            });
        }
        if let Err(source) = std::fs::rename(&temporary, &path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(CacheWriteError::Rename {
                from: temporary,
                to: path,
                source,
            });
        }

        self.console
            .debug(format_args!("cache write: {}", path.display()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::assert_matches;
    use std::path::Path;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Value {
        value: String,
    }

    fn key(value: &str) -> CacheKey {
        CacheKey::from_params("review/context", "provider:name", "model name", &value).unwrap()
    }

    #[test]
    fn path_orders_provider_and_model_before_namespace() {
        let store = CacheStore::new(".peer/cache", Console::default());
        let key = key("key");
        let path = store.path_for(&key);

        assert!(
            path.starts_with(
                Path::new(".peer/cache")
                    .join(CacheKey::version())
                    .join("provider_name")
                    .join("model_name")
                    .join("review_context")
            )
        );
    }

    #[test]
    fn sanitizes_path_segments() {
        assert_eq!(
            sanitize_path_segment("openai/gpt:4 mini"),
            "openai_gpt_4_mini"
        );
    }

    #[test]
    fn reads_and_atomically_writes_json() {
        let directory = tempfile::tempdir().unwrap();
        let store = CacheStore::new(directory.path(), Console::default());
        let key = key("key");
        let value = Value {
            value: "cached".to_string(),
        };

        store.write_json(&key, &value).unwrap();

        assert_eq!(store.read_json::<Value>(&key).unwrap(), Some(value));
        assert_eq!(
            std::fs::read_dir(store.path_for(&key).parent().unwrap())
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn missing_files_are_cache_misses() {
        let directory = tempfile::tempdir().unwrap();
        let store = CacheStore::new(directory.path(), Console::default());

        assert_eq!(store.read_json::<Value>(&key("missing")).unwrap(), None);
    }

    #[test]
    fn malformed_json_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let store = CacheStore::new(directory.path(), Console::default());
        let key = key("invalid");
        let path = store.path_for(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "not json").unwrap();

        assert_matches!(
            store.read_json::<Value>(&key),
            Err(CacheReadError::Deserialize { .. })
        );
    }
}
