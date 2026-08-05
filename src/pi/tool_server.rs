use std::fmt;
use std::fs;
use std::num::NonZeroU8;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::{JoinHandle, JoinSet};

use crate::console::Console;
use crate::extract::{ExtractError, Extractor, FileContent};

use super::rpc::{CodecError, read_record, write_record};

static SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct ToolServer {
    directory: PathBuf,
    socket_path: PathBuf,
    task: JoinHandle<()>,
}

impl ToolServer {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn start(project_root: &Path, console: Console) -> Result<Self, std::io::Error> {
        let (directory, socket_path, listener) = bind_listener()?;
        let extractor = Arc::new(Extractor::new(project_root.to_path_buf(), console));
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let extractor = Arc::clone(&extractor);
                            let console = console;
                            connections.spawn(async move {
                                if let Err(error) = handle_connection(stream, &extractor).await {
                                    console.debug(format_args!(
                                        "peer tool connection failed: {error}"
                                    ));
                                }
                            });
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::Interrupted
                                    | std::io::ErrorKind::ConnectionAborted
                            ) => {}
                        Err(_) => break,
                    },
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Ok(Self {
            directory,
            socket_path,
            task,
        })
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for ToolServer {
    fn drop(&mut self) {
        self.task.abort();
        let _ = fs::remove_file(&self.socket_path);
        let _ = fs::remove_dir(&self.directory);
    }
}

fn bind_listener() -> Result<(PathBuf, PathBuf, UnixListener), std::io::Error> {
    loop {
        let sequence = SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("peer-tools-{}-{sequence}", std::process::id()));
        match fs::DirBuilder::new().mode(0o700).create(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).inspect_err(|_| {
            let _ = fs::remove_dir(&directory);
        })?;
        let socket_path = directory.join("tools.sock");
        let listener = UnixListener::bind(&socket_path).inspect_err(|_| {
            let _ = fs::remove_dir(&directory);
        })?;
        return Ok((directory, socket_path, listener));
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRequest {
    id: String,
    tool: String,
    arguments: Value,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolResponse {
    id: String,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn handle_connection(stream: UnixStream, extractor: &Extractor) -> Result<(), CodecError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    // Decode into a generic value first so client input errors can return a
    // structured response with the original request ID when available.
    let request_value: Value = match read_record(&mut reader).await {
        Ok(value) => value,
        // Malformed or oversized records cannot provide a trustworthy request ID.
        Err(error @ (CodecError::Json(_) | CodecError::RecordTooLarge { .. })) => {
            return write_request_error(&mut writer, String::new(), error).await;
        }
        // I/O failures are transport errors, not invalid client input, so let the
        // connection handler report them instead of treating them as a response.
        Err(error @ CodecError::Io(_)) => return Err(error),
        // EOF means no complete request was received, so there is nothing to reply to.
        Err(error @ CodecError::Eof) => return Err(error),
    };
    let request_id = request_value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let request: ToolRequest = match serde_json::from_value(request_value) {
        Ok(request) => request,
        Err(error) => return write_request_error(&mut writer, request_id, error.into()).await,
    };
    let execution = execute_tool(extractor, &request.tool, request.arguments);
    tokio::pin!(execution);
    let result = tokio::select! {
        biased;
        read = reader.read_u8() => match read {
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => Err(ToolExecutionError::UnexpectedTrailingData),
        },
        result = &mut execution => result,
    };
    let response = match result {
        Ok(data) => ToolResponse {
            id: request.id,
            success: true,
            data: Some(data),
            error: None,
        },
        Err(error) => ToolResponse {
            id: request.id,
            success: false,
            data: None,
            error: Some(error.to_string()),
        },
    };
    write_record(&mut writer, &response).await
}

async fn write_request_error<W>(
    writer: &mut W,
    id: String,
    error: CodecError,
) -> Result<(), CodecError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    write_record(
        writer,
        &ToolResponse {
            id,
            success: false,
            data: None,
            error: Some(error.to_string()),
        },
    )
    .await
}

#[derive(Debug)]
enum ToolExecutionError {
    UnknownTool(String),
    InvalidArguments { tool: String, reason: String },
    UnexpectedTrailingData,
    Extract(ExtractError),
    Serialization(serde_json::Error),
}

impl fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTool(tool) => write!(f, "unknown peer read tool: {tool}"),
            Self::InvalidArguments { tool, reason } => {
                write!(f, "invalid arguments for {tool}: {reason}")
            }
            Self::UnexpectedTrailingData => {
                f.write_str("unexpected trailing data after the request record")
            }
            Self::Extract(error) => error.fmt(f),
            Self::Serialization(error) => write!(f, "cannot serialize tool result: {error}"),
        }
    }
}

impl std::error::Error for ToolExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnknownTool(_) => None,
            Self::InvalidArguments { .. } => None,
            Self::UnexpectedTrailingData => None,
            Self::Extract(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
}

impl From<ExtractError> for ToolExecutionError {
    fn from(error: ExtractError) -> Self {
        Self::Extract(error)
    }
}

