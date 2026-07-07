use std::fmt;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::console::Console;

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

    pub fn path_for(&self, key: &CacheKey) -> PathBuf {
        self.root
            .join(safe_segment(&key.tool))
            .join(safe_segment(&key.provider))
            .join(safe_segment(&key.model))
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.console
                    .debug(format!("cache miss: {}", path.display()));
                return Ok(None);
            }
            Err(source) => {
                self.console
                    .debug(format!("cannot read {}: {source:?}", path.display()));
                let error = CacheReadError::Read { path, source };
                return Err(error);
            }
        };

        match serde_json::from_str(&content) {
            Ok(value) => {
                self.console.debug(format!("cache hit: {}", path.display()));
                Ok(Some(value))
            }
            Err(source) => {
                self.console
                    .debug(format!("cannot read {}: {source:?}", path.display()));
                let error = CacheReadError::Deserialize { path, source };
                Err(error)
            }
        }
    }

    pub fn write_json<T>(&self, key: &CacheKey, value: &T) -> Result<(), CacheWriteError>
    where
        T: Serialize,
    {
        let path = self.path_for(key);
        if let Some(parent) = path.parent()
            && let Err(source) = std::fs::create_dir_all(parent)
        {
            self.console
                .debug(format!("cannot create {}: {source:?}", path.display()));
            let error = CacheWriteError::CreateDir {
                path: parent.to_path_buf(),
                source,
            };
            return Err(error);
        }
        let content = match serde_json::to_string_pretty(value) {
            Ok(content) => content,
            Err(source) => {
                self.console
                    .debug(format!("cannot convert cached value: {source:?}"));
                let error = CacheWriteError::Serialize { source };
                return Err(error);
            }
        };
        if let Err(source) = std::fs::write(&path, content) {
            self.console
                .debug(format!("cannot write {}: {source:?}", path.display()));
            let error = CacheWriteError::Write { path, source };
            return Err(error);
        }
        self.console
            .debug(format!("cache write: {}", path.display()));
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    tool: String,
    provider: String,
    model: String,
    params_hash: String,
}

impl CacheKey {
    pub fn from_params<T>(
        tool: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        params: &T,
    ) -> Result<Self, serde_json::Error>
    where
        T: Serialize,
    {
        Ok(Self {
            tool: tool.into(),
            provider: provider.into(),
            model: model.into(),
            params_hash: hash_serializable(params)?,
        })
    }
}

#[derive(Debug)]
pub enum CacheReadError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Deserialize {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for CacheReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read cache value {}: {source}", path.display())
            }
            Self::Deserialize { path, source } => {
                write!(
                    f,
                    "failed to parse cache value {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CacheReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Deserialize { source, .. } => Some(source),
        }
    }
}

#[derive(Debug)]
pub enum CacheWriteError {
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Serialize {
        source: serde_json::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for CacheWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateDir { path, source } => {
                write!(
                    f,
                    "failed to create cache directory {}: {source}",
                    path.display()
                )
            }
            Self::Serialize { source } => write!(f, "failed to serialize cache value: {source}"),
            Self::Write { path, source } => {
                write!(
                    f,
                    "failed to write cache value {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CacheWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateDir { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Serialize { source } => Some(source),
        }
    }
}

fn hash_serializable<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn safe_segment(segment: &str) -> String {
    let safe = segment
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if matches!(safe.as_str(), "." | "..") {
        "_".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::console::Console;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Value {
        value: String,
    }

    #[test]
    fn builds_key_from_serializable_params() {
        let key = CacheKey::from_params(
            "tool",
            "provider",
            "model",
            &Value {
                value: "test".to_string(),
            },
        )
        .unwrap();

        assert_eq!(key.params_hash.len(), 64);
        assert!(key.params_hash.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn builds_cache_path_with_hash_prefix() {
        let store = CacheStore::new(Path::new(".peer/cache"), Console::default());
        let key = CacheKey::from_params(
            "tool",
            "provider",
            "model",
            &Value {
                value: "test".to_string(),
            },
        )
        .unwrap();
        let prefix = &key.params_hash[..2];

        assert_eq!(
            store.path_for(&key),
            Path::new(".peer/cache")
                .join("tool")
                .join("provider")
                .join("model")
                .join(prefix)
                .join(format!("{}.json", key.params_hash))
        );
    }

    #[test]
    fn sanitizes_path_segments() {
        let store = CacheStore::new(Path::new(".peer/cache"), Console::default());
        let key = CacheKey::from_params(
            "tool/name",
            "provider:name",
            "model name",
            &Value {
                value: "test".to_string(),
            },
        )
        .unwrap();
        let prefix = &key.params_hash[..2];

        assert_eq!(
            store.path_for(&key),
            Path::new(".peer/cache")
                .join("tool_name")
                .join("provider_name")
                .join("model_name")
                .join(prefix)
                .join(format!("{}.json", key.params_hash))
        );
    }

    #[test]
    fn sanitizes_current_and_parent_directory_segments() {
        let store = CacheStore::new(Path::new(".peer/cache"), Console::default());
        let key = CacheKey::from_params(
            ".",
            "..",
            ".",
            &Value {
                value: "test".to_string(),
            },
        )
        .unwrap();
        let prefix = &key.params_hash[..2];

        assert_eq!(
            store.path_for(&key),
            Path::new(".peer/cache")
                .join("_")
                .join("_")
                .join("_")
                .join(prefix)
                .join(format!("{}.json", key.params_hash))
        );
    }

    #[test]
    fn reads_and_writes_json_values() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache"), Console::default());
        let key = CacheKey::from_params(
            "tool",
            "provider",
            "model",
            &Value {
                value: "key".to_string(),
            },
        )
        .unwrap();
        let value = Value {
            value: "cached".to_string(),
        };

        store.write_json(&key, &value).unwrap();

        assert_eq!(store.read_json::<Value>(&key).unwrap(), Some(value));
    }

    #[test]
    fn treats_missing_json_as_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache"), Console::default());
        let key = CacheKey::from_params(
            "tool",
            "provider",
            "model",
            &Value {
                value: "key".to_string(),
            },
        )
        .unwrap();

        assert_eq!(store.read_json::<Value>(&key).unwrap(), None);
    }

    #[test]
    fn returns_error_for_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache"), Console::default());
        let key = CacheKey::from_params(
            "tool",
            "provider",
            "model",
            &Value {
                value: "key".to_string(),
            },
        )
        .unwrap();
        let path = store.path_for(&key);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "not json").unwrap();

        assert!(matches!(
            store.read_json::<Value>(&key),
            Err(CacheReadError::Deserialize { .. })
        ));
    }
}
