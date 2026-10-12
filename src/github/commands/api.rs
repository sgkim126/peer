use std::collections::HashSet;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use url::Url;

use crate::gh::{GhClient, GhInvocation, GhOutput, GhRunError};

use super::CommandError;

const API_URL: &str = "https://api.github.com/";
const API_VERSION: &str = "2026-03-10";

#[derive(Debug)]
pub struct Collection<T> {
    pub items: Vec<T>,
    pub repository_accessible: bool,
}

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
    request(client, &endpoint(path)?, "GET", None, false)
        .await
        .map(|(value, _)| value)
}

pub async fn post<T: DeserializeOwned>(
    client: &GhClient,
    path: &str,
    body: &impl Serialize,
) -> Result<T, CommandError> {
    let body = serde_json::to_vec(body).map_err(CommandError::Serialize)?;
    request(client, &endpoint(path)?, "POST", Some(body), false)
        .await
        .map(|(value, _)| value)
}

pub async fn list<T: DeserializeOwned>(
    client: &GhClient,
    path: &str,
) -> Result<Vec<T>, CommandError> {
    Ok(pages(client, path, false, |page: Vec<T>| page).await?.items)
}

pub async fn pages<P: DeserializeOwned, T>(
    client: &GhClient,
    path: &str,
    skip_inaccessible: bool,
    items: impl Fn(P) -> Vec<T>,
) -> Result<Collection<T>, CommandError> {
    let mut url = endpoint(path)?;
    url.set_query(Some("per_page=100"));
    let mut next = Some(url);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some(url) = next {
        if !seen.insert(url.clone()) {
            return Err(CommandError::InvalidPagination);
        }
        match request(client, &url, "GET", None, true).await {
            Ok((page, following)) => {
                result.extend(items(page));
                next = following;
            }
            Err(
                CommandError::Api { status: 404, .. }
                | CommandError::Api {
                    status: 403,
                    rate_limited: false,
                    permission_denied: true,
                    ..
                },
            ) if skip_inaccessible && seen.len() == 1 => {
                return Ok(Collection {
                    items: Vec::new(),
                    repository_accessible: false,
                });
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Collection {
        items: result,
        repository_accessible: true,
    })
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
    paginated: bool,
) -> Result<(T, Option<Url>), CommandError> {
    // Validate every page before gh sees its URL, including its credentials.
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
    let value = serde_json::from_slice(response.body).map_err(|source| CommandError::Decode {
        endpoint: path,
        source,
    })?;
    let next = if paginated {
        next_page(&response)?
    } else {
        None
    };
    Ok((value, next))
}

#[cfg_attr(not(test), expect(dead_code))]
pub fn nonzero(output: GhOutput) -> CommandError {
    CommandError::Run(GhRunError::NonZeroExit {
        status: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub fn decode<T: DeserializeOwned>(output: GhOutput, command: &str) -> Result<T, CommandError> {
    if !output.status.success() {
        return Err(nonzero(output));
    }
    serde_json::from_slice(&output.stdout).map_err(|source| CommandError::Decode {
        endpoint: command.to_owned(),
        source,
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

fn next_page(response: &Response<'_>) -> Result<Option<Url>, CommandError> {
    let mut next = None;
    for (_, value) in response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("link"))
    {
        for link in value.split(',') {
            let mut parts = link.split(';');
            let target = parts.next().unwrap_or_default().trim();
            let is_next = parts.any(|part| {
                part.trim().split_once('=').is_some_and(|(key, value)| {
                    key.trim() == "rel"
                        && value
                            .trim()
                            .trim_matches('"')
                            .split_ascii_whitespace()
                            .any(|relation| relation == "next")
                })
            });
            if is_next {
                let url = target
                    .strip_prefix('<')
                    .and_then(|target| target.strip_suffix('>'))
                    .and_then(|target| Url::parse(target).ok())
                    .ok_or(CommandError::InvalidPagination)?;
                endpoint(url.as_str())?;
                if next.replace(url).is_some() {
                    return Err(CommandError::InvalidPagination);
                }
            }
        }
    }
    Ok(next)
}
