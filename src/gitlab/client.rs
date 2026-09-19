use std::time::Duration;

use log::trace;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use reqwest::{Client, Method, Url};
#[cfg(test)]
use serde::Deserialize;
use serde::de::DeserializeOwned;

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

#[cfg(test)]
mod tests;
