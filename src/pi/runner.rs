use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::process::Child;

use crate::cache::{CacheKey, CacheReadError, CacheStore, CacheWriteError};
use crate::llm::{LlmModelUsage, LlmUsage};

use super::assets::AssetError;
use super::dependency::DependencyError;
use super::model::ModelRef;
use super::process::PiProcess;
use super::protocol::RunConfig;
use super::rpc::{RpcClient, RpcError};
use super::tool_server::ToolServer;

#[derive(Debug)]
pub struct PiRunRequest {
    pub session_key: CacheKey,
    pub config: RunConfig,
    pub model: ModelRef,
    pub prompt: String,
    pub resume: bool,
}

#[derive(Debug)]
#[expect(dead_code)]
pub struct PiRunResult {
    pub outcome: Value,
    pub iterations: u32,
    pub usage: LlmUsage,
    pub session_id: String,
}

#[derive(Debug)]
pub struct PiRunFailure {
    pub error: PiRunError,
    pub usage: Option<LlmUsage>,
}

impl From<PiRunError> for PiRunFailure {
    fn from(error: PiRunError) -> Self {
        Self { error, usage: None }
    }
}

impl From<RpcError> for PiRunFailure {
    fn from(error: RpcError) -> Self {
        PiRunError::from(error).into()
    }
}

impl fmt::Display for PiRunFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for PiRunFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug)]
pub enum PiRunError {
    Dependency(DependencyError),
    Assets(AssetError),
    Start(std::io::Error),
    ToolServer(std::io::Error),
    Rpc(RpcError),
    CacheRead(CacheReadError),
    CacheWrite(CacheWriteError),
    UnsafeSessionPath(PathBuf),
    InvalidState(String),
    Exhausted { turns: u32 },
}

impl PiRunError {
    fn session_status(&self) -> SessionStatus {
        match self {
            Self::Dependency(_) => SessionStatus::FailedTerminal,
            Self::Assets(_) => SessionStatus::FailedTerminal,
            Self::Start(_) => SessionStatus::FailedTerminal,
            Self::ToolServer(_) => SessionStatus::FailedTerminal,
            Self::Rpc(_) => SessionStatus::FailedTransient,
            Self::CacheRead(_) => SessionStatus::FailedTerminal,
            Self::CacheWrite(_) => SessionStatus::FailedTerminal,
            Self::UnsafeSessionPath(_) => SessionStatus::FailedTerminal,
            Self::InvalidState(_) => SessionStatus::FailedTerminal,
            Self::Exhausted { .. } => SessionStatus::Exhausted,
        }
    }
}

impl fmt::Display for PiRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dependency(error) => error.fmt(f),
            Self::Assets(error) => error.fmt(f),
            Self::Start(error) => write!(f, "cannot start Pi RPC process: {error}"),
            Self::ToolServer(error) => write!(f, "cannot start peer tool server: {error}"),
            Self::Rpc(error) => error.fmt(f),
            Self::CacheRead(error) => write!(f, "cannot read Pi session cache: {error}"),
            Self::CacheWrite(error) => write!(f, "cannot write Pi session cache: {error}"),
            Self::UnsafeSessionPath(path) => {
                write!(
                    f,
                    "Pi session path is outside the cache: {}",
                    path.display()
                )
            }
            Self::InvalidState(reason) => write!(f, "Pi RPC returned invalid state: {reason}"),
            Self::Exhausted { turns, .. } => {
                write!(f, "Pi did not submit an outcome within {turns} turns")
            }
        }
    }
}

impl std::error::Error for PiRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Dependency(error) => Some(error),
            Self::Assets(error) => Some(error),
            Self::Start(error) => Some(error),
            Self::ToolServer(error) => Some(error),
            Self::Rpc(error) => Some(error),
            Self::CacheRead(error) => Some(error),
            Self::CacheWrite(error) => Some(error),
            Self::UnsafeSessionPath(_) => None,
            Self::InvalidState(_) => None,
            Self::Exhausted { .. } => None,
        }
    }
}

impl From<DependencyError> for PiRunError {
    fn from(error: DependencyError) -> Self {
        Self::Dependency(error)
    }
}

impl From<AssetError> for PiRunError {
    fn from(error: AssetError) -> Self {
        Self::Assets(error)
    }
}

impl From<RpcError> for PiRunError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

