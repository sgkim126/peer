use serde::{Serialize, de::DeserializeOwned};
use url::Url;

use crate::glab::{GlabClient, GlabInvocation, GlabOutput, GlabRunError};

use super::CommandError;

const API_URL: &str = "https://gitlab.com/api/v4/";

#[derive(Debug)]
pub struct Response<'a> {
    pub status: u16,
    pub headers: Vec<(&'a str, &'a str)>,
    pub body: &'a [u8],
}

impl Response<'_> {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn get<T: DeserializeOwned>(client: &GlabClient, path: &str) -> Result<T, CommandError> {
    request(client, &endpoint(path)?, "GET", None, false)
        .await
        .map(|(value, _)| value)
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn post<T: DeserializeOwned>(
    client: &GlabClient,
    path: &str,
    body: &impl Serialize,
) -> Result<T, CommandError> {
    let body = serde_json::to_vec(body).map_err(CommandError::Serialize)?;
    request(client, &endpoint(path)?, "POST", Some(body), false)
        .await
        .map(|(value, _)| value)
}

fn endpoint(path: &str) -> Result<Url, CommandError> {
    let base = Url::parse(API_URL).expect("valid GitLab API URL");
    let url = base
        .join(path)
        .map_err(|_| CommandError::InvalidPagination)?;
    if url.origin() != base.origin() {
        return Err(CommandError::InvalidPagination);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CommandError::InvalidPagination);
    }
    if !url.path().starts_with(base.path()) {
        return Err(CommandError::InvalidPagination);
    }
    if url.fragment().is_some() {
        return Err(CommandError::InvalidPagination);
    }
    Ok(url)
}

async fn request<T: DeserializeOwned>(
    client: &GlabClient,
    url: &Url,
    method: &str,
    body: Option<Vec<u8>>,
    _paginated: bool,
) -> Result<(T, Option<Url>), CommandError> {
    // Validate every page before glab sees its URL, including its credentials.
    endpoint(url.as_str())?;
    let path = url.path().to_owned();
    let mut args = vec![
        "api".into(),
        url.as_str().into(),
        "--hostname".into(),
        "gitlab.com".into(),
        "--method".into(),
        method.into(),
        "--include".into(),
        "--header".into(),
        "Accept: application/json".into(),
    ];
    if body.is_some() {
        args.extend([
            "--header".into(),
            "Content-Type: application/json".into(),
            "--input".into(),
            "-".into(),
        ]);
    }
    let output = client.run(&GlabInvocation { args, stdin: body }).await?;
    let response = match parse_response(&output.stdout) {
        Some(response) => response,
        None if !output.status.success() => return Err(nonzero(output)),
        None => {
            return Err(CommandError::InvalidResponse {
                command: "glab api".into(),
            });
        }
    };
    if !(200..300).contains(&response.status) {
        return Err(api_error(&path, &response));
    }
    if !output.status.success() {
        return Err(nonzero(output));
    }
    let value = serde_json::from_slice(response.body).map_err(|source| CommandError::Decode {
        endpoint: path,
        source,
    })?;
    let next = None;
    Ok((value, next))
}

pub fn nonzero(output: GlabOutput) -> CommandError {
    CommandError::Run(GlabRunError::NonZeroExit {
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn parse_response(bytes: &[u8]) -> Option<Response<'_>> {
    let mut offset = 0;
    let mut status_line = response_line(bytes, &mut offset)?.split_ascii_whitespace();
    let protocol = status_line.next()?;
    if !matches!(
        protocol,
        "HTTP/1.0" | "HTTP/1.1" | "HTTP/2" | "HTTP/2.0" | "HTTP/3" | "HTTP/3.0"
    ) {
        return None;
    }
    let status = status_line.next()?.parse::<u16>().ok()?;
    if !(100..600).contains(&status) {
        return None;
    }
    let mut headers = Vec::new();
    loop {
        let line = response_line(bytes, &mut offset)?;
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':')?;
        if name.is_empty() || name.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return None;
        }
        headers.push((name, value.trim()));
    }
    Some(Response {
        status,
        headers,
        body: &bytes[offset..],
    })
}

