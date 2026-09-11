use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;

use serde::Deserialize;
use serde_json::json;

use crate::render::RenderInput;

use super::feedback::{PreparedReview, fingerprints};
use super::{GitHubClient, GitHubError, Repository};

#[derive(Debug, Default)]
pub struct PublishReport {
    pub urls: Vec<String>,
    pub skipped: usize,
}

impl fmt::Display for PublishReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Published {} comment(s).", self.urls.len())?;
        write!(f, " Skipped {} duplicate item(s) or summary.", self.skipped)?;
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

#[derive(Deserialize)]
struct CommentBody {
    body: String,
}

impl GitHubClient {
    pub async fn publish(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        input: &RenderInput,
    ) -> Result<PublishReport, GitHubError> {
        self.pull_request(repository, number).await?;
        let mut seen = self.existing_fingerprints(repository, number).await?;
        let review = PreparedReview::new(input, repository);
        let mut report = PublishReport::default();
        let remaining = review
            .items
            .iter()
            .filter(|item| {
                if seen.insert(item.fingerprint.clone()) {
                    true
                } else {
                    report.skipped += 1;
                    false
                }
            })
            .collect::<Vec<_>>();
        let include_summary = review
            .summary_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| {
                if seen.insert(fingerprint.clone()) {
                    true
                } else {
                    report.skipped += 1;
                    false
                }
            });
        let body = review.aggregate(&remaining, include_summary);
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

    async fn existing_fingerprints(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<HashSet<String>, GitHubError> {
        let mut seen = HashSet::new();
        for path in [
            format!("repos/{repository}/issues/{number}/comments"),
            format!("repos/{repository}/pulls/{number}/comments"),
        ] {
            for comment in self.list::<CommentBody>(&path).await? {
                seen.extend(fingerprints(&comment.body));
            }
        }
        Ok(seen)
    }
}
