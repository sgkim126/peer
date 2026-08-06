use std::path::PathBuf;

use crate::console::Console;

use super::assets::materialize;
use super::dependency::PiDependency;
use super::process::{PiProcess, PiProcessOptions};
use super::runner::{PiRunError, PiRunRequest, PiRunResult, PiRunner};
use super::tool_server::ToolServer;

pub struct PiRuntime {
    project_root: PathBuf,
    cache_root: PathBuf,
    console: Console,
}

impl PiRuntime {
    #[expect(dead_code)]
    pub fn new(
        project_root: impl Into<PathBuf>,
        cache_root: impl Into<PathBuf>,
        console: Console,
    ) -> Self {
        Self {
            project_root: project_root.into(),
            cache_root: cache_root.into(),
            console,
        }
    }

    #[expect(dead_code)]
    pub async fn run(&self, request: PiRunRequest) -> Result<PiRunResult, PiRunError> {
        let dependency = PiDependency::discover().await?;
        let version_root = self.cache_root.join(crate::cache::CacheKey::version());
        let assets = materialize(&version_root)?;
        let session_dir = version_root.join("pi-sessions");
        let agent_dir = version_root.join("pi-agent");
        std::fs::create_dir_all(&session_dir).map_err(PiRunError::Start)?;
        std::fs::create_dir_all(&agent_dir).map_err(PiRunError::Start)?;
        let tool_server =
            ToolServer::start(&self.project_root, self.console).map_err(PiRunError::ToolServer)?;
        let process = PiProcess::spawn(&PiProcessOptions {
            executable: dependency.executable,
            cwd: self.project_root.clone(),
            session_dir,
            extension: assets.extension,
            agent_dir,
            tool_socket: tool_server.socket_path().to_path_buf(),
        })
        .map_err(PiRunError::Start)?;
        let mut runner = PiRunner::new(process, tool_server);
        runner.run(request).await
    }
}
