use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

use crate::gh::{GhClient, GhInvocation, GhOutput, GhRunError};

use super::CommandError;

const API_URL: &str = "https://api.github.com/";
const API_VERSION: &str = "2026-03-10";

struct Response<'a> {
    status: u16,
    headers: Vec<(&'a str, &'a str)>,
    body: &'a [u8],
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
pub async fn get<T: DeserializeOwned>(client: &GhClient, path: &str) -> Result<T, CommandError> {
    request(client, &endpoint(path)?, "GET", None).await
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn post<T: DeserializeOwned>(
    client: &GhClient,
    path: &str,
    body: &impl Serialize,
) -> Result<T, CommandError> {
    let body = serde_json::to_vec(body).map_err(CommandError::Serialize)?;
    request(client, &endpoint(path)?, "POST", Some(body)).await
}

fn endpoint(path: &str) -> Result<Url, CommandError> {
    let base = Url::parse(API_URL).expect("valid GitHub API URL");
    let url = base
        .join(path)
        .map_err(|_| CommandError::InvalidPagination)?;
    if url.origin() != base.origin() {
        return Err(CommandError::InvalidPagination);
    }
    if !url.username().is_empty() {
        return Err(CommandError::InvalidPagination);
    }
    if url.password().is_some() {
        return Err(CommandError::InvalidPagination);
    }
    Ok(url)
}

async fn request<T: DeserializeOwned>(
    client: &GhClient,
    url: &Url,
    method: &str,
    body: Option<Vec<u8>>,
) -> Result<T, CommandError> {
    // Validate the endpoint before gh sees its URL, including its credentials.
    endpoint(url.as_str())?;
    let path = url.path().to_owned();
    let mut args = vec![
        "api".into(),
        url.as_str().into(),
        "--hostname".into(),
        "github.com".into(),
        "--method".into(),
        method.into(),
        "--include".into(),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
        "--header".into(),
        format!("X-GitHub-Api-Version: {API_VERSION}"),
    ];
    if body.is_some() {
        args.extend([
            "--header".into(),
            "Content-Type: application/json".into(),
            "--input".into(),
            "-".into(),
        ]);
    }
    let output = client.run(&GhInvocation { args, stdin: body }).await?;
    let response = match parse_response(&output.stdout) {
        Some(response) => response,
        None if !output.status.success() => return Err(nonzero(output)),
        None => {
            return Err(CommandError::InvalidResponse {
                command: "gh api".into(),
            });
        }
    };
    if !(200..300).contains(&response.status) {
        return Err(api_error(&path, &response));
    }
    if !output.status.success() {
        return Err(nonzero(output));
    }
    serde_json::from_slice(response.body).map_err(|source| CommandError::Decode {
        endpoint: path,
        source,
    })
}

#[cfg_attr(not(test), expect(dead_code))]
pub fn nonzero(output: GhOutput) -> CommandError {
    CommandError::Run(GhRunError::NonZeroExit {
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
        if name.is_empty() {
            return None;
        }
        if name.bytes().any(|byte| byte.is_ascii_whitespace()) {
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
    #[derive(Deserialize)]
    struct ErrorBody {
        message: String,
    }

    let status = response.status;
    let mut rate_limited = status == 429
        || (status == 403
            && (response.header("x-ratelimit-remaining") == Some("0")
                || response.header("retry-after").is_some()));
    let mut permission_denied = false;
    if status == 403 && !rate_limited {
        let message = serde_json::from_slice::<ErrorBody>(response.body)
            .ok()
            .map(|body| body.message.trim().to_ascii_lowercase());
        rate_limited = message.as_deref().is_some_and(|message| {
            message.contains("secondary rate limit")
                || message.starts_with("api rate limit exceeded")
        });
        let sso_required = response.header("x-github-sso").is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("required"))
        });
        permission_denied = !rate_limited
            && (sso_required
                || matches!(
                    message.as_deref(),
                    Some(
                        "resource not accessible by integration"
                            | "resource not accessible by personal access token"
                    )
                ));
    }
    CommandError::Api {
        endpoint: endpoint.into(),
        status,
        rate_limited,
        permission_denied,
    }
}