fn response_line<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a str> {
    let end = *offset
        + bytes
            .get(*offset..)?
            .iter()
            .position(|byte| *byte == b'\n')?;
    let line = &bytes[*offset..end];
    *offset = end + 1;
    std::str::from_utf8(line.strip_suffix(b"\r").unwrap_or(line)).ok()
}

fn api_error(endpoint: &str, response: &Response<'_>) -> CommandError {
    let status = response.status;
    let rate_limited = status == 429
        || (status == 403
            && (response.header("ratelimit-remaining") == Some("0")
                || response.header("retry-after").is_some()));
    // Classify validation errors without retaining the body or stderr: either
    // can contain private note text echoed by GitLab or by glab itself.
    let (position_invalid, commit_invalid) = if matches!(status, 400 | 422) {
        serde_json::from_slice::<serde_json::Value>(response.body)
            .ok()
            .and_then(|body| {
                body.get("message")
                    .map(|message| (position_error(message), commit_error(message)))
            })
            .unwrap_or((false, false))
    } else {
        (false, false)
    };
    CommandError::Api {
        endpoint: endpoint.into(),
        status,
        rate_limited,
        position_invalid,
        commit_invalid,
    }
}

pub fn position_error(message: &serde_json::Value) -> bool {
    match message {
        serde_json::Value::Object(fields) => {
            !fields.is_empty()
                && fields.iter().all(|(field, detail)| {
                    (position_field(field) && has_validation_message(detail))
                        || (field == "commit_id" && diff_refs_message(detail))
                })
        }
        _ => false,
    }
}

pub fn commit_error(message: &serde_json::Value) -> bool {
    match message {
        serde_json::Value::Object(fields) => {
            !fields.is_empty()
                && fields
                    .iter()
                    .all(|(field, detail)| field == "commit_id" && has_validation_message(detail))
        }
        _ => false,
    }
}

fn position_field(field: &str) -> bool {
    matches!(field, "position" | "line_code")
        || field.starts_with("position.")
        || field.starts_with("position[")
}

fn has_validation_message(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(message) => !message.trim().is_empty(),
        serde_json::Value::Array(messages) => {
            !messages.is_empty() && messages.iter().all(has_validation_message)
        }
        serde_json::Value::Object(fields) => {
            !fields.is_empty() && fields.values().all(has_validation_message)
        }
        _ => false,
    }
}

fn diff_refs_message(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(message) => message.to_ascii_lowercase().contains("diff refs"),
        serde_json::Value::Array(messages) => {
            !messages.is_empty() && messages.iter().all(diff_refs_message)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;

    use super::*;

    #[test]
    fn accepts_glab_mixed_line_endings() {
        // glab prints its status line with LF and the header terminator with CRLF.
        let response =
            parse_response(b"HTTP/2.0 200 OK\nContent-Type: application/json\r\n\r\n[]").unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert_eq!(response.body, b"[]");
    }

    #[test]
    fn accepts_http_crlf_line_endings() {
        let response =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[]").unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert_eq!(response.body, b"[]");
    }

    #[test]
    fn preserves_http_status_when_glab_cannot_decode_an_error_body() {
        // glab emits --include headers before attempting to parse an error JSON
        // body. Its JSON error can leave stdout without a body.
        let response = parse_response(b"HTTP/2.0 403 Forbidden\n\r\n").unwrap();

        assert_matches!(
            api_error("/api/v4/projects/1", &response),
            CommandError::Api {
                status: 403,
                position_invalid: false,
                commit_invalid: false,
                ..
            }
        );
    }

    #[test]
    fn rejects_json_without_http_headers() {
        assert_matches!(parse_response(br#"{"id": 1}"#), None);
    }

    #[test]
    fn rejects_invalid_http_status() {
        assert_matches!(parse_response(b"HTTP/2.0 invalid\n\n{}"), None);
    }

    #[test]
    fn rejects_header_names_containing_whitespace() {
        assert_matches!(
            parse_response(b"HTTP/2.0 200 OK\nInvalid Header: value\n\n{}"),
            None
        );
    }

    #[test]
    fn rejects_unterminated_http_headers() {
        assert_matches!(
            parse_response(b"HTTP/2.0 200 OK\nContent-Type: application/json"),
            None
        );
    }
}
