use std::fmt;

/// A secret value whose current string buffer is zeroed when it is dropped.
///
/// This does not erase copies made elsewhere, such as cloned values, prior
/// allocations, logs, or serialized output.
pub struct Secret(String);

#[derive(Debug, PartialEq, Eq)]
pub enum SecretError {
    MissingEnv { name: String },
    NonUnicodeEnv { name: String },
}

impl Secret {
    pub fn from_env(name: &str) -> Result<Self, SecretError> {
        Self::from_env_with(name, |name| std::env::var(name))
    }

    pub fn from_env_with(
        name: &str,
        get: impl FnOnce(&str) -> Result<String, std::env::VarError>,
    ) -> Result<Self, SecretError> {
        match get(name) {
            Ok(value) if !value.is_empty() => Ok(Self(value)),
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

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // SAFETY: The mutable borrow is exclusive, and every byte is replaced
        // with NUL before the borrow ends, leaving the `String` as valid UTF-8.
        let bytes = unsafe { self.0.as_mut_vec() };
        for byte in bytes {
            // SAFETY: `byte` comes from an exclusive mutable reference to the
            // initialized `String` buffer, so it is valid, aligned, and writable.
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

    #[cfg(unix)]
    use std::ffi::OsString;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn debug_redacts_secret() {
        let secret =
            Secret::from_env_with("TEST_SECRET", |_| Ok("secret-value".to_owned())).unwrap();

        assert_eq!(format!("{secret:?}"), "<******>");
    }

    #[test]
    fn from_env_fails_when_missing() {
        let name = "PEER_TEST_MISSING_SECRET_7E3B8F91A2C4";

        assert_eq!(
            Secret::from_env_with(name, |_| Err(std::env::VarError::NotPresent)).unwrap_err(),
            SecretError::MissingEnv {
                name: name.to_owned()
            }
        );
    }

    #[test]
    fn from_env_fails_when_empty() {
        let name = "PEER_TEST_EMPTY_SECRET_7E3B8F91A2C4";

        assert_eq!(
            Secret::from_env_with(name, |_| Ok(String::new())).unwrap_err(),
            SecretError::MissingEnv {
                name: name.to_owned()
            }
        );
    }

    #[test]
    fn from_env_succeeds_when_set() {
        let name = "PEER_TEST_PRESENT_SECRET_7E3B8F91A2C4";

        let secret = Secret::from_env_with(name, |_| Ok("my-api-key".to_owned())).unwrap();

        assert_eq!(secret.expose_secret(), "my-api-key");
    }

    #[cfg(unix)]
    #[test]
    fn from_env_fails_when_value_is_not_unicode() {
        let name = "PEER_TEST_NON_UNICODE_SECRET_7E3B8F91A2C4";

        assert_eq!(
            Secret::from_env_with(name, |_| {
                Err(std::env::VarError::NotUnicode(OsString::from_vec(vec![
                    0xff,
                ])))
            })
            .unwrap_err(),
            SecretError::NonUnicodeEnv {
                name: name.to_owned()
            }
        );
    }
}
