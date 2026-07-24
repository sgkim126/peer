use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct CacheKeyError {
    pub source: serde_json::Error,
}

impl From<serde_json::Error> for CacheKeyError {
    fn from(source: serde_json::Error) -> Self {
        Self { source }
    }
}

impl fmt::Display for CacheKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to serialize cache key parameters: {}",
            self.source
        )
    }
}

impl std::error::Error for CacheKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
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
                write!(f, "failed to read cache file {}: {source}", path.display())
            }
            Self::Deserialize { path, source } => {
                write!(f, "failed to parse cache file {}: {source}", path.display())
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
    Serialize {
        source: serde_json::Error,
    },
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
}

impl From<serde_json::Error> for CacheWriteError {
    fn from(source: serde_json::Error) -> Self {
        Self::Serialize { source }
    }
}

impl fmt::Display for CacheWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize { source } => write!(f, "failed to serialize cache value: {source}"),
            Self::CreateDir { path, source } => write!(
                f,
                "failed to create cache directory {}: {source}",
                path.display()
            ),
            Self::Write { path, source } => {
                write!(f, "failed to write cache file {}: {source}", path.display())
            }
            Self::Rename { from, to, source } => write!(
                f,
                "failed to move cache file {} to {}: {source}",
                from.display(),
                to.display()
            ),
        }
    }
}

impl std::error::Error for CacheWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize { source } => Some(source),
            Self::CreateDir { source, .. } => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::Rename { source, .. } => Some(source),
        }
    }
}
