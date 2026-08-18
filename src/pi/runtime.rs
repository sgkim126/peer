use std::path::PathBuf;

use log::{debug, trace};

use crate::cache::CacheStore;

use super::assets::materialize;
use super::dependency::PiDependency;
use super::process::{PiProcess, PiProcessOptions};
use super::runner::{PiRunError, PiRunFailure, PiRunRequest, PiRunResult, PiRunner};
use super::tool_server::ToolServer;

pub struct PiRuntime {
    project_root: PathBuf,
    cache: CacheStore,
    runner: Option<PiRunner>,
}

impl PiRuntime {
    pub fn new(project_root: impl Into<PathBuf>, cache: CacheStore) -> Self {
        Self {
            project_root: project_root.into(),
            cache,
            runner: None,
        }
    }

    pub async fn run(&mut self, request: PiRunRequest) -> Result<PiRunResult, PiRunFailure> {
        if self.runner.is_none() {
            trace!("starting Pi runner");
            self.runner = Some(self.start_runner().await?);
        }
        let result = self
            .runner
            .as_mut()
            .expect("Pi runner was initialized")
            .run(request)
            .await;
        if matches!(
            result,
            Err(PiRunFailure {
                error: PiRunError::Rpc(_),
                ..
            })
        ) {
            debug!("discarding Pi runner after RPC failure");
            self.runner = None;
        }
        result
    }

    async fn start_runner(&self) -> Result<PiRunner, PiRunError> {
        trace!("discovering Pi dependency");
        let dependency = PiDependency::discover().await?;
        trace!(
            "using Pi executable {:?} version {:?}",
            dependency.executable, dependency.version
        );
        let version_root = self.cache.version_root();
        trace!("materializing Pi assets in {version_root:?}");
        let assets = materialize(&version_root)?;
        let session_dir = version_root.join("pi-sessions");
        let agent_dir = version_root.join("pi-agent");
        std::fs::create_dir_all(&session_dir).map_err(PiRunError::Start)?;
        std::fs::create_dir_all(&agent_dir).map_err(PiRunError::Start)?;
        trace!("starting peer tool server");
        let tool_server = ToolServer::start(&self.project_root).map_err(PiRunError::ToolServer)?;
        trace!("starting Pi process");
        let process = PiProcess::spawn(&PiProcessOptions {
            executable: dependency.executable,
            cwd: self.project_root.clone(),
            session_dir,
            extension: assets.extension,
            agent_dir,
            tool_socket: tool_server.socket_path().to_path_buf(),
        })
        .map_err(PiRunError::Start)?;
        trace!("Pi runner ready");
        Ok(PiRunner::new(process, tool_server, self.cache.clone()))
    }
}