impl From<CacheReadError> for PiRunError {
    fn from(error: CacheReadError) -> Self {
        Self::CacheRead(error)
    }
}

impl From<CacheWriteError> for PiRunError {
    fn from(error: CacheWriteError) -> Self {
        Self::CacheWrite(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionStatus {
    Running,
    Exhausted,
    FailedTransient,
    Completed,
    FailedTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionRecord {
    status: SessionStatus,
    session_id: String,
    session_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_usage_entry_id: Option<String>,
}

impl SessionRecord {
    fn resumable(&self) -> bool {
        match self.status {
            SessionStatus::Running => true,
            SessionStatus::Exhausted => true,
            SessionStatus::FailedTransient => true,
            SessionStatus::Completed => false,
            SessionStatus::FailedTerminal => false,
        }
    }
}

type ProcessClient = RpcClient<BufReader<tokio::process::ChildStdout>, tokio::process::ChildStdin>;

#[expect(dead_code)]
pub struct PiRunner {
    child: Child,
    client: ProcessClient,
    cache: CacheStore,
    version_root: PathBuf,
    _tool_server: ToolServer,
}

impl PiRunner {
    pub fn new(process: PiProcess, tool_server: ToolServer, cache: CacheStore) -> Self {
        let (child, stdin, stdout) = process.into_parts();
        let version_root = cache.version_root();
        Self {
            child,
            client: RpcClient::new(BufReader::new(stdout), stdin),
            cache,
            version_root,
            _tool_server: tool_server,
        }
    }

    pub async fn run(&mut self, request: PiRunRequest) -> Result<PiRunResult, PiRunFailure> {
        let existing = if request.resume {
            match self.cache.read_json(&request.session_key) {
                Ok(record) => record,
                Err(CacheReadError::Deserialize { .. }) => None,
                Err(error) => return Err(PiRunError::from(error).into()),
            }
        } else {
            None
        };
        let (mut record, continuation) = match existing.filter(SessionRecord::resumable) {
            Some(record) => {
                if let Err(error) = self.switch_session(&record).await {
                    let mut failed = record;
                    failed.status = error.session_status();
                    if let Err(write_error) = self.write_session(&request.session_key, &failed) {
                        warn!("cannot persist Pi session state: {write_error}");
                    }
                    return Err(error.into());
                }
                (record, true)
            }
            None => {
                let response = self
                    .client
                    .request(json!({
                        "type": "new_session"
                    }))
                    .await?;
                ensure_not_cancelled(&response.data, "new_session")?;
                (self.current_session().await?, false)
            }
        };
        record.status = SessionStatus::Running;
        self.write_session(&request.session_key, &record)?;

        let result = self
            .run_configured(&request, &mut record, continuation)
            .await;
        record.status = result.as_ref().map_or_else(
            |failure| failure.error.session_status(),
            |_| SessionStatus::Completed,
        );
        if let Err(error) = self.write_session(&request.session_key, &record) {
            warn!("cannot persist Pi session state: {error}");
        }
        result
    }

    async fn switch_session(&mut self, record: &SessionRecord) -> Result<(), PiRunError> {
        validate_relative_path(&record.session_path)?;
        let session_path = self.version_root.join(&record.session_path);
        let session_path = session_path
            .to_str()
            .ok_or_else(|| PiRunError::UnsafeSessionPath(session_path.to_path_buf()))?;
        let response = self
            .client
            .request(json!({
                "type": "switch_session",
                "sessionPath": session_path
            }))
            .await?;
        ensure_not_cancelled(&response.data, "switch_session")?;
        let actual = self.current_session().await?;
        if actual.session_id != record.session_id || actual.session_path != record.session_path {
            return Err(PiRunError::InvalidState(
                "switched Pi session does not match the cached session".to_string(),
            ));
        }
        Ok(())
    }

    async fn current_session(&mut self) -> Result<SessionRecord, PiRunError> {
        let response = self
            .client
            .request(json!({
                "type": "get_state"
            }))
            .await?;
        let data = response
            .data
            .ok_or_else(|| PiRunError::InvalidState("get_state omitted data".to_string()))?;
        let session_id = data
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PiRunError::InvalidState("missing sessionId".to_string()))?;
        let session_file = PathBuf::from(
            data.get("sessionFile")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| PiRunError::InvalidState("missing sessionFile".to_string()))?,
        );
        let session_path = session_file.strip_prefix(&self.version_root).map_err(|_| {
            PiRunError::InvalidState(format!(
                "session file {} is outside {}",
                session_file.display(),
                self.version_root.display()
            ))
        })?;
        validate_relative_path(session_path)?;
        Ok(SessionRecord {
            status: SessionStatus::Running,
            session_id,
            session_path: session_path.to_path_buf(),
            last_usage_entry_id: None,
        })
    }

    fn write_session(&self, key: &CacheKey, record: &SessionRecord) -> Result<(), PiRunError> {
        validate_relative_path(&record.session_path)?;
        self.cache.write_json(key, record)?;
        Ok(())
    }

    async fn run_configured(
        &mut self,
        request: &PiRunRequest,
        record: &mut SessionRecord,
        continuation: bool,
    ) -> Result<PiRunResult, PiRunFailure> {
        let config_bytes = serde_json::to_vec(&request.config)
            .map_err(|error| PiRunError::InvalidState(error.to_string()))?;
        let digest = blake3::hash(&config_bytes).to_hex().to_string();
        let envelope = ConfigureEnvelope {
            digest: &digest,
            config: &request.config,
        };
        let encoded = URL_SAFE_NO_PAD.encode(
            &serde_json::to_vec(&envelope)
                .map_err(|error| PiRunError::InvalidState(error.to_string()))?,
        );
        self.client
            .request(json!({
                "type": "prompt",
                "message": format!("/peer-configure-v1 {encoded}")
            }))
            .await?;
        wait_for_configuration(&mut self.client, &digest).await?;
        let response = self
            .client
            .request(json!({
                "type": "set_model",
                "provider": request.model.provider(),
                "modelId": request.model.model()
            }))
            .await?;
        ensure_not_cancelled(&response.data, "set_model")?;
        let message = if continuation {
            format!("/peer-continue-v1 {digest}")
        } else {
            request.prompt.clone()
        };
        self.client
            .request(json!({
                "type": "prompt",
                "message": message
            }))
            .await?;

        let outcome = match self
            .wait_for_outcome(&digest, request.config.max_turns)
            .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                if matches!(&error, PiRunError::Rpc(_)) {
                    // Do not request usage over a failed RPC connection
                    return Err(error.into());
                }
                let usage = match self.read_usage(record).await {
                    Ok(usage) => Some(usage),
                    Err(usage_error) => {
                        debug!("cannot read Pi usage after run failure: {usage_error:?}");
                        None
                    }
                };
                return Err(PiRunFailure { error, usage });
            }
        };
        match outcome {
            WaitOutcome::Completed {
                outcome,
                iterations,
            } => {
                let usage = match self.read_usage(record).await {
                    Ok(usage) => usage,
                    Err(error) => {
                        warn!("cannot read Pi usage: {error}");
                        LlmUsage::zero(request.model.to_string())
                    }
                };
                Ok(PiRunResult {
                    outcome,
                    iterations,
                    usage,
                    session_id: record.session_id.clone(),
                })
            }
            WaitOutcome::Exhausted { turns } => {
                let usage = match self.read_usage(record).await {
                    Ok(usage) => usage,
                    Err(error) => {
                        warn!("cannot read Pi usage: {error}");
                        LlmUsage::zero(request.model.to_string())
                    }
                };
                Err(PiRunFailure {
                    error: PiRunError::Exhausted { turns },
                    usage: Some(usage),
                })
            }
        }
    }

