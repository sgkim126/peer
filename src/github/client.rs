use std::collections::HashSet;
use std::num::{NonZeroU32, NonZeroU64};
use std::time::{Duration, Instant};

use log::{debug, trace};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, de::DeserializeOwned};

use crate::context::ReviewContext;
use crate::git::CommitHash;

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

    pub async fn review_input(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<GitHubReviewInput, GitHubError> {
        debug!("loading GitHub review input: repository={repository} pull_request={number}");
        let prefix = format!("repos/{repository}");
        let pull_url = self
            .base
            .join(&format!("{prefix}/pulls/{number}"))
            .expect("valid PR path");
        let (pull, _) = self.get::<PullRequest>(pull_url.clone()).await?;
        let comments = self
            .list::<IssueComment>(&format!("{prefix}/issues/{number}/comments"))
            .await?;
        let reviews = self
            .list::<PullRequestReview>(&format!("{prefix}/pulls/{number}/reviews"))
            .await?;
        let review_comments = self
            .list::<ReviewComment>(&format!("{prefix}/pulls/{number}/comments"))
            .await?;
        let commits = self.pull_request_commits(repository, number, &pull).await?;
        let mut commit_comments = Vec::new();
        for commit in &commits {
            commit_comments.extend(
                self.list::<CommitComment>(&format!("{prefix}/commits/{commit}/comments"))
                    .await?,
            );
        }
        // A base change can alter commit membership without changing the head or count.
        let (current_pull, _) = self.get::<PullRequest>(pull_url).await?;
        if current_pull.base.sha != pull.base.sha
            || current_pull.head.sha != pull.head.sha
            || current_pull.commits != pull.commits
        {
            return Err(GitHubError::IncompleteCommits);
        }
        let context =
            mapping::review_context(pull, comments, reviews, review_comments, commit_comments);
        debug!(
            "loaded GitHub review input: repository={repository} pull_request={number} commits={} threads={}",
            commits.len(),
            context.comments.len()
        );
        Ok(GitHubReviewInput { context, commits })
    }

    pub async fn pull_request_commits(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        pull: &PullRequest,
    ) -> Result<Vec<CommitHash>, GitHubError> {
        let commits: Vec<_> = self
            .list::<CommitRef>(&format!("repos/{repository}/pulls/{number}/commits"))
            .await?
            .into_iter()
            .map(|commit| commit.sha)
            .collect();
        // Reject truncated lists and commits inconsistent with the initial PR.
        if commits.len() != pull.commits || commits.last() != Some(&pull.head.sha) {
            return Err(GitHubError::IncompleteCommits);
        }
        // Reject repeated pages.
        if commits
            .iter()
            .map(AsRef::as_ref)
            .collect::<HashSet<&str>>()
            .len()
            != commits.len()
        {
            return Err(GitHubError::IncompleteCommits);
        }
        Ok(commits)
    }

    pub async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, GitHubError> {
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
        self.request(url, Method::GET, None).await
    }

    pub async fn pull_request(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<PullRequest, GitHubError> {
        let url = self
            .base
            .join(&format!("repos/{repository}/pulls/{number}"))
            .expect("valid PR path");
        self.get(url).await.map(|(pull, _)| pull)
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GitHubError> {
        let url = self.base.join(path).expect("valid GitHub API path");
        self.request(url, Method::POST, Some(body))
            .await
            .map(|(value, _)| value)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        url: Url,
        method: Method,
        body: Option<&serde_json::Value>,
    ) -> Result<(T, HeaderMap), GitHubError> {
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
        trace!("sending GitHub request: method={method} endpoint={endpoint}");
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|source| {
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
    pub base: CommitRef,
    pub head: CommitRef,
    pub commits: usize,
}

#[derive(Debug, Deserialize)]
pub struct CommitRef {
    pub sha: CommitHash,
}

#[derive(Debug)]
pub struct GitHubReviewInput {
    pub context: ReviewContext,
    pub commits: Vec<CommitHash>,
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

#[derive(Debug, Deserialize)]
pub struct PullRequestReview {
    pub id: u64,
    pub submitted_at: Option<String>,
    pub state: String,
    pub user: Option<User>,
    pub body: Option<String>,
    pub commit_id: Option<CommitHash>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub in_reply_to_id: Option<u64>,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
    pub path: String,
    pub commit_id: CommitHash,
    pub original_commit_id: Option<CommitHash>,
    pub line: Option<NonZeroU32>,
    pub original_line: Option<NonZeroU32>,
    pub side: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitComment {
    pub id: u64,
    pub created_at: String,
    pub user: Option<User>,
    pub body: String,
    pub path: Option<String>,
    pub commit_id: CommitHash,
}

#[cfg(test)]
mod tests;
