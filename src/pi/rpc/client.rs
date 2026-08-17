use std::collections::VecDeque;
use std::fmt;

use log::trace;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncWrite};

use super::codec::{CodecError, read_record, write_record};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RpcResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub command: String,
    pub success: bool,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RpcRequest {
    pub id: String,
    #[serde(flatten)]
    pub command: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub enum RpcError {
    Codec(CodecError),
    UnusableAfterOversizedRecord,
    InvalidCommand,
    ReservedCommandId,
    Rejected { command: String, reason: String },
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => error.fmt(f),
            Self::UnusableAfterOversizedRecord => {
                f.write_str("Pi RPC client is unusable after an oversized record")
            }
            Self::InvalidCommand => f.write_str("Pi RPC command must be a JSON object"),
            Self::ReservedCommandId => {
                f.write_str("Pi RPC command must not contain the reserved `id` field")
            }
            Self::Rejected { command, reason } => {
                write!(f, "Pi RPC command {command} was rejected: {reason}")
            }
        }
    }
}

impl std::error::Error for RpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            Self::UnusableAfterOversizedRecord => None,
            Self::InvalidCommand => None,
            Self::ReservedCommandId => None,
            Self::Rejected { .. } => None,
        }
    }
}

impl From<CodecError> for RpcError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

pub struct RpcClient<R, W> {
    reader: R,
    writer: W,
    events: VecDeque<Value>,
    next_id: u64,
    unusable: bool,
}

impl<R, W> RpcClient<R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader,
            writer,
            events: VecDeque::new(),
            next_id: 1,
            unusable: false,
        }
    }

    pub async fn request(&mut self, command: Value) -> Result<RpcResponse, RpcError> {
        self.ensure_usable()?;
        let Value::Object(command) = command else {
            return Err(RpcError::InvalidCommand);
        };
        if command.contains_key("id") {
            return Err(RpcError::ReservedCommandId);
        }
        let command_type = command
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let request = RpcRequest {
            id: format!("peer-{}", self.next_id),
            command,
        };
        self.next_id += 1;
        trace!(
            "Pi RPC request: id={:?} command={command_type:?}",
            request.id
        );
        write_record(&mut self.writer, &request).await?;

        loop {
            let value = self.read_value().await?;
            if value.get("type").and_then(Value::as_str) != Some("response") {
                self.events.push_back(value);
                continue;
            }
            let response = RpcResponse::deserialize(&value).map_err(CodecError::from)?;
            if response.id != request.id {
                self.events.push_back(value);
                continue;
            }
            trace!(
                "Pi RPC response: id={:?} success={}",
                response.id, response.success
            );
            return if response.success {
                Ok(response)
            } else {
                Err(RpcError::Rejected {
                    command: response.command,
                    reason: response
                        .error
                        .unwrap_or_else(|| "unknown error".to_string()),
                })
            };
        }
    }

    pub async fn next_event(&mut self) -> Result<Value, RpcError> {
        self.ensure_usable()?;
        match self.events.pop_front() {
            Some(event) => Ok(event),
            None => self.read_value().await,
        }
    }

    fn ensure_usable(&self) -> Result<(), RpcError> {
        if self.unusable {
            Err(RpcError::UnusableAfterOversizedRecord)
        } else {
            Ok(())
        }
    }

    async fn read_value(&mut self) -> Result<Value, RpcError> {
        match read_record::<_, Value>(&mut self.reader).await {
            Ok(value) => {
                trace!(
                    "Pi RPC record: type={:?} id={:?}",
                    value.get("type").and_then(Value::as_str),
                    value.get("id").and_then(Value::as_str)
                );
                Ok(value)
            }
            Err(error) => {
                if matches!(&error, CodecError::RecordTooLarge { .. }) {
                    self.unusable = true;
                }
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, duplex};

    use super::super::MAX_RECORD_BYTES_FOR_TEST;

    #[tokio::test]
    async fn queues_events_while_waiting_for_a_response() {
        let (client_reader, mut server_writer) = duplex(1024);
        let (mut server_reader, client_writer) = duplex(1024);
        let mut client = RpcClient::new(BufReader::new(client_reader), client_writer);

        let server = tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                server_reader.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let request: Value = serde_json::from_slice(&request).unwrap();
            server_writer
                .write_all(b"{\"type\":\"agent_start\"}\n")
                .await
                .unwrap();
            server_writer
                .write_all(
                    format!(
                        "{{\"id\":{},\"type\":\"response\",\"command\":\"get_state\",\"success\":true}}\n",
                        serde_json::to_string(&request["id"]).unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let response = client
            .request(json!({
                "type": "get_state"
            }))
            .await
            .unwrap();
        assert_eq!(response.command, "get_state");
        assert_eq!(
            client.next_event().await.unwrap(),
            json!({
                "type": "agent_start"
            })
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn preserves_an_unmatched_response_without_optional_fields() {
        let (client_reader, mut server_writer) = duplex(1024);
        let (mut server_reader, client_writer) = duplex(1024);
        let mut client = RpcClient::new(BufReader::new(client_reader), client_writer);

        let server = tokio::spawn(async move {
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                server_reader.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if byte[0] == b'\n' {
                    break;
                }
            }
            let request: Value = serde_json::from_slice(&request).unwrap();
            server_writer
                .write_all(
                    b"{\"id\":\"peer-unmatched\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true}\n",
                )
                .await
                .unwrap();
            server_writer
                .write_all(
                    format!(
                        "{{\"id\":{},\"type\":\"response\",\"command\":\"get_state\",\"success\":true}}\n",
                        serde_json::to_string(&request["id"]).unwrap()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        client
            .request(json!({ "type": "get_state" }))
            .await
            .unwrap();
        assert_eq!(
            client.next_event().await.unwrap(),
            json!({
                "id": "peer-unmatched",
                "type": "response",
                "command": "get_state",
                "success": true
            })
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_a_command_with_an_id() {
        let mut client = RpcClient::new(BufReader::new(tokio::io::empty()), tokio::io::sink());

        let result = client
            .request(json!({
                "id": "command-supplied",
                "type": "get_state"
            }))
            .await;

        assert_matches!(result, Err(RpcError::ReservedCommandId));
        assert_eq!(client.next_id, 1);
    }

    #[tokio::test]
    async fn becomes_unusable_when_an_oversized_record_leaves_a_remainder() {
        let mut input = vec![b' '; MAX_RECORD_BYTES_FOR_TEST as usize + 1];
        input.extend_from_slice(b"{\"type\":\"agent_start\"}\n");
        let mut client = RpcClient::new(BufReader::new(input.as_slice()), tokio::io::sink());

        let first = client.next_event().await;
        assert_matches!(
            first,
            Err(RpcError::Codec(CodecError::RecordTooLarge {
                max_bytes: MAX_RECORD_BYTES_FOR_TEST
            }))
        );

        let second = client.next_event().await;
        assert_matches!(second, Err(RpcError::UnusableAfterOversizedRecord));
    }
}