    async fn read_usage(&mut self, record: &mut SessionRecord) -> Result<LlmUsage, PiRunError> {
        let entries_command = match &record.last_usage_entry_id {
            Some(entry_id) => json!({
                "type": "get_entries",
                "since": entry_id
            }),
            None => json!({
                "type": "get_entries"
            }),
        };
        let entries = self
            .client
            .request(entries_command)
            .await?
            .data
            .ok_or_else(|| PiRunError::InvalidState("get_entries omitted data".to_string()))?;
        let (usage, leaf_id) = usage_from_entries(&entries)?;
        if let Some(leaf_id) = leaf_id {
            record.last_usage_entry_id = Some(leaf_id);
        }
        Ok(usage)
    }

    async fn wait_for_outcome(
        &mut self,
        digest: &str,
        max_turns: u32,
    ) -> Result<WaitOutcome, PiRunError> {
        let mut outcome = None;
        let mut turns = 0;
        loop {
            let event = self.client.next_event().await?;
            match event.get("type").and_then(Value::as_str) {
                Some("turn_end") => turns += 1,
                Some("tool_execution_end") => {
                    if outcome.is_none()
                        && event
                            .pointer("/result/details/type")
                            .and_then(Value::as_str)
                            == Some("peer.outcome")
                    {
                        outcome = Some(
                            event
                                .pointer("/result/details/outcome")
                                .cloned()
                                .ok_or_else(|| {
                                    PiRunError::InvalidState(
                                        "peer.outcome omitted outcome".to_string(),
                                    )
                                })?,
                        );
                        self.client
                            .request(json!({
                                "type": "abort"
                            }))
                            .await?;
                    }
                }
                Some("agent_settled") => {
                    if let Some(outcome) = outcome {
                        return Ok(WaitOutcome::Completed {
                            outcome,
                            iterations: turns,
                        });
                    }
                    if turns >= max_turns {
                        return Ok(WaitOutcome::Exhausted { turns });
                    }
                    self.client
                        .request(json!({
                            "type": "prompt",
                            "message": format!("/peer-continue-v1 {digest}")
                        }))
                        .await?;
                }
                Some("extension_error") => {
                    debug!("Pi extension error event: {event:?}");
                    return Err(PiRunError::InvalidState("Pi extension failed".to_string()));
                }
                _ => {}
            }
        }
    }
}

