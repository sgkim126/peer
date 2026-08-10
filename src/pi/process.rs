use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use tokio::process::{Child, ChildStdin, ChildStdout, Command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiProcessOptions {
    /// An absolute path, a command name resolved through `PATH`, or a path relative to `cwd`.
    pub executable: PathBuf,
    /// The absolute working directory for the process.
    pub cwd: PathBuf,
    pub session_dir: PathBuf,
    pub extension: PathBuf,
    pub agent_dir: PathBuf,
    pub tool_socket: PathBuf,
}

pub struct PiProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl fmt::Debug for PiProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PiProcess")
            .field("id", &self.child.id())
            .finish_non_exhaustive()
    }
}

impl PiProcess {
    pub fn spawn(options: &PiProcessOptions) -> Result<Self, std::io::Error> {
        let mut command = build_command(options);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .expect("Pi command always configures piped stdin");
        let stdout = child
            .stdout
            .take()
            .expect("Pi command always configures piped stdout");
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    pub fn into_parts(self) -> (Child, ChildStdin, ChildStdout) {
        (self.child, self.stdin, self.stdout)
    }
}

fn build_command(options: &PiProcessOptions) -> Command {
    let executable = resolve_executable(&options.executable, &options.cwd);
    let mut command = Command::new(executable.as_ref());
    command
        .arg("--mode")
        .arg("rpc")
        .arg("--session-dir")
        .arg(&options.session_dir)
        .arg("--no-builtin-tools")
        .arg("--no-extensions")
        .arg("-e")
        .arg(&options.extension)
        .arg("--no-skills")
        .arg("--no-prompt-templates")
        .arg("--no-themes")
        .arg("--no-context-files")
        .arg("--no-approve")
        .current_dir(&options.cwd)
        .env("PI_CODING_AGENT_DIR", &options.agent_dir)
        .env("PI_SKIP_VERSION_CHECK", "1")
        .env("PI_TELEMETRY", "0")
        .env("PEER_TOOL_SOCKET", &options.tool_socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    command
}

fn resolve_executable<'a>(executable: &'a Path, cwd: &'a Path) -> std::borrow::Cow<'a, Path> {
    let mut components = executable.components();
    let is_bare_command =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if executable.is_absolute() || is_bare_command {
        executable.into()
    } else {
        cwd.join(executable).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_options_accept_paths_without_shell_parsing() {
        let options = PiProcessOptions {
            executable: Path::new("/tmp/pi executable").to_path_buf(),
            cwd: Path::new("/tmp/repo; echo unsafe").to_path_buf(),
            session_dir: Path::new("/tmp/sessions with spaces").to_path_buf(),
            extension: Path::new("/tmp/extension.ts").to_path_buf(),
            agent_dir: Path::new("/tmp/agent").to_path_buf(),
            tool_socket: Path::new("/tmp/peer-tools.sock").to_path_buf(),
        };

        let command = build_command(&options);
        assert_eq!(command.as_std().get_program(), options.executable);
        assert!(
            command
                .as_std()
                .get_args()
                .any(|argument| argument == options.session_dir)
        );
        assert!(command.as_std().get_envs().any(|(key, value)| {
            key == "PEER_TOOL_SOCKET" && value == Some(options.tool_socket.as_os_str())
        }));
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(options.cwd.as_path())
        );
    }

    #[test]
    fn resolves_a_relative_executable_against_the_process_working_directory() {
        let options = PiProcessOptions {
            executable: Path::new("bin/pi").to_path_buf(),
            cwd: Path::new("/tmp/repo").to_path_buf(),
            session_dir: Path::new("/tmp/sessions").to_path_buf(),
            extension: Path::new("/tmp/extension.ts").to_path_buf(),
            agent_dir: Path::new("/tmp/agent").to_path_buf(),
            tool_socket: Path::new("/tmp/peer-tools.sock").to_path_buf(),
        };

        let command = build_command(&options);

        assert_eq!(
            command.as_std().get_program(),
            Path::new("/tmp/repo/bin/pi")
        );
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(options.cwd.as_path())
        );
    }
}
