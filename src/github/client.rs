use std::collections::HashSet;
use std::num::{NonZeroU32, NonZeroU64};
use std::time::{Duration, Instant};

use log::{debug, trace};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, LINK};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, de::DeserializeOwned};

use crate::context::ReviewContext;
use crate::git::CommitHash;

use super::position::{ChangedFile, CommitCommentPosition, commit_comment_position};
use super::{GitHubError, Repository, mapping};

const API_URL: &str = "https://api.github.com/";
const API_VERSION: &str = "2026-03-10";

pub struct GitHubClient {
    http: Client,
    base: Url,
}

struct Collection<T> {
    items: Vec<T>,
    repository_accessible: bool,
}

#[derive(Deserialize)]
struct ApiErrorResponse {
    message: String,
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
        let source_repository = pull
            .head
            .repo
            .as_ref()
            .map(|repo| Repository::parse(&repo.full_name))
            .transpose()?;
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
        let source = source_repository.as_ref().filter(|source| {
            !source
                .to_string()
                .eq_ignore_ascii_case(&repository.to_string())
        });
        let mut source_repository_accessible = true;
        let mut first_source_request = true;
        let mut commit_comments = Vec::new();
        for commit in &commits {
            commit_comments.extend(self.commit_comments(repository, commit, false).await?.items);
            if let Some(source) = source
                && source_repository_accessible
            {
                // A token scoped to the target may not be able to read the fork.
                // Only the first request can skip an inaccessible fork; later failures
                // must abort the review so partially loaded comments are never used.
                let comments = self
                    .commit_comments(source, commit, first_source_request)
                    .await?;
                first_source_request = false;
                source_repository_accessible = comments.repository_accessible;
                commit_comments.extend(comments.items);
            }
        }
        // A base change can alter commit membership without changing the head or count.
        let (current_pull, _) = self.get::<PullRequest>(pull_url).await?;
        if current_pull.base.sha != pull.base.sha
            || current_pull.head.sha != pull.head.sha
            || current_pull.head.repo != pull.head.repo
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

    async fn commit_comments(
        &self,
        repository: &Repository,
        commit: &CommitHash,
        skip_inaccessible_repository: bool,
    ) -> Result<Collection<CommitComment>, GitHubError> {
        let mut comments = self
            .list_with_access::<CommitComment>(
                &format!("repos/{repository}/commits/{commit}/comments"),
                skip_inaccessible_repository,
            )
            .await?;
        if !comments.items.iter().any(|comment| {
            comment.commit_id == *commit
                && comment.path.as_ref().is_some_and(|path| !path.is_empty())
                && comment.position.is_some_and(|position| position > 0)
        }) {
            return Ok(comments);
        }
        // Coordinates are optional context. A missing diff must not discard comments,
        // and no coordinates are resolved until every file page has loaded.
        match self.commit_files(repository, commit).await {
            Ok(files) => {
                for comment in &mut comments.items {
                    if comment.commit_id == *commit
                        && let Some(path) = &comment.path
                        && let Some(position) = comment.position
                    {
                        comment.resolved_position = commit_comment_position(&files, path, position);
                    }
                }
            }
            Err(error) => debug!(
                "GitHub commit comment coordinates unavailable: repository={repository} commit={commit} error={error}"
            ),
        }
        Ok(comments)
    }

    async fn commit_files(
        &self,
        repository: &Repository,
        commit: &CommitHash,
    ) -> Result<Vec<ChangedFile>, GitHubError> {
        let mut url = self
            .base
            .join(&format!("repos/{repository}/commits/{commit}"))
            .expect("valid commit path");
        url.set_query(Some("per_page=100"));
        let mut next = Some(url);
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        while let Some(url) = next {
            if !seen.insert(url.clone()) {
                return Err(GitHubError::InvalidPagination);
            }
            let (page, headers) = self.get::<CommitFiles>(url).await?;
            files.extend(page.files);
            next = next_page(&headers)?;
        }
        Ok(files)
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
        Ok(self.list_with_access(path, false).await?.items)
    }

    async fn list_with_access<T: DeserializeOwned>(
        &self,
        path: &str,
        skip_inaccessible_repository: bool,
    ) -> Result<Collection<T>, GitHubError> {
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
            let (page, headers) = match self.get::<Vec<T>>(url).await.inspect_err(|_| {
                debug!(
                    "GitHub collection request failed: endpoint={path} page={} loaded_items={}",
                    seen.len(),
                    items.len()
                );
            }) {
                Ok(page) => page,
                Err(
                    error @ (GitHubError::Api { status: 404, .. }
                    | GitHubError::Api {
                        status: 403,
                        rate_limited: false,
                        permission_denied: true,
                        ..
                    }),
                ) if skip_inaccessible_repository && seen.len() == 1 => {
                    debug!("GitHub optional collection unavailable: endpoint={path} error={error}");
                    return Ok(Collection {
                        items: Vec::new(),
                        repository_accessible: false,
                    });
                }
                Err(error) => return Err(error),
            };
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
        Ok(Collection {
            items,
            repository_accessible: true,
        })
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
            let mut rate_limited = status.as_u16() == 429
                || (status.as_u16() == 403
                    && (response
                        .headers()
                        .get("x-ratelimit-remaining")
                        .is_some_and(|value| value == "0")
                        || response.headers().contains_key("retry-after")));
            let mut permission_denied = false;
            if status.as_u16() == 403 && !rate_limited {
                let sso_required = response
                    .headers()
                    .get("x-github-sso")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| {
                        value
                            .split(';')
                            .next()
                            .is_some_and(|value| value.trim().eq_ignore_ascii_case("required"))
                    });
                // Secondary limits may only be identified in the response body.
                // Preserve the known HTTP failure if that body cannot be read or decoded.
                let message = response
                    .json::<ApiErrorResponse>()
                    .await
                    .ok()
                    .map(|error| error.message.trim().to_ascii_lowercase());
                rate_limited = message.as_deref().is_some_and(|message| {
                    message.contains("secondary rate limit")
                        || message.starts_with("api rate limit exceeded")
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
            let error = GitHubError::Api {
                endpoint,
                status: status.as_u16(),
                rate_limited,
                permission_denied,
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
    pub repo: Option<RepositoryRef>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct RepositoryRef {
    pub full_name: String,
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
    pub position: Option<u32>,
    // The API's line can refer to a deleted line, so only trust patch-derived positions.
    #[serde(skip)]
    pub resolved_position: Option<CommitCommentPosition>,
}

#[derive(Deserialize)]
struct CommitFiles {
    files: Vec<ChangedFile>,
}

#[cfg(test)]
mod tests;