enum WaitOutcome {
    Completed { outcome: Value, iterations: u32 },
    Exhausted { turns: u32 },
}

async fn wait_for_configuration<R, W>(
    client: &mut RpcClient<R, W>,
    digest: &str,
) -> Result<(), PiRunError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let expected = format!("peer.configured:{digest}");
    let mut configured = false;
    let mut turn_ended = false;
    loop {
        let event = client.next_event().await?;
        match event.get("type").and_then(Value::as_str) {
            Some("extension_ui_request")
                if event.get("method").and_then(Value::as_str) == Some("notify")
                    && event.get("message").and_then(Value::as_str) == Some(expected.as_str()) =>
            {
                configured = true;
            }
            Some("turn_end") if configured => turn_ended = true,
            Some("agent_settled") if turn_ended => return Ok(()),
            Some("extension_error") => {
                debug!("Pi extension configuration error event: {event:?}");
                return Err(PiRunError::InvalidState(
                    "extension configuration failed".to_string(),
                ));
            }
            _ => {}
        }
    }
}

fn ensure_not_cancelled(data: &Option<Value>, command: &str) -> Result<(), PiRunError> {
    if data
        .as_ref()
        .and_then(|data| data.get("cancelled"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Err(PiRunError::InvalidState(format!("Pi cancelled {command}")));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), PiRunError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PiRunError::UnsafeSessionPath(path.to_path_buf()));
    }
    Ok(())
}

#[derive(Serialize)]
struct ConfigureEnvelope<'a> {
    digest: &'a str,
    config: &'a RunConfig,
}

