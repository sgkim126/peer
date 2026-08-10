use std::path::PathBuf;

use crate::cache::CacheStore;
use crate::console::Console;

use super::assets::materialize;
use super::dependency::PiDependency;
use super::process::{PiProcess, PiProcessOptions};
use super::runner::{PiRunError, PiRunRequest, PiRunResult, PiRunner};
use super::tool_server::ToolServer;

pub struct PiRuntime {
    project_root: PathBuf,
    console: Console,
    cache: CacheStore,
    runner: Option<PiRunner>,
}

impl PiRuntime {
    pub fn new(project_root: impl Into<PathBuf>, cache: CacheStore, console: Console) -> Self {
        Self {
            project_root: project_root.into(),
            cache,
            console,
            runner: None,
        }
    }

    pub async fn run(&mut self, request: PiRunRequest) -> Result<PiRunResult, PiRunError> {
        if self.runner.is_none() {
            self.runner = Some(self.start_runner().await?);
        }
        let result = self
            .runner
            .as_mut()
            .expect("Pi runner was initialized")
            .run(request)
            .await;
        if matches!(result, Err(PiRunError::Rpc(_))) {
            self.runner = None;
        }
        result
    }

    async fn start_runner(&self) -> Result<PiRunner, PiRunError> {
        let dependency = PiDependency::discover().await?;
        let version_root = self.cache.version_root();
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
        Ok(PiRunner::new(
            process,
            tool_server,
            self.cache.clone(),
            self.console,
        ))
    }
}
