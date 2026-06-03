use std::fmt;

#[derive(Debug)]
pub enum PeerError {
    InvalidConfig {
        message: String,
        source: Option<Box<dyn std::error::Error>>,
    },
}

impl fmt::Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig { message, source } => {
                if let Some(source) = source {
                    write!(f, "{message} ({source})")
                } else {
                    write!(f, "{message}")
                }
            }
        }
    }
}

impl std::error::Error for PeerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig { source, .. } => source.as_deref(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::error::Error;

    #[test]
    fn config_error_source_chain_is_preserved() {
        let e = PeerError::InvalidConfig {
            message: "invalid config".into(),
            source: Some(Box::new(std::io::Error::other("underlying cause"))),
        };
        assert!(e.source().is_some());
    }

    #[test]
    fn config_error_without_source_has_no_chain() {
        let e = PeerError::InvalidConfig {
            message: "not found".into(),
            source: None,
        };

        assert!(e.source().is_none());
    }
}
