use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::console::Console;

use super::{
    CacheKey, CachePruneError, CacheReadError, CacheRemoveError, CacheVersion, CacheWriteError,
};

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

    pub fn version_root(&self) -> PathBuf {
        self.root.join(CacheKey::version())
    }

    fn path_for(&self, key: &CacheKey) -> PathBuf {
        self.version_root()
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

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn remove(&self, key: &CacheKey) -> Result<(), CacheRemoveError> {
        let path = self.path_for(key);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.console
                    .debug(format_args!("cache remove: {}", path.display()));
                Ok(())
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => {
                self.console
                    .debug(format_args!("cannot remove {}: {source:?}", path.display()));
                Err(CacheRemoveError { path, source })
            }
        }
    }

    pub fn prune(&self, all: bool) -> Result<usize, CachePruneError> {
        let started = Instant::now();
        self.console
            .verbose(format_args!("prune started root={:?} all={all}", self.root));
        let current = (!all)
            .then(|| {
                let version = CacheKey::version();
                CacheVersion::parse(&version).ok_or_else(|| {
                    self.console.debug(format_args!(
                        "cannot prune: invalid cache version {version}"
                    ));
                    CachePruneError::InvalidVersion { version }
                })
            })
            .transpose()?;
        let Some(entries) = self.entries()? else {
            self.console.verbose(format_args!(
                "prune completed removed=0 skipped=0 failed=0 duration_ms={}",
                started.elapsed().as_millis()
            ));
            return Ok(0);
        };
        let mut removed = 0;
        let mut skipped = 0;
        for entry in entries {
            let entry = entry.map_err(|source| {
                self.console.debug(format_args!(
                    "cannot read entry in {:?}: {source:?}",
                    self.root
                ));
                CachePruneError::ReadDir {
                    path: self.root.clone(),
                    source,
                }
            })?;
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    skipped += 1;
                    self.console.debug(format_args!(
                        "cache entry skipped path={path:?} reason=already-missing"
                    ));
                    continue;
                }
                Err(source) => {
                    self.console.debug(format_args!(
                        "cannot inspect cache entry {path:?}: {source:?}"
                    ));
                    return Err(CachePruneError::InspectEntry { path, source });
                }
            };
            let should_prune = all
                || (file_type.is_dir()
                    && entry
                        .file_name()
                        .to_str()
                        .and_then(CacheVersion::parse)
                        .zip(current)
                        .is_some_and(|(version, current)| version < current));
            if !should_prune {
                skipped += 1;
                self.console.debug(format_args!(
                    "cache entry skipped path={path:?} reason=not-prunable"
                ));
                continue;
            }

            self.console.debug(format_args!(
                "cache entry selected for pruning path={path:?} reason={}",
                if all { "all" } else { "outdated-version" }
            ));
            let result = if file_type.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            match result {
                Ok(()) => {
                    removed += 1;
                    self.console
                        .debug(format_args!("cache entry removed path={path:?}"));
                }
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                    skipped += 1;
                    self.console.debug(format_args!(
                        "cache entry skipped path={path:?} reason=already-missing"
                    ));
                }
                Err(source) => {
                    self.console.debug(format_args!(
                        "cache entry removal failed path={path:?} error={source:?}"
                    ));
                    return Err(CachePruneError::Remove { path, source });
                }
            }
        }
        self.console.verbose(format_args!(
            "prune completed removed={removed} skipped={skipped} failed=0 duration_ms={}",
            started.elapsed().as_millis()
        ));
        Ok(removed)
    }

    fn entries(&self) -> Result<Option<std::fs::ReadDir>, CachePruneError> {
        self.console
            .debug(format_args!("inspecting cache root path={:?}", self.root));
        let metadata = match std::fs::symlink_metadata(&self.root) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                self.console.debug(format_args!(
                    "cache root skipped path={:?} reason=missing",
                    self.root
                ));
                return Ok(None);
            }
            Err(source) => {
                self.console.debug(format_args!(
                    "cannot inspect cache root {:?}: {source:?}",
                    self.root
                ));
                return Err(CachePruneError::Inspect {
                    path: self.root.clone(),
                    source,
                });
            }
        };
        if !metadata.file_type().is_dir() {
            self.console.debug(format_args!(
                "cache root rejected path={:?} reason=not-directory-or-symlink",
                self.root
            ));
            return Err(CachePruneError::UnsafeRoot {
                path: self.root.clone(),
            });
        }
        self.console
            .debug(format_args!("reading cache entries root={:?}", self.root));
        std::fs::read_dir(&self.root).map(Some).map_err(|source| {
            self.console.debug(format_args!(
                "cannot read cache directory {:?}: {source:?}",
                self.root
            ));
            CachePruneError::ReadDir {
                path: self.root.clone(),
                source,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;
    use std::path::Path;

    use serde::Deserialize;

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
    fn removes_a_single_cache_value() {
        let directory = tempfile::tempdir().unwrap();
        let store = CacheStore::new(directory.path(), Console::default());
        let key = key("key");
        store
            .write_json(
                &key,
                &Value {
                    value: "cached".to_string(),
                },
            )
            .unwrap();

        store.remove(&key).unwrap();
        assert_eq!(store.read_json::<Value>(&key).unwrap(), None);
        store.remove(&key).unwrap();
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

    #[test]
    fn prune_removes_cache_entries_but_keeps_the_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("cache");
        std::fs::create_dir_all(root.join("0.1/provider")).unwrap();
        std::fs::write(root.join("0.1/provider/value.json"), "{}").unwrap();
        std::fs::write(root.join("loose.tmp"), "temporary").unwrap();
        let store = CacheStore::new(&root, Console::default());

        assert_eq!(store.prune(true).unwrap(), 2);
        assert!(root.is_dir());
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }

    #[test]
    fn prune_succeeds_when_the_cache_root_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let store = CacheStore::new(directory.path().join("missing"), Console::default());

        assert_eq!(store.prune(true).unwrap(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn prune_rejects_a_symbolic_link_cache_root() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let root = directory.path().join("cache");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, &root).unwrap();
        let store = CacheStore::new(&root, Console::default());

        assert_matches!(store.prune(true), Err(CachePruneError::UnsafeRoot { .. }));
        assert!(target.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn prune_removes_a_symbolic_link_without_following_it() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("cache");
        let target = directory.path().join("target");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("preserved"), "value").unwrap();
        symlink(&target, root.join("linked")).unwrap();
        let store = CacheStore::new(&root, Console::default());

        assert_eq!(store.prune(true).unwrap(), 1);
        assert!(target.join("preserved").is_file());
    }

    #[test]
    fn prune_without_all_removes_only_outdated_version_directories() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("cache");
        let current = CacheKey::version();
        for name in ["0.0", current.as_str(), "999999.0", "invalid"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::write(root.join("loose.tmp"), "temporary").unwrap();
        let store = CacheStore::new(&root, Console::default());

        assert_eq!(store.prune(false).unwrap(), 1);
        assert!(!root.join("0.0").exists());
        assert!(root.join(current).is_dir());
        assert!(root.join("999999.0").is_dir());
        assert!(root.join("invalid").is_dir());
        assert!(root.join("loose.tmp").is_file());
    }
}
