use std::fmt;

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
