use std::collections::BTreeMap;
use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::process::Child;

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
    pub config: RunConfig,
    pub model: ModelRef,
    pub prompt: String,
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
#[expect(dead_code)]
pub enum PiRunError {
    Dependency(DependencyError),
    Assets(AssetError),
    Start(std::io::Error),
    ToolServer(std::io::Error),
    Rpc(RpcError),
    InvalidState(String),
    Exhausted { turns: u32 },
}

impl fmt::Display for PiRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dependency(error) => error.fmt(f),
            Self::Assets(error) => error.fmt(f),
            Self::Start(error) => write!(f, "cannot start Pi RPC process: {error}"),
            Self::ToolServer(error) => write!(f, "cannot start peer tool server: {error}"),
            Self::Rpc(error) => error.fmt(f),
            Self::InvalidState(reason) => write!(f, "Pi RPC returned invalid state: {reason}"),
            Self::Exhausted { turns } => {
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

type ProcessClient = RpcClient<BufReader<tokio::process::ChildStdout>, tokio::process::ChildStdin>;

#[expect(dead_code)]
pub struct PiRunner {
    child: Child,
    client: ProcessClient,
    _tool_server: ToolServer,
}

impl PiRunner {
    #[expect(dead_code)]
    pub fn new(process: PiProcess, tool_server: ToolServer) -> Self {
        let (child, stdin, stdout) = process.into_parts();
        Self {
            child,
            client: RpcClient::new(BufReader::new(stdout), stdin),
            _tool_server: tool_server,
        }
    }

    #[expect(dead_code)]
    pub async fn run(&mut self, request: PiRunRequest) -> Result<PiRunResult, PiRunError> {
        let response = self
            .client
            .request(json!({
                "type": "new_session"
            }))
            .await?;
        let new_session_was_cancelled = response
            .data
            .as_ref()
            .and_then(|data| data.get("cancelled"))
            .and_then(Value::as_bool)
            == Some(true);

        if new_session_was_cancelled {
            return Err(PiRunError::InvalidState(
                "new_session was cancelled".to_string(),
            ));
        }
        let session_id = self.current_session_id().await?;
        self.run_configured(&request, session_id).await
    }

    async fn current_session_id(&mut self) -> Result<String, PiRunError> {
        let response = self
            .client
            .request(json!({
                "type": "get_state"
            }))
            .await?;
        let data = response
            .data
            .ok_or_else(|| PiRunError::InvalidState("get_state omitted data".to_string()))?;

        data.get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PiRunError::InvalidState("missing sessionId".to_string()))
    }

    async fn run_configured(
        &mut self,
        request: &PiRunRequest,
        session_id: String,
    ) -> Result<PiRunResult, PiRunError> {
        let config_bytes = serde_json::to_vec(&request.config)
            .map_err(|error| PiRunError::InvalidState(error.to_string()))?;
        let digest = blake3::hash(&config_bytes).to_hex().to_string();
        let envelope = ConfigureEnvelope {
            digest: &digest,
            config: &request.config,
        };
        let encoded = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&envelope)
                .map_err(|error| PiRunError::InvalidState(error.to_string()))?,
        );
        self.client
            .request(json!({
                "type": "prompt",
                "message": format!("/peer-configure-v1 {encoded}")
            }))
            .await?;
        wait_for_configuration(&mut self.client, &digest).await?;
        self.client
            .request(json!({
                "type": "set_model",
                "provider": request.model.provider(),
                "modelId": request.model.model()
            }))
            .await?;
        self.client
            .request(json!({
                "type": "prompt",
                "message": request.prompt
            }))
            .await?;

        let (outcome, iterations) = self
            .wait_for_outcome(&digest, request.config.max_turns)
            .await?;
        let entries = self
            .client
            .request(json!({
                "type": "get_entries"
            }))
            .await?
            .data
            .ok_or_else(|| PiRunError::InvalidState("get_entries omitted data".to_string()))?;
        let usage = usage_from_entries(&entries)?;
        Ok(PiRunResult {
            outcome,
            iterations,
            usage,
            session_id,
        })
    }

    async fn wait_for_outcome(
        &mut self,
        digest: &str,
        max_turns: u32,
    ) -> Result<(Value, u32), PiRunError> {
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
                        let terminal_outcome = event
                            .pointer("/result/details/outcome")
                            .cloned()
                            .ok_or_else(|| {
                                PiRunError::InvalidState("peer.outcome omitted outcome".to_string())
                            })?;
                        outcome = Some(terminal_outcome);
                        self.client
                            .request(json!({
                                "type": "abort"
                            }))
                            .await?;
                    }
                }
                Some("agent_settled") => {
                    if let Some(outcome) = outcome {
                        return Ok((outcome, turns));
                    }
                    if turns >= max_turns {
                        return Err(PiRunError::Exhausted { turns });
                    }
                    self.client
                        .request(json!({
                            "type": "prompt",
                            "message": format!("/peer-continue-v1 {digest}")
                        }))
                        .await?;
                }
                Some("extension_error") => {
                    return Err(PiRunError::InvalidState("Pi extension failed".to_string()));
                }
                _ => {}
            }
        }
    }
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
                return Err(PiRunError::InvalidState(
                    "extension configuration failed".to_string(),
                ));
            }
            _ => {}
        }
    }
}

#[derive(Serialize)]
struct ConfigureEnvelope<'a> {
    digest: &'a str,
    config: &'a RunConfig,
}

fn usage_from_entries(data: &Value) -> Result<LlmUsage, PiRunError> {
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
    Ok(LlmUsage::from_pi_models(by_model.into_values().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ]
        });

        let usage = usage_from_entries(&data).unwrap();
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 16);
        assert_eq!(usage.cache_write_tokens, 1);
        assert!((usage.cost_usd - 0.05).abs() < 1e-9);
        assert_eq!(usage.models.len(), 1);
    }
}
