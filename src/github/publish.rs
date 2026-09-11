use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;

use log::warn;
use serde::Deserialize;
use serde_json::json;

use crate::render::RenderInput;

use super::feedback::{PreparedReview, fingerprints, marker};
use super::position::{ChangedFile, comment_position};
use super::{GitHubClient, GitHubError, Repository};

#[derive(Debug, Default)]
pub struct PublishReport {
    pub urls: Vec<String>,
    pub skipped: usize,
    pub inline: usize,
}

impl fmt::Display for PublishReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Published {} comment(s).", self.urls.len())?;
        write!(
            f,
            " {} inline, {} conversation.",
            self.inline,
            self.urls.len() - self.inline
        )?;
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
        let pull = self.pull_request(repository, number).await?;
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
        let files = if remaining.iter().any(|item| item.location.is_some()) {
            match self
                .list::<ChangedFile>(&format!("repos/{repository}/pulls/{number}/files"))
                .await
            {
                Ok(files) => files,
                Err(error) => {
                    warn!(
                        "Could not load changed files; collecting feedback in a conversation comment: {error}"
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let mut fallback = Vec::new();
        for item in remaining {
            let position = item
                .location
                .as_ref()
                .and_then(|location| comment_position(&files, location));
            let Some(mut params) = position else {
                fallback.push(item);
                continue;
            };
            params["commit_id"] = json!(pull.head.sha);
            params["body"] = json!(format!("{}\n\n{}", item.body, marker(&item.fingerprint)));
            match self
                .post::<PublishedComment>(
                    &format!("repos/{repository}/pulls/{number}/comments"),
                    &params,
                )
                .await
            {
                Ok(comment) => {
                    report.urls.push(comment.html_url);
                    report.inline += 1;
                }
                Err(error) => {
                    warn!(
                        "Could not publish inline feedback at {}: {error}; collecting it in a conversation comment",
                        params["path"]
                    );
                    fallback.push(item);
                }
            }
        }
        let body = review.aggregate(&fallback, include_summary);
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
