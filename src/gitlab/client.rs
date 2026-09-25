use std::collections::HashSet;
use std::num::{NonZeroU32, NonZeroU64};
use std::time::Duration;

use log::{debug, trace};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, LINK};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};

use crate::context::ReviewContext;
use crate::git::CommitHash;

use super::{GitLabError, Repository, mapping};

const API_URL: &str = "https://gitlab.com/api/v4/";

pub struct GitLabClient {
    http: Client,
    base: Url,
}

struct Collection<T> {
    items: Vec<T>,
    project_accessible: bool,
}

impl GitLabClient {
    pub fn from_env() -> Result<Self, GitLabError> {
        Self::new(
            &std::env::var("GITLAB_TOKEN")?,
            Url::parse(API_URL).expect("valid GitLab API URL"),
            Duration::from_secs(30),
        )
    }

    pub fn new(token: &str, base: Url, timeout: Duration) -> Result<Self, GitLabError> {
        let mut headers = HeaderMap::new();
        headers.insert("private-token", private_token(token)?);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let http = Client::builder()
            .user_agent(concat!("peer/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        Ok(Self { http, base })
    }

    pub async fn review_input(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<GitLabReviewInput, GitLabError> {
        debug!("loading GitLab review input: repository={repository} merge_request={number}");
        let merge_request = self.merge_request(repository, number).await?;
        let source = merge_request.source(number)?;
        let prefix = format!("{}/merge_requests/{number}", repository.api_path());
        let commits: Vec<_> = self
            .list::<CommitRef>(&format!("{prefix}/commits"))
            .await?
            .into_iter()
            .map(|commit| commit.id)
            .collect();
        let unique: HashSet<_> = commits.iter().map(CommitHash::as_ref).collect();
        if commits.is_empty()
            || unique.len() != commits.len()
            || !commits.contains(&source.diff_refs.head_sha)
        {
            return Err(GitLabError::IncompleteCommits);
        }
        let mut discussions = self
            .list::<Discussion>(&format!("{prefix}/discussions"))
            .await?;
        let source_project_id = source
            .source_project_id
            .filter(|project_id| *project_id != source.project_id);
        let mut source_project_accessible = true;
        let mut first_source_request = true;
        for commit in &commits {
            discussions.extend(
                self.commit_discussions(source.project_id, commit, false)
                    .await?
                    .items,
            );
            if let Some(project_id) = source_project_id
                && source_project_accessible
            {
                // A token scoped to the target may not be able to read the fork.
                // Only the first request can skip an inaccessible fork; later failures
                // must abort the review so partially loaded comments are never used.
                let comments = self
                    .commit_discussions(project_id, commit, first_source_request)
                    .await?;
                first_source_request = false;
                source_project_accessible = comments.project_accessible;
                discussions.extend(comments.items);
            }
        }
        let current = self.merge_request(repository, number).await?;
        if current.source(number)? != source
            || current.source_branch != merge_request.source_branch
            || current.target_branch != merge_request.target_branch
        {
            return Err(GitLabError::MergeRequestChanged);
        }
        // GitLab's commit list order is not part of the API contract. The local
        // target resolver validates membership and orders this set using Git.
        let context = mapping::review_context(merge_request, discussions);
        debug!(
            "loaded GitLab review input: repository={repository} merge_request={number} commits={} threads={}",
            commits.len(),
            context.comments.len()
        );
        Ok(GitLabReviewInput {
            context,
            commits,
            source,
        })
    }

    pub async fn merge_request(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<MergeRequest, GitLabError> {
        self.get(&format!(
            "{}/merge_requests/{number}",
            repository.api_path()
        ))
        .await
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitLabError> {
        let url = self.base.join(path).expect("valid GitLab API path");
        self.request(url, Method::GET, None)
            .await
            .map(|(value, _)| value)
    }

    pub async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, GitLabError> {
        self.list_with_access(path, false)
            .await
            .map(|collection| collection.items)
    }

    async fn list_with_access<T: DeserializeOwned>(
        &self,
        path: &str,
        skip_inaccessible_project: bool,
    ) -> Result<Collection<T>, GitLabError> {
        let mut url = self.base.join(path).expect("valid GitLab API path");
        url.query_pairs_mut().append_pair("per_page", "100");
        let collection_path = url.path().to_string();
        let mut next = Some(url);
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        while let Some(url) = next {
            if url.path() != collection_path || !seen.insert(url.clone()) {
                return Err(GitLabError::InvalidPagination);
            }
            let (page, headers) = match self.request::<Vec<T>>(url, Method::GET, None).await {
                Ok(page) => page,
                Err(
                    error @ GitLabError::Api {
                        status: 403 | 404,
                        rate_limited: false,
                        ..
                    },
                ) if skip_inaccessible_project && seen.len() == 1 => {
                    debug!("GitLab optional collection unavailable: path={path} error={error}");
                    return Ok(Collection {
                        items: Vec::new(),
                        project_accessible: false,
                    });
                }
                Err(error) => return Err(error),
            };
            items.extend(page);
            next = next_page(&headers)?;
        }
        Ok(Collection {
            items,
            project_accessible: true,
        })
    }

    async fn commit_discussions(
        &self,
        project_id: u64,
        commit: &CommitHash,
        skip_inaccessible_project: bool,
    ) -> Result<Collection<Discussion>, GitLabError> {
        let mut discussions = self
            .list_with_access::<Discussion>(
                &format!("projects/{project_id}/repository/commits/{commit}/discussions"),
                skip_inaccessible_project,
            )
            .await?;
        // Commit notes omit commit_id, so preserve the requested SHA
        // on every note, including replies whose root was deleted.
        for note in discussions
            .items
            .iter_mut()
            .flat_map(|discussion| &mut discussion.notes)
        {
            note.commit_id.get_or_insert_with(|| commit.clone());
        }
        Ok(discussions)
    }

    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, GitLabError> {
        let url = self.base.join(path).expect("valid GitLab API path");
        self.request(url, Method::POST, Some(body))
            .await
            .map(|(value, _)| value)
    }

    async fn request<T: DeserializeOwned>(
        &self,
        url: Url,
        method: Method,
        body: Option<&serde_json::Value>,
    ) -> Result<(T, HeaderMap), GitLabError> {
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || !url.path().starts_with(self.base.path())
            || url.fragment().is_some()
        {
            return Err(GitLabError::InvalidPagination);
        }
        let endpoint = url.path().to_string();
        trace!("sending GitLab request: method={method} endpoint={endpoint}");
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|source| GitLabError::Request {
                endpoint: endpoint.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let rate_limited = status.as_u16() == 429
                || (status.as_u16() == 403
                    && (response
                        .headers()
                        .get("ratelimit-remaining")
                        .is_some_and(|value| value == "0")
                        || response.headers().contains_key("retry-after")));
            // Only definite target validation errors permit retrying an inline
            // discussion without a position. Never retain error response bodies:
            // they can contain private note text or credentials.
            let (position_invalid, commit_invalid) = if matches!(status.as_u16(), 400 | 422) {
                response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|body| {
                        body.get("message")
                            .map(|message| (position_error(message), commit_error(message)))
                    })
                    .unwrap_or((false, false))
            } else {
                (false, false)
            };
            return Err(GitLabError::Api {
                endpoint,
                status: status.as_u16(),
                rate_limited,
                position_invalid,
                commit_invalid,
            });
        }
        let headers = response.headers().clone();
        let value = response.json().await.map_err(|source| {
            if source.is_decode() {
                GitLabError::Decode {
                    endpoint: endpoint.clone(),
                    source,
                }
            } else {
                GitLabError::Request {
                    endpoint: endpoint.clone(),
                    source,
                }
            }
        })?;
        Ok((value, headers))
    }
}

fn position_error(message: &serde_json::Value) -> bool {
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

fn commit_error(message: &serde_json::Value) -> bool {
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

fn private_token(token: &str) -> Result<HeaderValue, GitLabError> {
    if token.trim().is_empty() {
        return Err(GitLabError::MissingToken);
    }
    if !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(GitLabError::InvalidToken);
    }
    let mut value = HeaderValue::from_str(token).map_err(|_| GitLabError::InvalidToken)?;
    value.set_sensitive(true);
    Ok(value)
}

fn next_page(headers: &HeaderMap) -> Result<Option<Url>, GitLabError> {
    let mut next = None;
    for header in headers.get_all(LINK) {
        for link in header
            .to_str()
            .map_err(|_| GitLabError::InvalidPagination)?
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
                    .ok_or(GitLabError::InvalidPagination)?;
                if next.replace(url).is_some() {
                    return Err(GitLabError::InvalidPagination);
                }
            }
        }
    }
    Ok(next)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiffRefs {
    pub base_sha: CommitHash,
    pub head_sha: CommitHash,
    pub start_sha: CommitHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "provider", rename = "gitlab")]
pub struct GitLabReviewSource {
    pub project_id: u64,
    pub iid: u64,
    pub source_project_id: Option<u64>,
    pub diff_refs: DiffRefs,
}

impl<'de> Deserialize<'de> for GitLabReviewSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "provider", rename_all = "lowercase", deny_unknown_fields)]
        enum Source {
            Gitlab {
                project_id: u64,
                iid: u64,
                source_project_id: Option<u64>,
                diff_refs: DiffRefs,
            },
        }
        let Source::Gitlab {
            project_id,
            iid,
            source_project_id,
            diff_refs,
        } = Source::deserialize(deserializer)?;
        Ok(Self {
            project_id,
            iid,
            source_project_id,
            diff_refs,
        })
    }
}

