use std::collections::HashSet;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use log::{debug, trace};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK};
use reqwest::{Client, Url};
use serde::{Deserialize, de::DeserializeOwned};

use crate::context::ReviewContext;

use super::{GitHubError, Repository, mapping};

const API_URL: &str = "https://api.github.com/";
const API_VERSION: &str = "2026-03-10";

pub struct GitHubClient {
    http: Client,
    base: Url,
}

impl GitHubClient {
    pub fn from_env() -> Result<Self, GitHubError> {
        let token = std::env::var("GITHUB_TOKEN")?;
        Self::new(
            &token,
            Url::parse(API_URL).expect("valid GitHub API URL"),
            Duration::from_secs(30),
        )
    }

    fn new(token: &str, base: Url, timeout: Duration) -> Result<Self, GitHubError> {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization(token)?);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            HeaderValue::from_static(API_VERSION),
        );
        let http = Client::builder()
            .user_agent(concat!("peer/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        trace!("GitHub client configured: api_version={API_VERSION} timeout={timeout:?}");
        Ok(Self { http, base })
    }

    pub async fn review_context(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<ReviewContext, GitHubError> {
        debug!("loading GitHub review context: repository={repository} pull_request={number}");
        let prefix = format!("repos/{repository}");
        let pull_url = self
            .base
            .join(&format!("{prefix}/pulls/{number}"))
            .expect("valid PR path");
        let (pull, _) = self.get::<PullRequest>(pull_url).await?;
        let comments = self
            .list::<IssueComment>(&format!("{prefix}/issues/{number}/comments"))
            .await?;
        let context = mapping::review_context(pull, comments);
        debug!(
            "loaded GitHub review context: repository={repository} pull_request={number} threads={}",
            context.comments.len()
        );
        Ok(context)
    }

    async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, GitHubError> {
        let started = Instant::now();
        debug!("loading GitHub collection: endpoint={path}");
        let mut url = self.base.join(path).expect("valid GitHub API path");
        url.set_query(Some("per_page=100"));
        let mut next = Some(url);
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        while let Some(url) = next {
            if !seen.insert(url.clone()) {
                debug!(
                    "GitHub pagination cycle detected: endpoint={path} pages={} loaded_items={}",
                    seen.len(),
                    items.len()
                );
                return Err(GitHubError::InvalidPagination);
            }
            trace!(
                "loading GitHub collection page: endpoint={path} page={}",
                seen.len()
            );
            let (page, headers) = self.get::<Vec<T>>(url).await.inspect_err(|_| {
                debug!(
                    "GitHub collection request failed: endpoint={path} page={} loaded_items={}",
                    seen.len(),
                    items.len()
                );
            })?;
            let page_items = page.len();
            items.extend(page);
            next = next_page(&headers).inspect_err(|_| {
                debug!(
                    "GitHub pagination failed: endpoint={path} page={} loaded_items={}",
                    seen.len(),
                    items.len()
                );
            })?;
            trace!(
                "loaded GitHub collection page: endpoint={path} page={} items={page_items} total_items={} has_next={}",
                seen.len(),
                items.len(),
                next.is_some()
            );
        }
        debug!(
            "loaded GitHub collection: endpoint={path} pages={} items={} duration_ms={}",
            seen.len(),
            items.len(),
            started.elapsed().as_millis()
        );
        Ok(items)
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<(T, HeaderMap), GitHubError> {
        // Pagination must never forward the token to another host or protocol.
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            debug!(
                "GitHub pagination URL rejected: origin_mismatch={} has_username={} has_password={}",
                url.origin() != self.base.origin(),
                !url.username().is_empty(),
                url.password().is_some()
            );
            return Err(GitHubError::InvalidPagination);
        }
        let started = Instant::now();
        let endpoint = url.path().to_string();
        trace!("sending GitHub request: method=GET endpoint={endpoint}");
        let response = self.http.get(url).send().await.map_err(|source| {
            debug!(
                "GitHub request failed: endpoint={endpoint} timeout={} connect={} duration_ms={}",
                source.is_timeout(),
                source.is_connect(),
                started.elapsed().as_millis()
            );
            GitHubError::Request {
                endpoint: endpoint.clone(),
                source,
            }
        })?;
        let status = response.status();
        trace!("received GitHub response: endpoint={endpoint} status={status}");
        if !status.is_success() {
            let rate_limited = status.as_u16() == 429
                || (status.as_u16() == 403
                    && (response
                        .headers()
                        .get("x-ratelimit-remaining")
                        .is_some_and(|value| value == "0")
                        || response.headers().contains_key("retry-after")));
            let error = GitHubError::Api {
                endpoint,
                status: status.as_u16(),
                rate_limited,
            };
            debug!("{error}; duration_ms={}", started.elapsed().as_millis());
            return Err(error);
        }
        let headers = response.headers().clone();
        let value = response.json().await.map_err(|source| {
            let error = if source.is_decode() {
                GitHubError::Decode {
                    endpoint: endpoint.clone(),
                    source,
                }
            } else {
                GitHubError::Request {
                    endpoint: endpoint.clone(),
                    source,
                }
            };
            debug!("{error}; duration_ms={}", started.elapsed().as_millis());
            error
        })?;
        trace!(
            "GitHub request completed: endpoint={endpoint} status={status} duration_ms={}",
            started.elapsed().as_millis()
        );
        Ok((value, headers))
    }
}

fn authorization(token: &str) -> Result<HeaderValue, GitHubError> {
    if token.trim().is_empty() {
        return Err(GitHubError::MissingToken);
    }
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(GitHubError::InvalidToken);
    }
    let mut value =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| GitHubError::InvalidToken)?;
    value.set_sensitive(true);
    Ok(value)
}

fn next_page(headers: &HeaderMap) -> Result<Option<Url>, GitHubError> {
    let mut next = None;
    for header in headers.get_all(LINK) {
        for link in header
            .to_str()
            .map_err(|_| {
                debug!("GitHub pagination Link header contains invalid text");
                GitHubError::InvalidPagination
            })?
            .split(',')
        {
            let mut parts = link.split(';');
            let target = parts.next().unwrap_or_default().trim();
            let is_next = parts.any(|part| {
                part.trim().split_once('=').is_some_and(|(key, value)| {
                    key.trim() == "rel"
                        && value
                            .trim()
                            .trim_matches('"')
                            .split_ascii_whitespace()
                            .any(|rel| rel == "next")
                })
            });
            if is_next {
                let url = target
                    .strip_prefix('<')
                    .and_then(|value| value.strip_suffix('>'))
                    .and_then(|value| Url::parse(value).ok())
                    .ok_or_else(|| {
                        debug!("GitHub pagination next link has an invalid URL");
                        GitHubError::InvalidPagination
                    })?;
                if next.replace(url).is_some() {
                    debug!("GitHub pagination contains multiple next links");
                    return Err(GitHubError::InvalidPagination);
                }
            }
        }
    }
    Ok(next)
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub title: String,
    pub body: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
}

#[cfg(test)]
mod tests;