impl From<serde_json::Error> for ToolExecutionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevisionArguments {
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeArguments {
    from_revision: String,
    to_revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileArguments {
    revision: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDiffArguments {
    from_revision: String,
    to_revision: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListTreeArguments {
    revision: String,
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepArguments {
    revision: String,
    query: String,
    path: Option<String>,
    context_lines: Option<NonZeroU8>,
}

#[cfg_attr(not(test), expect(dead_code))]
async fn execute_tool(
    extractor: &Extractor,
    tool: &str,
    arguments: Value,
) -> Result<Value, ToolExecutionError> {
    match tool {
        "get_commit_message" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_message(&arguments.revision).await?;
            Ok(Value::String(result.message))
        }
        "get_commit_diff" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_diff(&arguments.revision).await?;
            Ok(Value::String(result.diff))
        }
        "get_changed_files" => {
            let arguments: RevisionArguments = parse_arguments(tool, arguments)?;
            let result = extractor.commit_files(&arguments.revision).await?;
            Ok(serde_json::to_value(result.files)?)
        }
        "get_commits_in_range" => {
            let arguments: RangeArguments = parse_arguments(tool, arguments)?;
            let range = format!("{}..{}", arguments.from_revision, arguments.to_revision);
            let result = extractor.commit_list(&range).await?;
            Ok(serde_json::to_value(result.commits)?)
        }
        "get_file_content" => {
            let arguments: FileArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .file_content(&arguments.revision, Path::new(&arguments.path), None)
                .await?;
            match result {
                FileContent::Text { content, .. } => Ok(json!({
                    "type": "text",
                    "content": content
                })),
                FileContent::Binary { size, .. } => Ok(json!({
                    "type": "binary",
                    "size": size
                })),
            }
        }
        "get_file_diff" => {
            let arguments: FileDiffArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .file_diff(
                    &arguments.from_revision,
                    &arguments.to_revision,
                    Path::new(&arguments.path),
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        "list_tree" => {
            let arguments: ListTreeArguments = parse_arguments(tool, arguments)?;
            let result = extractor
                .list_tree(
                    &arguments.revision,
                    arguments.path.as_deref().map(Path::new),
                    arguments.recursive,
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        "grep" => {
            let arguments: GrepArguments = parse_arguments(tool, arguments)?;
            let context_lines = arguments
                .context_lines
                .unwrap_or(NonZeroU8::new(2).expect("2 is non-zero"));
            let result = extractor
                .grep(
                    &arguments.query,
                    &arguments.revision,
                    arguments.path.as_deref().map(Path::new),
                    context_lines,
                )
                .await?;
            Ok(serde_json::to_value(result)?)
        }
        _ => Err(ToolExecutionError::UnknownTool(tool.to_string())),
    }
}

fn parse_arguments<T>(tool: &str, arguments: Value) -> Result<T, ToolExecutionError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(arguments).map_err(|error| ToolExecutionError::InvalidArguments {
        tool: tool.to_string(),
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use tokio::io::AsyncWriteExt;
    use tokio::net::UnixStream;

    use crate::git::run_git;
    use crate::pi::rpc::MAX_RECORD_BYTES_FOR_TEST;

    async fn send_raw_request(request: Vec<u8>) -> ToolResponse {
        let repository = tempfile::tempdir().unwrap();
        let server = ToolServer::start(repository.path(), Console::default()).unwrap();
        let stream = UnixStream::connect(server.socket_path()).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let write = tokio::spawn(async move { writer.write_all(&request).await });

        let response = read_record(&mut BufReader::new(reader)).await.unwrap();
        let _ = write.await;
        response
    }

    #[tokio::test]
    async fn serves_repository_tools_over_a_unix_socket() {
        let repository = tempfile::tempdir().unwrap();
        let console = Console::default();
        run_git(&["init"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["config", "user.email", "test@example.com"],
            repository.path(),
            console,
        )
        .await
        .unwrap();
        run_git(&["config", "user.name", "Test"], repository.path(), console)
            .await
            .unwrap();
        fs::write(repository.path().join("file.txt"), "content\n").unwrap();
        run_git(&["add", "file.txt"], repository.path(), console)
            .await
            .unwrap();
        run_git(
            &["commit", "--no-gpg-sign", "-m", "socket test"],
            repository.path(),
            console,
        )
        .await
        .unwrap();

        let server = ToolServer::start(repository.path(), console).unwrap();
        let stream = UnixStream::connect(server.socket_path()).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        write_record(
            &mut writer,
            &json!({
                "id": "request-1",
                "tool": "get_commit_message",
                "arguments": {
                    "revision": "HEAD"
                }
            }),
        )
        .await
        .unwrap();
        let response: ToolResponse = read_record(&mut BufReader::new(reader)).await.unwrap();

        assert_eq!(response.id, "request-1");
        assert!(response.success);
        assert_eq!(
            response.data,
            Some(Value::String("socket test".to_string()))
        );
        assert_eq!(response.error, None);
    }

    #[tokio::test]
    async fn serves_overlapping_connections() {
        let repository = tempfile::tempdir().unwrap();
        let server = ToolServer::start(repository.path(), Console::default()).unwrap();
        let first_stream = UnixStream::connect(server.socket_path()).await.unwrap();
        let second_stream = UnixStream::connect(server.socket_path()).await.unwrap();
        let (first_reader, mut first_writer) = first_stream.into_split();
        let (second_reader, mut second_writer) = second_stream.into_split();

        write_record(
            &mut second_writer,
            &json!({
                "id": "request-2",
                "tool": "unknown",
                "arguments": {}
            }),
        )
        .await
        .unwrap();
        let second_response: ToolResponse = read_record(&mut BufReader::new(second_reader))
            .await
            .unwrap();

        assert_eq!(second_response.id, "request-2");
        assert!(!second_response.success);

        tokio::task::yield_now().await;
        write_record(
            &mut first_writer,
            &json!({
                "id": "request-1",
                "tool": "unknown",
                "arguments": {}
            }),
        )
        .await
        .unwrap();
        let first_response: ToolResponse = read_record(&mut BufReader::new(first_reader))
            .await
            .unwrap();

        assert_eq!(first_response.id, "request-1");
        assert!(!first_response.success);
    }

    #[tokio::test]
    async fn removes_socket_and_directory_when_dropped() {
        let repository = tempfile::tempdir().unwrap();
        let server = ToolServer::start(repository.path(), Console::default()).unwrap();
        let socket_path = server.socket_path().to_path_buf();
        let directory = socket_path.parent().unwrap().to_path_buf();

        assert!(socket_path.exists());
        assert!(directory.exists());

        drop(server);

        assert!(!socket_path.exists());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn reports_unknown_tools() {
        let extractor = Extractor::new(PathBuf::from("/unused"), Console::default());
        let error = execute_tool(&extractor, "unknown", json!({}))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "unknown peer read tool: unknown");
    }

    #[tokio::test]
    async fn reports_malformed_requests_without_an_id() {
        let response = send_raw_request(b"{invalid}\n".to_vec()).await;

        assert_eq!(response.id, "");
        assert!(!response.success);
        assert_eq!(response.data, None);
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .starts_with("Pi RPC record is invalid JSON:")
        );
    }

    #[tokio::test]
    async fn handles_client_input_errors_after_writing_the_response() {
        let (server_stream, client_stream) = UnixStream::pair().unwrap();
        let (reader, mut writer) = client_stream.into_split();
        let server = tokio::spawn(async move {
            let extractor = Extractor::new(PathBuf::from("/unused"), Console::default());
            handle_connection(server_stream, &extractor).await
        });
        writer.write_all(b"{invalid}\n").await.unwrap();

        let response: ToolResponse = read_record(&mut BufReader::new(reader)).await.unwrap();
        let result = server.await.unwrap();

        assert!(!response.success);
        assert_matches!(result, Ok(()));
    }

    #[tokio::test]
    async fn propagates_eof_before_a_request() {
        let (server_stream, mut client_stream) = UnixStream::pair().unwrap();
        let server = tokio::spawn(async move {
            let extractor = Extractor::new(PathBuf::from("/unused"), Console::default());
            handle_connection(server_stream, &extractor).await
        });
        client_stream.shutdown().await.unwrap();

        let result = server.await.unwrap();

        assert_matches!(result, Err(CodecError::Eof));
    }

    #[tokio::test]
    async fn preserves_the_id_when_request_fields_are_invalid() {
        let response = send_raw_request(
            br#"{"id":"request-1","tool":"get_commit_message","arguments":{},"extra":true}
"#
            .to_vec(),
        )
        .await;

        assert_eq!(response.id, "request-1");
        assert!(!response.success);
        assert_eq!(response.data, None);
        assert!(
            response
                .error
                .as_deref()
                .unwrap()
                .contains("unknown field `extra`")
        );
    }

    #[tokio::test]
    async fn reports_oversized_requests_without_an_id() {
        let mut request = vec![b' '; MAX_RECORD_BYTES_FOR_TEST as usize];
        request.push(b' ');
        let response = send_raw_request(request).await;

        assert_eq!(response.id, "");
        assert!(!response.success);
        assert_eq!(response.data, None);
        assert_eq!(
            response.error.as_deref(),
            Some("Pi RPC record exceeds the 4194304-byte limit")
        );
    }

    #[tokio::test]
    async fn rejects_trailing_data_after_a_request() {
        let repository = tempfile::tempdir().unwrap();
        let server = ToolServer::start(repository.path(), Console::default()).unwrap();
        let stream = UnixStream::connect(server.socket_path()).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut request = serde_json::to_vec(&json!({
            "id": "request-1",
            "tool": "get_commit_message",
            "arguments": {
                "revision": "HEAD"
            }
        }))
        .unwrap();
        request.extend_from_slice(b"\ntrailing");
        writer.write_all(&request).await.unwrap();

        let response: ToolResponse = read_record(&mut BufReader::new(reader)).await.unwrap();

        assert_eq!(response.id, "request-1");
        assert!(!response.success);
        assert_eq!(response.data, None);
        assert_eq!(
            response.error.as_deref(),
            Some("unexpected trailing data after the request record")
        );
    }
}