#[derive(Debug)]
pub struct GitLabReviewInput {
    pub context: ReviewContext,
    pub commits: Vec<CommitHash>,
    pub source: GitLabReviewSource,
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub title: String,
    pub description: Option<String>,
    pub project_id: u64,
    pub iid: u64,
    pub source_project_id: Option<u64>,
    pub target_project_id: u64,
    pub source_branch: String,
    pub target_branch: String,
    pub sha: Option<CommitHash>,
    #[serde(default, deserialize_with = "deserialize_diff_refs")]
    pub diff_refs: Option<DiffRefs>,
}

impl MergeRequest {
    pub fn source(&self, number: NonZeroU64) -> Result<GitLabReviewSource, GitLabError> {
        if self.project_id == 0 {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.project_id != self.target_project_id {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.iid != number.get() {
            return Err(GitLabError::InvalidMergeRequest);
        }
        if self.source_project_id == Some(0) {
            return Err(GitLabError::InvalidMergeRequest);
        }
        let diff_refs = self
            .diff_refs
            .as_ref()
            .ok_or(GitLabError::MergeRequestNotReady)?;
        if self.sha.as_ref() != Some(&diff_refs.head_sha) {
            return Err(GitLabError::MergeRequestNotReady);
        }
        Ok(GitLabReviewSource {
            project_id: self.project_id,
            iid: self.iid,
            source_project_id: self.source_project_id,
            diff_refs: diff_refs.clone(),
        })
    }
}

fn deserialize_diff_refs<'de, D>(deserializer: D) -> Result<Option<DiffRefs>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct OptionalDiffRefs {
        base_sha: Option<CommitHash>,
        head_sha: Option<CommitHash>,
        start_sha: Option<CommitHash>,
    }
    let refs = Option::<OptionalDiffRefs>::deserialize(deserializer)?;
    Ok(refs.and_then(|refs| {
        Some(DiffRefs {
            base_sha: refs.base_sha?,
            head_sha: refs.head_sha?,
            start_sha: refs.start_sha?,
        })
    }))
}

#[derive(Debug, Deserialize)]
struct CommitRef {
    id: CommitHash,
}

#[derive(Debug, Deserialize)]
pub struct Discussion {
    pub id: String,
    pub notes: Vec<Note>,
}

#[derive(Debug, Deserialize)]
pub struct Note {
    pub id: u64,
    pub body: String,
    pub author: Option<User>,
    pub created_at: String,
    pub system: bool,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub confidential: bool,
    pub commit_id: Option<CommitHash>,
    pub position: Option<Position>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Position {
    pub position_type: String,
    pub head_sha: Option<CommitHash>,
    pub new_path: Option<String>,
    pub old_path: Option<String>,
    pub new_line: Option<NonZeroU32>,
    pub old_line: Option<NonZeroU32>,
}

#[cfg(test)]
mod tests;
