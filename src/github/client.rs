use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use log::{debug, trace};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
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
        let pull = self.get::<PullRequest>(pull_url).await?;
        let context = mapping::review_context(pull);
        debug!("loaded GitHub review context: repository={repository} pull_request={number}");
        Ok(context)
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T, GitHubError> {
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
        Ok(value)
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

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub title: String,
    pub body: Option<String>,
}

#[cfg(test)]
mod tests;