fn usage_from_entries(data: &Value) -> Result<(LlmUsage, Option<String>), PiRunError> {
    let entries = data
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| PiRunError::InvalidState("get_entries omitted entries".to_string()))?;
    let mut by_model = BTreeMap::<(String, String), LlmModelUsage>::new();
    for message in entries
        .iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("message"))
        .filter_map(|entry| entry.get("message"))
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
    {
        let Some(provider) = message.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let Some(model) = message.get("model").and_then(Value::as_str) else {
            continue;
        };
        let usage = message.get("usage").unwrap_or(&Value::Null);
        let total = by_model
            .entry((provider.to_string(), model.to_string()))
            .or_insert_with(|| LlmModelUsage {
                provider: provider.to_string(),
                model: model.to_string(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: 0.0,
            });
        total.input_tokens += usage.get("input").and_then(Value::as_u64).unwrap_or(0);
        total.output_tokens += usage.get("output").and_then(Value::as_u64).unwrap_or(0);
        total.cache_read_tokens += usage.get("cacheRead").and_then(Value::as_u64).unwrap_or(0);
        total.cache_write_tokens += usage.get("cacheWrite").and_then(Value::as_u64).unwrap_or(0);
        total.cost_usd += usage
            .pointer("/cost/total")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    }
    let leaf_id = data
        .get("leafId")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((
        LlmUsage::from_pi_models(by_model.into_values().collect()),
        leaf_id,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use tokio::io::{AsyncWriteExt, BufReader, duplex};

    #[tokio::test]
    async fn drains_the_configuration_turn_before_returning() {
        let (client_reader, mut server_writer) = duplex(1024);
        let mut client = RpcClient::new(BufReader::new(client_reader), tokio::io::sink());
        let server = tokio::spawn(async move {
            server_writer
                .write_all(
                    b"{\"type\":\"extension_ui_request\",\"method\":\"notify\",\"message\":\"peer.configured:digest\"}\n\
                      {\"type\":\"turn_end\"}\n\
                      {\"type\":\"agent_settled\"}\n\
                      {\"type\":\"agent_start\"}\n",
                )
                .await
                .unwrap();
        });

        wait_for_configuration(&mut client, "digest").await.unwrap();

        assert_eq!(
            client.next_event().await.unwrap(),
            json!({
                "type": "agent_start"
            })
        );
        server.await.unwrap();
    }

    #[test]
    fn aggregates_pi_usage_by_provider_and_model() {
        let data = json!({
            "entries": [
                {
                    "type": "message",
                    "message": {
                        "role": "assistant",
                        "provider": "mistral",
                        "model": "medium",
                        "usage": {
                            "input": 10,
                            "output": 2,
                            "cacheRead": 7,
                            "cacheWrite": 1,
                            "cost": {
                                "total": 0.02
                            }
                        }
                    }
                },
                {
                    "type": "message",
                    "message": {
                        "role": "assistant",
                        "provider": "mistral",
                        "model": "medium",
                        "usage": {
                            "input": 20,
                            "output": 3,
                            "cacheRead": 9,
                            "cacheWrite": 0,
                            "cost": {
                                "total": 0.03
                            }
                        }
                    }
                }
            ],
            "leafId": "entry-2"
        });

        let (usage, leaf_id) = usage_from_entries(&data).unwrap();
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 16);
        assert_eq!(usage.cache_write_tokens, 1);
        assert!((usage.cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(usage.models.len(), 1);
        assert_eq!(leaf_id.as_deref(), Some("entry-2"));
    }

    #[test]
    fn rejects_cancelled_session_creation() {
        let error = ensure_not_cancelled(
            &Some(json!({
                "cancelled": true
            })),
            "new_session",
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled new_session"));
    }

    fn session_record(status: SessionStatus) -> SessionRecord {
        SessionRecord {
            status,
            session_id: "session-1".to_string(),
            session_path: PathBuf::from("pi-sessions/session.jsonl"),
            last_usage_entry_id: None,
        }
    }

    #[test]
    fn resumes_running_sessions() {
        assert!(session_record(SessionStatus::Running).resumable());
    }

    #[test]
    fn resumes_exhausted_sessions() {
        assert!(session_record(SessionStatus::Exhausted).resumable());
    }

    #[test]
    fn resumes_transiently_failed_sessions() {
        assert!(session_record(SessionStatus::FailedTransient).resumable());
    }

    #[test]
    fn does_not_resume_completed_sessions() {
        assert!(!session_record(SessionStatus::Completed).resumable());
    }

    #[test]
    fn does_not_resume_terminally_failed_sessions() {
        assert!(!session_record(SessionStatus::FailedTerminal).resumable());
    }

    #[test]
    fn marks_session_switch_mismatches_as_terminal() {
        let error = PiRunError::InvalidState(
            "switched Pi session does not match the cached session".to_string(),
        );
        assert_eq!(error.session_status(), SessionStatus::FailedTerminal);
    }

    #[test]
    fn rejects_cancelled_session_switches() {
        let error = ensure_not_cancelled(
            &Some(json!({
                "cancelled": true
            })),
            "switch_session",
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled switch_session"));
    }

    #[test]
    fn rejects_session_path_traversal() {
        assert_matches!(
            validate_relative_path(Path::new("../outside.jsonl")),
            Err(PiRunError::UnsafeSessionPath(_))
        );
        assert_matches!(
            validate_relative_path(Path::new("pi-sessions/session.jsonl")),
            Ok(_)
        );
    }
}
