use std::collections::HashSet;
use std::num::NonZeroU64;
use std::time::Duration;

use log::trace;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, LINK};
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};

use crate::git::CommitHash;

use super::GitLabError;

const API_URL: &str = "https://gitlab.com/api/v4/";

#[cfg_attr(not(test), expect(dead_code))]
pub struct GitLabClient {
    http: Client,
    base: Url,
}

impl GitLabClient {
    #[expect(dead_code)]
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

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, GitLabError> {
        let url = self.base.join(path).expect("valid GitLab API path");
        self.request(url, Method::GET, None)
            .await
            .map(|(value, _)| value)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, GitLabError> {
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
            let (page, headers) = self.request::<Vec<T>>(url, Method::GET, None).await?;
            items.extend(page);
            next = next_page(&headers)?;
        }
        Ok(items)
    }

    #[cfg_attr(not(test), expect(dead_code))]
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
            return Err(GitLabError::Api {
                endpoint,
                status: status.as_u16(),
                rate_limited,
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

#[derive(Debug, Deserialize)]
#[expect(dead_code)]
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
    #[cfg_attr(not(test), expect(dead_code))]
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

#[cfg_attr(not(test), expect(dead_code))]
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

#[cfg(test)]
mod tests;
