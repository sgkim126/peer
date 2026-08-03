use std::fmt;
use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug)]
pub enum CodecError {
    Io(io::Error),
    Eof,
    Json(serde_json::Error),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Pi RPC I/O failed: {error}"),
            Self::Eof => f.write_str("Pi RPC stream ended unexpectedly"),
            Self::Json(error) => write!(f, "Pi RPC record is invalid JSON: {error}"),
        }
    }
}

impl std::error::Error for CodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Eof => None,
            Self::Json(error) => Some(error),
        }
    }
}

impl From<io::Error> for CodecError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for CodecError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub async fn read_record<R, T>(stdout: &mut R) -> Result<T, CodecError>
where
    R: AsyncBufRead + Unpin,
    T: DeserializeOwned,
{
    let mut record = Vec::new();
    if stdout.read_until(b'\n', &mut record).await? == 0 {
        return Err(CodecError::Eof);
    }
    if record.last() != Some(&b'\n') {
        return Err(CodecError::Eof);
    }
    record.pop();
    if record.last() == Some(&b'\r') {
        record.pop();
    }
    Ok(serde_json::from_slice(&record)?)
}

pub async fn write_record<W, T>(stdin: &mut W, value: &T) -> Result<(), CodecError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut record = serde_json::to_vec(value)?;
    record.push(b'\n');
    stdin.write_all(&record).await?;
    stdin.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use serde_json::json;
    use tokio::io::{BufReader, duplex};

    #[tokio::test]
    async fn reads_lf_and_optional_carriage_return_only() {
        let (mut input, output) = duplex(256);
        input
            .write_all(b"{\"value\":\"line\\u2028separator\"}\r\n")
            .await
            .unwrap();
        drop(input);

        let value: serde_json::Value = read_record(&mut BufReader::new(output)).await.unwrap();
        assert_eq!(
            value,
            json!({
                "value": "line\u{2028}separator"
            })
        );
    }

    #[tokio::test]
    async fn reports_eof_before_a_record() {
        let (input, output) = duplex(16);
        drop(input);

        let result = read_record::<_, serde_json::Value>(&mut BufReader::new(output)).await;
        assert_matches!(result, Err(CodecError::Eof));
    }

    #[tokio::test]
    async fn reports_eof_before_record_delimiter() {
        let (mut input, output) = duplex(16);
        input.write_all(b"{\"valid\":true}").await.unwrap();
        drop(input);

        let result = read_record::<_, serde_json::Value>(&mut BufReader::new(output)).await;
        assert_matches!(result, Err(CodecError::Eof));
    }
}
