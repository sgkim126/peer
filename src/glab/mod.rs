use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct GlabInvocation {
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
}

impl fmt::Debug for GlabInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlabInvocation")
            .field("args", &self.args)
            .field("stdin_bytes", &self.stdin.as_ref().map(Vec::len))
            .finish()
    }
}

pub struct GlabOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl fmt::Debug for GlabOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlabOutput")
            .field("status", &self.status)
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .finish()
    }
}

#[derive(Debug)]
pub enum GlabRunError {
    Spawn(io::Error),
    Io(io::Error),
    Timeout,
    NonZeroExit { status: Option<i32>, stderr: String },
}

impl GlabRunError {
    /// A failed invocation may already have performed a remote mutation.
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn may_have_run(&self) -> bool {
        !matches!(self, Self::Spawn(_))
    }
}

impl fmt::Display for GlabRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) if error.kind() == io::ErrorKind::NotFound => write!(
                f,
                "cannot find glab; install GitLab CLI and ensure glab is on PATH: {error}"
            ),
            Self::Spawn(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                write!(
                    f,
                    "cannot execute glab; check its executable permissions: {error}"
                )
            }
            Self::Spawn(error) => write!(f, "cannot start glab: {error}"),
            Self::Io(error) => write!(f, "glab process I/O failed: {error}"),
            Self::Timeout => write!(f, "glab command timed out"),
            Self::NonZeroExit { status, stderr } => write!(
                f,
                "glab command failed with status {}: {stderr}",
                status.map_or_else(|| "signal".to_string(), |status| status.to_string())
            ),
        }
    }
}

impl std::error::Error for GlabRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Timeout => None,
            Self::NonZeroExit { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GlabClient {
    executable: PathBuf,
    timeout: Duration,
}

impl Default for GlabClient {
    fn default() -> Self {
        Self::new(PathBuf::from("glab"), DEFAULT_TIMEOUT)
    }
}

impl GlabClient {
    pub fn new(executable: PathBuf, timeout: Duration) -> Self {
        Self {
            executable,
            timeout,
        }
    }

    /// Check installation without accessing the network or requiring authentication.
    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn check_available(&self) -> Result<(), GlabRunError> {
        let output = self
            .run(&GlabInvocation {
                args: vec!["--version".to_string()],
                stdin: None,
            })
            .await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(GlabRunError::NonZeroExit {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            })
        }
    }

    /// Preserve unsuccessful exit output so commands can interpret HTTP responses.
    pub async fn run(&self, invocation: &GlabInvocation) -> Result<GlabOutput, GlabRunError> {
        let mut child = self
            .build_command(invocation)
            .spawn()
            .map_err(GlabRunError::Spawn)?;
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("glab stdout is always piped");
        let mut stderr = child.stderr.take().expect("glab stderr is always piped");
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
                return Err(GlabRunError::Timeout);
            }
        };
        stdout.map_err(GlabRunError::Io)?;
        stderr.map_err(GlabRunError::Io)?;
        let status = status.map_err(GlabRunError::Io)?;
        // A failed command can close stdin early while still returning useful HTTP
        // headers and a response body. Let its command decoder inspect that output.
        if status.success() {
            input.map_err(GlabRunError::Io)?;
        }
        Ok(GlabOutput {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    }

    fn build_command(&self, invocation: &GlabInvocation) -> Command {
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
            // Some glab versions check the legacy variable before GLAB_NO_PROMPT.
            .env("GLAB_NO_PROMPT", "1")
            .env("NO_PROMPT", "1")
            .env("PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("GLAB_PAGER", "cat")
            .env("GLAB_DISPLAY_HYPERLINKS", "false")
            .env("DISPLAY_HYPERLINKS", "false")
            .env_remove("GLAB_FORCE_HYPERLINKS")
            .env_remove("FORCE_HYPERLINKS")
            // Disable both application and HTTP debugging, including legacy names.
            .env("GLAB_DEBUG", "false")
            .env("GLAB_DEBUG_HTTP", "false")
            .env("DEBUG", "false")
            .env("DEBUG_HTTP", "false")
            // Availability and API invocations must not start unrelated requests.
            .env("GLAB_CHECK_UPDATE", "false")
            .env("CHECK_UPDATE", "false")
            .env("GLAB_SEND_TELEMETRY", "false")
            .env("GLAB_SHOW_WHATS_NEW", "false")
            .kill_on_drop(true);
        command
    }
}

#[cfg(test)]
mod tests;
