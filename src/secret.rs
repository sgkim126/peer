use std::fmt;

/// A secret value whose current string buffer is zeroed when it is dropped.
///
/// This does not erase copies made elsewhere, such as cloned values, prior
/// allocations, logs, or serialized output.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

#[derive(Debug, PartialEq, Eq)]
pub enum SecretError {
    MissingEnv { name: String },
    NonUnicodeEnv { name: String },
}

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    #[cfg_attr(not(test), expect(dead_code))]
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

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // `String` is valid UTF-8 before this operation, and a sequence of NUL
        // bytes is valid UTF-8 afterwards.
        let bytes = unsafe { self.0.as_mut_vec() };
        for byte in bytes {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }

        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
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
        let secret = Secret::new("secret-value".to_owned());

        assert_eq!(format!("{secret:?}"), "<******>");
    }

    #[test]
    fn from_env_fails_when_missing() {
        let name = "PEER_TEST_MISSING_SECRET_7E3B8F91A2C4";

        assert_eq!(
            Secret::from_env(name),
            Err(SecretError::MissingEnv {
                name: name.to_owned()
            })
        );
    }
}
