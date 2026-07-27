use std::fmt;

/// A secret value whose current string buffer is zeroed when it is dropped.
///
/// This does not erase copies made elsewhere, such as cloned values, prior
/// allocations, logs, or serialized output.
#[derive(Clone)]
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

    use std::ffi::{OsStr, OsString};
    use std::sync::{Mutex, MutexGuard};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        name: &'static str,
        original: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: impl AsRef<OsStr>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let original = std::env::var_os(name);

            // SAFETY: The guard serializes all access to these test-specific
            // environment variables and holds the lock until it restores the
            // original value.
            unsafe {
                std::env::set_var(name, value);
            };

            Self {
                name,
                original,
                _lock: lock,
            }
        }

        fn remove(name: &'static str) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let original = std::env::var_os(name);

            // SAFETY: The guard serializes all access to these test-specific
            // environment variables and holds the lock until it restores the
            // original value.
            unsafe {
                std::env::remove_var(name);
            };

            Self {
                name,
                original,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: The guard still holds ENV_LOCK, so no other test in this
            // module can access the test-specific environment variable while
            // its original value is restored.
            unsafe {
                match self.original.take() {
                    Some(value) => std::env::set_var(self.name, value),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    #[test]
    fn debug_redacts_secret() {
        let secret = Secret::new("secret-value".to_owned());

        assert_eq!(format!("{secret:?}"), "<******>");
    }

    #[test]
    fn from_env_fails_when_missing() {
        let name = "PEER_TEST_MISSING_SECRET_7E3B8F91A2C4";
        let _env = EnvVarGuard::remove(name);

        assert_eq!(
            Secret::from_env(name).unwrap_err(),
            SecretError::MissingEnv {
                name: name.to_owned()
            }
        );
    }

    #[test]
    fn from_env_fails_when_empty() {
        let name = "PEER_TEST_EMPTY_SECRET_7E3B8F91A2C4";
        let _env = EnvVarGuard::set(name, "");

        assert_eq!(
            Secret::from_env(name).unwrap_err(),
            SecretError::MissingEnv {
                name: name.to_owned()
            }
        );
    }

    #[test]
    fn from_env_succeeds_when_set() {
        let name = "PEER_TEST_PRESENT_SECRET_7E3B8F91A2C4";
        let _env = EnvVarGuard::set(name, "my-api-key");

        let secret = Secret::from_env(name).unwrap();

        assert_eq!(secret.expose_secret(), "my-api-key");
    }

    #[cfg(unix)]
    #[test]
    fn from_env_fails_when_value_is_not_unicode() {
        let name = "PEER_TEST_NON_UNICODE_SECRET_7E3B8F91A2C4";
        let _env = EnvVarGuard::set(name, OsString::from_vec(vec![0xff]));

        assert_eq!(
            Secret::from_env(name).unwrap_err(),
            SecretError::NonUnicodeEnv {
                name: name.to_owned()
            }
        );
    }
}
