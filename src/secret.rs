use std::fmt;

#[derive(Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Secret(String);

#[derive(Debug)]
pub enum SecretError {
    MissingEnv { name: String },
    NonUnicodeEnv { name: String },
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[allow(dead_code)]
    pub fn from_env(name: &str) -> Result<Self, SecretError> {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(Self::new(value)),
            Ok(_) => Err(SecretError::MissingEnv {
                name: name.to_string(),
            }),
            Err(std::env::VarError::NotPresent) => Err(SecretError::MissingEnv {
                name: name.to_string(),
            }),
            Err(std::env::VarError::NotUnicode(_)) => Err(SecretError::NonUnicodeEnv {
                name: name.to_string(),
            }),
        }
    }

    #[allow(dead_code)]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<******>")
    }
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnv { name } => {
                write!(f, "environment variable {name} is not set or empty")
            }
            Self::NonUnicodeEnv { name } => {
                write!(f, "environment variable {name} is not valid unicode")
            }
        }
    }
}

impl std::error::Error for SecretError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secret() {
        let secret = Secret::new("secret-value");

        assert_eq!(format!("{secret:?}"), "<******>");
    }

    #[test]
    fn from_env_fails_when_missing() {
        let name = "PEER_TEST_MISSING_SECRET_7E3B8F91A2C4";

        let error = Secret::from_env(name).unwrap_err();

        assert!(matches!(error, SecretError::MissingEnv { .. }));
        assert_eq!(
            error.to_string(),
            format!("environment variable {name} is not set or empty")
        );
    }
}
