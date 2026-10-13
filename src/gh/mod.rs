use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct GhInvocation {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

impl fmt::Debug for GhInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GhInvocation")
            .field("args", &self.args)
            .field("stdin_bytes", &self.stdin.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Debug)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct GhOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug)]
pub enum GhRunError {
    Spawn(io::Error),
    Io(io::Error),
    Timeout,
    NonZeroExit { status: Option<i32>, stderr: String },
}

impl GhRunError {
    /// A failed invocation may already have performed a remote mutation.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn may_have_run(&self) -> bool {
        !matches!(self, Self::Spawn(_))
    }
}

impl fmt::Display for GhRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) if error.kind() == io::ErrorKind::NotFound => write!(
                f,
                "cannot find gh; install GitHub CLI and ensure gh is on PATH: {error}"
            ),
            Self::Spawn(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                write!(
                    f,
                    "cannot execute gh; check its executable permissions: {error}"
                )
            }
            Self::Spawn(error) => write!(f, "cannot start gh: {error}"),
            Self::Io(error) => write!(f, "gh process I/O failed: {error}"),
            Self::Timeout => write!(f, "gh command timed out"),
            Self::NonZeroExit { status, stderr } => write!(
                f,
                "gh command failed with status {}: {stderr}",
                status.map_or_else(|| "signal".to_string(), |status| status.to_string())
            ),
        }
    }
}

impl std::error::Error for GhRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(error) | Self::Io(error) => Some(error),
            Self::Timeout | Self::NonZeroExit { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GhClient {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for GhClient {
    fn default() -> Self {
        Self::new(PathBuf::from("gh"), DEFAULT_TIMEOUT)
    }
}

impl GhClient {
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }

    /// Check installation without accessing the network or requiring authentication.
    pub async fn check_available(&self) -> Result<(), GhRunError> {
        let output = self
            .run(&GhInvocation {
                args: vec!["--version".to_string()],
                stdin: None,
            })
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GhRunError::NonZeroExit {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Preserve unsuccessful exit output so commands can interpret HTTP responses.
    pub async fn run(&self, invocation: &GhInvocation) -> Result<GhOutput, GhRunError> {
        let mut child = self
            .build_command(invocation)
            .spawn()
            .map_err(GhRunError::Spawn)?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("gh stdout is always piped");
        let mut stderr = child.stderr.take().expect("gh stderr is always piped");
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();

        let result = tokio::time::timeout(self.timeout, async {
            tokio::join!(
                async {
                    if let Some(mut stdin) = stdin {
                        stdin
                            .write_all(invocation.stdin.as_deref().unwrap_or_default())
                            .await?;
                        stdin.shutdown().await?;
                    }
                    Ok::<(), io::Error>(())
                },
                stdout.read_to_end(&mut stdout_bytes),
                stderr.read_to_end(&mut stderr_bytes),
                child.wait(),
            )
        })
        .await;

        let (input, stdout, stderr, status) = match result {
            Ok(result) => result,
            Err(_) => {
                // kill also waits, so a timed-out child is reaped before returning.
                let _ = child.kill().await;
                return Err(GhRunError::Timeout);
            }
        };
        stdout.map_err(GhRunError::Io)?;
        stderr.map_err(GhRunError::Io)?;
        let status = status.map_err(GhRunError::Io)?;
        // A failed command can close stdin early while still returning useful HTTP
        // headers and a response body. Let its command decoder inspect that output.
        if status.success() {
            input.map_err(GhRunError::Io)?;
        }
        Ok(GhOutput {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    }

    fn build_command(&self, invocation: &GhInvocation) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .args(&invocation.args)
            .stdin(if invocation.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("GH_PAGER", "cat")
            .env_remove("GH_FORCE_TTY")
            // An explicit false value also suppresses gh's legacy DEBUG fallback.
            .env("GH_DEBUG", "0")
            .kill_on_drop(true);
        command
    }
}

#[cfg(test)]
mod tests;
