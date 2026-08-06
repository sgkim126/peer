use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tokio::process::Command;

pub const SUPPORTED_PI_VERSION: &str = "0.83.0";

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct PiDependency {
    pub executable: PathBuf,
    pub version: String,
}

#[derive(Debug)]
pub enum DependencyError {
    Start(std::io::Error),
    Failed {
        status: Option<i32>,
        stderr: String,
    },
    UnsupportedVersion {
        expected: &'static str,
        actual: String,
    },
}

impl fmt::Display for DependencyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(error) => write!(
                f,
                "cannot start Pi; install pi {SUPPORTED_PI_VERSION} and ensure it is on PATH: {error}"
            ),
            Self::Failed { status, stderr } => write!(
                f,
                "pi --version failed with status {}: {stderr}",
                status.map_or_else(|| "signal".to_string(), |status| status.to_string())
            ),
            Self::UnsupportedVersion { expected, actual } => {
                write!(
                    f,
                    "unsupported Pi version {actual}; peer requires {expected}"
                )
            }
        }
    }
}

impl std::error::Error for DependencyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Start(error) => Some(error),
            Self::Failed { .. } => None,
            Self::UnsupportedVersion { .. } => None,
        }
    }
}

impl From<std::io::Error> for DependencyError {
    fn from(error: std::io::Error) -> Self {
        Self::Start(error)
    }
}

impl PiDependency {
    #[expect(dead_code)]
    pub async fn discover() -> Result<Self, DependencyError> {
        let search_path = std::env::var_os("PATH").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "PATH is not configured")
        })?;
        let cwd = std::env::current_dir()?;
        Self::from_search_path(&search_path, &cwd).await
    }

    async fn from_search_path(
        search_path: &std::ffi::OsStr,
        cwd: &Path,
    ) -> Result<Self, DependencyError> {
        let executable = find_executable(search_path, cwd)?;
        Self::from_executable(executable).await
    }

    async fn from_executable(executable: PathBuf) -> Result<Self, DependencyError> {
        let output = Command::new(&executable).arg("--version").output().await?;
        if !output.status.success() {
            return Err(DependencyError::Failed {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let actual = stdout
            .lines()
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .trim_start_matches('v')
            .to_string();
        if actual != SUPPORTED_PI_VERSION {
            return Err(DependencyError::UnsupportedVersion {
                expected: SUPPORTED_PI_VERSION,
                actual,
            });
        }
        Ok(Self {
            executable,
            version: actual,
        })
    }
}

fn find_executable(search_path: &std::ffi::OsStr, cwd: &Path) -> Result<PathBuf, std::io::Error> {
    for directory in std::env::split_paths(search_path) {
        let candidate = directory.join("pi");
        let candidate = if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        };
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return fs::canonicalize(candidate);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "cannot find executable `pi` on PATH",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    fn fake_pi(version: &str) -> (tempfile::TempDir, PathBuf) {
        fake_pi_with_output(&format!("pi {version}\\n"))
    }

    fn fake_pi_with_output(output: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("pi");
        fs::write(&executable, format!("#!/bin/sh\nprintf '{output}'\n")).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        (directory, executable)
    }

    #[tokio::test]
    async fn accepts_the_supported_pi_version() {
        let (_directory, executable) = fake_pi(SUPPORTED_PI_VERSION);
        let dependency = PiDependency::from_executable(executable.clone())
            .await
            .unwrap();

        assert_eq!(dependency.executable, executable);
        assert_eq!(dependency.version, SUPPORTED_PI_VERSION);
    }

    #[tokio::test]
    async fn accepts_the_first_line_version_when_more_output_follows() {
        let output = format!("pi {SUPPORTED_PI_VERSION}\\nbuild information\\n");
        let (_directory, executable) = fake_pi_with_output(&output);

        let dependency = PiDependency::from_executable(executable).await.unwrap();

        assert_eq!(dependency.version, SUPPORTED_PI_VERSION);
    }

    #[tokio::test]
    async fn rejects_an_incompatible_pi_version() {
        let (_directory, executable) = fake_pi("0.82.0");
        let error = PiDependency::from_executable(executable).await.unwrap_err();

        assert_matches!(
            error,
            DependencyError::UnsupportedVersion { actual, .. } if actual == "0.82.0"
        );
    }

    #[tokio::test]
    async fn discovers_and_retains_the_absolute_executable_path() {
        let (directory, executable) = fake_pi(SUPPORTED_PI_VERSION);

        let dependency =
            PiDependency::from_search_path(directory.path().as_os_str(), Path::new("/unused"))
                .await
                .unwrap();

        assert_eq!(dependency.executable, executable.canonicalize().unwrap());
        assert!(dependency.executable.is_absolute());
    }
}
