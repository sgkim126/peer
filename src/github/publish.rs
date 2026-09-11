use std::fmt;
use std::num::NonZeroU64;

use serde::Deserialize;
use serde_json::json;

use crate::render::{RenderInput, github};

use super::{GitHubClient, GitHubError, Repository};

#[derive(Debug, Default)]
pub struct PublishReport {
    pub urls: Vec<String>,
}

impl fmt::Display for PublishReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Published {} comment(s).", self.urls.len())?;
        for url in &self.urls {
            write!(f, "\n{url}")?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct PublishedComment {
    pub html_url: String,
}

impl GitHubClient {
    pub async fn publish(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        input: &RenderInput,
    ) -> Result<PublishReport, GitHubError> {
        self.pull_request(repository, number).await?;
        let body = github::render(input, &repository.to_string());
        let mut report = PublishReport::default();
        if !body.trim().is_empty() {
            let comment: PublishedComment = self
                .post(
                    &format!("repos/{repository}/issues/{number}/comments"),
                    &json!({
                        "body": body
                    }),
                )
                .await?;
            report.urls.push(comment.html_url);
        }
        Ok(report)
    }
}
