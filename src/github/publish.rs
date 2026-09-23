use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::num::NonZeroU64;

use log::warn;
use serde::Deserialize;
use serde_json::json;

use crate::git::CommitHash;
use crate::render::RenderDocument;

use super::feedback::{PreparedReview, fingerprints, marker};
use super::position::{ChangedFile, comment_position};
use super::{GitHubClient, GitHubError, Repository};

pub const CONVERSATION_MARKER: &str = "<!-- peer-review:conversation:v1 -->";

#[derive(Debug, Default)]
pub struct PublishReport {
    pub urls: Vec<String>,
    pub published: usize,
    pub skipped: usize,
    pub inline: usize,
    pub recovered: usize,
}

impl fmt::Display for PublishReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Published {} comment(s).", self.published)?;
        write!(
            f,
            " {} inline, {} conversation.",
            self.inline,
            self.published - self.inline
        )?;
        write!(f, " Skipped {} duplicate item(s) or summary.", self.skipped)?;
        if self.recovered != 0 {
            write!(
                f,
                " Confirmed {} comment(s) after an uncertain response.",
                self.recovered
            )?;
        }
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
    html_url: Option<String>,
}

#[derive(Default)]
struct ExistingFeedback {
    fingerprints: HashSet<String>,
    urls: HashMap<String, String>,
}

impl GitHubClient {
    pub async fn publish(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        input: &RenderDocument,
    ) -> Result<PublishReport, GitHubError> {
        let pull = self.pull_request(repository, number).await?;
        let commits = self.pull_request_commits(repository, number, &pull).await?;
        let mut seen = self
            .existing_feedback(repository, number)
            .await?
            .fingerprints;
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
        let (files, files_loaded) = if remaining.iter().any(|item| item.location.is_some()) {
            match self
                .list::<ChangedFile>(&format!("repos/{repository}/pulls/{number}/files"))
                .await
            {
                Ok(files) => (files, true),
                Err(error) => {
                    warn!(
                        "Could not load changed files; collecting feedback in a conversation comment: {error}"
                    );
                    (Vec::new(), false)
                }
            }
        } else {
            (Vec::new(), false)
        };
        if files_loaded {
            let current = self.pull_request(repository, number).await?;
            if current.base.sha != pull.base.sha || current.head.sha != pull.head.sha {
                return Err(GitHubError::PullRequestChanged);
            }
        }
        let mut fallback_fingerprints = HashSet::new();
        let mut uncertain = Vec::new();
        for &item in &remaining {
            let position = resolve_commit(item.commit.as_ref(), &commits)
                .filter(|commit| *commit == &pull.head.sha)
                .and(item.location.as_ref())
                .and_then(|location| comment_position(&files, location));
            let Some(mut params) = position else {
                fallback_fingerprints.insert(&item.fingerprint);
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
                    report.published += 1;
                    report.inline += 1;
                }
                Err(error) => {
                    if error.may_have_published() {
                        uncertain.push((item, params["path"].take(), error));
                        continue;
                    }
                    warn!(
                        "Could not publish inline feedback at {}: {error}; collecting it in a conversation comment",
                        params["path"]
                    );
                    fallback_fingerprints.insert(&item.fingerprint);
                }
            }
        }
        if !uncertain.is_empty() {
            let confirmed = self
                .feedback_from_comments(&[format!("repos/{repository}/pulls/{number}/comments")])
                .await?;
            for (item, path, error) in uncertain {
                if confirmed.fingerprints.contains(&item.fingerprint) {
                    if let Some(url) = confirmed.urls.get(&item.fingerprint) {
                        report.urls.push(url.clone());
                    }
                    report.published += 1;
                    report.inline += 1;
                    report.recovered += 1;
                } else {
                    warn!(
                        "Could not publish inline feedback at {path}: {error}; collecting it in a conversation comment"
                    );
                    fallback_fingerprints.insert(&item.fingerprint);
                }
            }
        }
        let fallback = remaining
            .into_iter()
            .filter(|item| fallback_fingerprints.contains(&item.fingerprint))
            .collect::<Vec<_>>();
        let body = review.aggregate(&fallback, include_summary);
        if !body.trim().is_empty() {
            let body = format!("{body}\n\n{CONVERSATION_MARKER}");
            match self
                .post::<PublishedComment>(
                    &format!("repos/{repository}/issues/{number}/comments"),
                    &json!({
                        "body": body
                    }),
                )
                .await
            {
                Ok(comment) => {
                    report.urls.push(comment.html_url);
                    report.published += 1;
                }
                Err(error) => {
                    if !error.may_have_published() {
                        return Err(error);
                    }
                    let expected = fingerprints(&body);
                    let confirmed = self.existing_feedback(repository, number).await?;
                    if !expected.is_subset(&confirmed.fingerprints) {
                        return Err(error);
                    }
                    report.urls.extend(
                        expected
                            .iter()
                            .filter_map(|fingerprint| confirmed.urls.get(fingerprint))
                            .cloned()
                            .collect::<BTreeSet<_>>(),
                    );
                    report.published += 1;
                    report.recovered += 1;
                }
            }
        }
        Ok(report)
    }

    async fn existing_feedback(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<ExistingFeedback, GitHubError> {
        self.feedback_from_comments(&[
            format!("repos/{repository}/issues/{number}/comments"),
            format!("repos/{repository}/pulls/{number}/comments"),
        ])
        .await
    }

    async fn feedback_from_comments(
        &self,
        paths: &[String],
    ) -> Result<ExistingFeedback, GitHubError> {
        let mut existing = ExistingFeedback::default();
        for path in paths {
            for comment in self.list::<CommentBody>(path).await? {
                for fingerprint in fingerprints(&comment.body) {
                    if let Some(url) = &comment.html_url {
                        existing.urls.insert(fingerprint.clone(), url.clone());
                    }
                    existing.fingerprints.insert(fingerprint);
                }
            }
        }
        Ok(existing)
    }
}

/// Resolves a feedback target only when exactly one current PR commit matches.
fn resolve_commit<'a>(
    target: Option<&CommitHash>,
    commits: &'a [CommitHash],
) -> Option<&'a CommitHash> {
    let target = target?;
    let mut matches = commits.iter().filter(|commit| target.matches(commit));
    let commit = matches.next()?;
    matches.next().is_none().then_some(commit)
}
