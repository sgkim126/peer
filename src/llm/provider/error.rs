use std::fmt;

#[derive(Debug)]
#[cfg_attr(not(test), expect(dead_code))]
pub enum LlmCallError {
    ContextOverflow {
        message: String,
    },
    Transient {
        message: String,
        source: Box<dyn std::error::Error>,
    },
    Permanent {
        message: String,
        source: Box<dyn std::error::Error>,
    },
}

impl fmt::Display for LlmCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContextOverflow { message } => {
                write!(f, "LLM context length exceeded: {message}")
            }
            Self::Transient { message, .. } => {
                write!(f, "transient LLM call failure: {message}")
            }
            Self::Permanent { message, .. } => {
                write!(f, "permanent LLM call failure: {message}")
            }
        }
    }
}

impl std::error::Error for LlmCallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transient { source, .. } => Some(source.as_ref()),
            Self::Permanent { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implements_error() {
        fn assert_error<T: std::error::Error>() {}

        assert_error::<LlmCallError>();
    }

    #[test]
    fn displays_context_overflow() {
        let error = LlmCallError::ContextOverflow {
            message: "maximum is 128k tokens".to_string(),
        };

        assert_eq!(
            error.to_string(),
            "LLM context length exceeded: maximum is 128k tokens"
        );
    }

    #[test]
    fn displays_transient_failure() {
        let error = LlmCallError::Transient {
            message: "request timed out".to_string(),
            source: Box::new(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request timed out",
            )),
        };

        assert_eq!(
            error.to_string(),
            "transient LLM call failure: request timed out"
        );
    }

    #[test]
    fn displays_permanent_failure() {
        let error = LlmCallError::Permanent {
            message: "invalid API key".to_string(),
            source: Box::new(std::io::Error::other("missing secret")),
        };

        assert_eq!(
            error.to_string(),
            "permanent LLM call failure: invalid API key"
        );
    }

    #[test]
    fn permanent_failure_exposes_source() {
        let error = LlmCallError::Permanent {
            message: "failed to load API key".to_string(),
            source: Box::new(std::io::Error::other("missing secret")),
        };

        let source = std::error::Error::source(&error).unwrap();

        assert_eq!(source.to_string(), "missing secret");
    }
}
