use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use reqwest::{Client, Url};

use super::GitLabError;

const API_URL: &str = "https://gitlab.com/api/v4/";

#[expect(dead_code)]
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
