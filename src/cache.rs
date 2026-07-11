use std::fmt;
use std::path::PathBuf;

use semver::Version;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::console::Console;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

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
            .join(safe_segment(&cache_version(BINARY_VERSION)))
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
                    .debug(format_args!("cache miss: {}", path.display()));
                return Ok(None);
            }
            Err(source) => {
                self.console
                    .debug(format_args!("cannot read {}: {source:?}", path.display()));
                let error = CacheReadError::Read { path, source };
                return Err(error);
            }
        };

        match serde_json::from_str(&content) {
            Ok(value) => {
                self.console
                    .debug(format_args!("cache hit: {}", path.display()));
                Ok(Some(value))
            }
            Err(source) => {
                self.console
                    .debug(format_args!("cannot read {}: {source:?}", path.display()));
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
                .debug(format_args!("cannot create {}: {source:?}", path.display()));
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
                    .debug(format_args!("cannot convert cached value: {source:?}"));
                let error = CacheWriteError::Serialize { source };
                return Err(error);
            }
        };
        if let Err(source) = std::fs::write(&path, content) {
            self.console
                .debug(format_args!("cannot write {}: {source:?}", path.display()));
            let error = CacheWriteError::Write { path, source };
            return Err(error);
        }
        self.console
            .debug(format_args!("cache write: {}", path.display()));
        Ok(())
    }

    /// Removes cache directories belonging to versions older than this binary.
    pub fn prune_older_versions(&self) -> Result<usize, CachePruneError> {
        let current_version = Version::parse(&format!("{}.0", cache_version(BINARY_VERSION)))
            .expect("CARGO_PKG_VERSION must contain a major and minor version");
        prune_entries(&self.root, |entry, file_type| {
            if !file_type.is_dir() {
                return false;
            }

            entry
                .file_name()
                .to_str()
                .and_then(|name| {
                    Version::parse(name)
                        .or_else(|_| Version::parse(&format!("{name}.0")))
                        .ok()
                })
                .is_some_and(|version| {
                    Version::new(version.major, version.minor, 0) < current_version
                })
        })
    }

    /// Removes every cache entry, including entries for the current version.
    pub fn prune_all(&self) -> Result<usize, CachePruneError> {
        prune_entries(&self.root, |_, _| true)
    }
}

fn prune_entries(
    root: &std::path::Path,
    mut should_remove: impl FnMut(&std::fs::DirEntry, &std::fs::FileType) -> bool,
) -> Result<usize, CachePruneError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(source) => {
            return Err(CachePruneError::ReadDir {
                path: root.to_path_buf(),
                source,
            });
        }
    };

    let mut removed = 0;
    for entry in entries {
        let entry = entry.map_err(|source| CachePruneError::ReadDir {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| CachePruneError::ReadDir {
                path: path.clone(),
                source,
            })?;
        if !should_remove(&entry, &file_type) {
            continue;
        }

        let result = if file_type.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        result.map_err(|source| CachePruneError::Remove { path, source })?;
        removed += 1;
    }

    Ok(removed)
}

fn cache_version(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
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

#[derive(Debug)]
pub enum CachePruneError {
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Remove {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for CachePruneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadDir { path, source } => {
                write!(
                    f,
                    "failed to read cache directory {}: {source}",
                    path.display()
                )
            }
            Self::Remove { path, source } => {
                write!(
                    f,
                    "failed to remove cache directory {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for CachePruneError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDir { source, .. } | Self::Remove { source, .. } => Some(source),
        }
    }
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
                .join(cache_version(BINARY_VERSION))
                .join("tool")
                .join("provider")
                .join("model")
                .join(prefix)
                .join(format!("{}.json", key.params_hash))
        );
    }

    #[test]
    fn cache_version_ignores_patch_number() {
        assert_eq!(cache_version("1.2.3"), "1.2");
        assert_eq!(cache_version("1.2.4"), "1.2");
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
                .join(cache_version(BINARY_VERSION))
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
                .join(cache_version(BINARY_VERSION))
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

    #[test]
    fn prunes_only_older_cache_version_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache");
        std::fs::create_dir_all(root.join("0.0")).unwrap();
        std::fs::create_dir_all(root.join(cache_version(BINARY_VERSION))).unwrap();
        std::fs::create_dir_all(root.join("999.0")).unwrap();
        std::fs::create_dir_all(root.join("not-a-version")).unwrap();
        std::fs::write(root.join("cache-file"), "cache").unwrap();
        let store = CacheStore::new(&root, Console::default());

        let removed = store.prune_older_versions().unwrap();

        assert_eq!(removed, 1);
        assert!(!root.join("0.0").exists());
        assert!(root.join(cache_version(BINARY_VERSION)).exists());
        assert!(root.join("999.0").exists());
        assert!(root.join("not-a-version").exists());
        assert!(root.join("cache-file").exists());
    }

    #[test]
    fn prunes_all_cache_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cache");
        std::fs::create_dir_all(root.join("0.1")).unwrap();
        std::fs::create_dir_all(root.join("not-a-version")).unwrap();
        std::fs::write(root.join("cache-file"), "cache").unwrap();
        let store = CacheStore::new(&root, Console::default());

        let removed = store.prune_all().unwrap();

        assert_eq!(removed, 3);
        assert!(root.is_dir());
        assert!(std::fs::read_dir(root).unwrap().next().is_none());
    }

    #[test]
    fn pruning_missing_cache_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache"), Console::default());

        assert_eq!(store.prune_older_versions().unwrap(), 0);
    }
}
