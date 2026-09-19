use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;

use serde::Deserialize;

use crate::feedback::{Feedback, PreparedReview, marker};
use crate::git::CommitHash;
use crate::render::RenderDocument;
use crate::stage::StageTarget;

use super::client::MergeRequest;
use super::{CONVERSATION_MARKER, GitLabClient, GitLabError, GitLabReviewSource, Repository};

const MAX_NOTE_CHARACTERS: usize = 1_000_000;

#[derive(Clone, Debug, Default)]
#[expect(dead_code)]
pub struct PublishReport {
    pub urls: Vec<String>,
    pub published: usize,
    pub skipped: usize,
    pub inline: usize,
    pub recovered: usize,
}

impl PublishReport {
    #[expect(dead_code)]
    fn record(&mut self, url: String, inline: bool, recovered: bool) {
        self.urls.push(url);
        self.published += 1;
        self.inline += usize::from(inline);
        self.recovered += usize::from(recovered);
    }
}

impl fmt::Display for PublishReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Published {} comment(s). {} inline, {} conversation. Skipped {} duplicate item(s) or summary.",
            self.published,
            self.inline,
            self.published - self.inline,
            self.skipped,
        )?;
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

#[derive(Debug)]
#[expect(dead_code)]
pub struct PublishError {
    pub report: PublishReport,
    reason: Box<PublishFailure>,
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.reason, self.report)
    }
}

impl std::error::Error for PublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.reason.as_ref())
    }
}

impl From<GitLabError> for PublishError {
    fn from(error: GitLabError) -> Self {
        Self {
            report: PublishReport::default(),
            reason: Box::new(PublishFailure::Api(error)),
        }
    }
}

#[derive(Debug)]
#[expect(dead_code)]
enum PublishFailure {
    Api(GitLabError),
    SnapshotMismatch,
    InvalidReviewCommits,
    NoteTooLong,
    Unconfirmed {
        request: Option<GitLabError>,
        verification: Option<GitLabError>,
    },
}

impl fmt::Display for PublishFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => error.fmt(f),
            Self::SnapshotMismatch => write!(
                f,
                "GitLab merge request differs from the reviewed snapshot; review its current diff before publishing"
            ),
            Self::InvalidReviewCommits => write!(
                f,
                "review commits do not identify the current GitLab merge request head and its commits"
            ),
            Self::NoteTooLong => write!(
                f,
                "GitLab feedback exceeds the 1,000,000-character note limit"
            ),
            Self::Unconfirmed {
                request,
                verification,
            } => {
                write!(
                    f,
                    "GitLab publication could not be confirmed; stopped without posting a duplicate fallback"
                )?;
                if let Some(error) = request {
                    write!(f, ": {error}")?;
                }
                if let Some(error) = verification {
                    write!(f, "; verification failed: {error}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PublishFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Api(error) => Some(error),
            Self::Unconfirmed {
                request,
                verification,
            } => request
                .as_ref()
                .or(verification.as_ref())
                .map(|error| error as &(dyn std::error::Error + 'static)),
            _ => None,
        }
    }
}

impl From<GitLabError> for PublishFailure {
    fn from(error: GitLabError) -> Self {
        Self::Api(error)
    }
}

#[derive(Deserialize)]
#[expect(dead_code)]
struct CommitRef {
    id: CommitHash,
}

#[derive(Deserialize)]
#[expect(dead_code)]
struct PublishedNote {
    id: NonZeroU64,
}

#[derive(Deserialize)]
#[expect(dead_code)]
struct PublishedDiscussion {
    notes: Vec<PublishedNote>,
}

#[derive(Deserialize)]
#[expect(dead_code)]
struct ExistingDiscussion {
    notes: Vec<ExistingNote>,
}

#[derive(Deserialize)]
#[expect(dead_code)]
struct ExistingNote {
    id: NonZeroU64,
    body: String,
    #[serde(default)]
    system: bool,
    #[serde(default)]
    internal: bool,
    #[serde(default)]
    confidential: bool,
}

#[derive(Default)]
#[expect(dead_code)]
struct ExistingFeedback {
    fingerprints: HashSet<String>,
    notes: Vec<(HashSet<String>, String)>,
}

impl GitLabClient {
    #[expect(dead_code)]
    async fn assert_current(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        expected: &MergeRequest,
    ) -> Result<(), PublishFailure> {
        let current = self.merge_request(repository, number).await?;
        if current.source(number)? != expected.source(number)? {
            return Err(PublishFailure::SnapshotMismatch);
        }
        if current.source_branch != expected.source_branch {
            return Err(PublishFailure::SnapshotMismatch);
        }
        if current.target_branch != expected.target_branch {
            return Err(PublishFailure::SnapshotMismatch);
        }
        Ok(())
    }
}

#[expect(dead_code)]
fn validate_commits(
    input: &RenderDocument,
    commits: &[CommitRef],
    source: &GitLabReviewSource,
) -> Result<(), PublishFailure> {
    let unique: HashSet<_> = commits.iter().map(|commit| commit.id.as_ref()).collect();
    if commits.is_empty() {
        return Err(GitLabError::IncompleteCommits.into());
    }
    if unique.len() != commits.len() {
        return Err(GitLabError::IncompleteCommits.into());
    }
    if !commits
        .iter()
        .any(|commit| commit.id == source.diff_refs.head_sha)
    {
        return Err(GitLabError::IncompleteCommits.into());
    }
    let mut referenced: Vec<_> = input
        .ordered_commits
        .iter()
        .chain(input.findings.iter().map(|finding| &finding.commit))
        .chain(
            input
                .questions
                .iter()
                .flat_map(|question| question.related_commits.iter()),
        )
        .chain(
            input
                .questions
                .iter()
                .filter_map(|question| question.location.as_ref().map(|location| &location.commit)),
        )
        .chain(
            input
                .recommendations
                .iter()
                .flat_map(|recommendation| recommendation.related_commits.iter()),
        )
        .collect();
    for stage in &input.stages {
        match &stage.target {
            StageTarget::Commit(commit) => referenced.push(commit),
            StageTarget::Range { from, to } => {
                if !from.matches(&source.diff_refs.base_sha)
                    && !from.matches(&source.diff_refs.start_sha)
                {
                    referenced.push(from);
                }
                referenced.push(to);
            }
        }
    }
    if referenced.iter().any(|hash| {
        commits
            .iter()
            .filter(|commit| hash.matches(&commit.id))
            .count()
            != 1
    }) {
        return Err(PublishFailure::InvalidReviewCommits);
    }
    if input.source.is_none() {
        let head = &source.diff_refs.head_sha;
        let has_head = input.ordered_commits.last().map_or_else(
            || referenced.iter().any(|commit| commit.matches(head)),
            |commit| commit.matches(head),
        );
        if !has_head {
            return Err(PublishFailure::InvalidReviewCommits);
        }
    }
    Ok(())
}

#[expect(dead_code)]
fn head_location(item: &Feedback, source: &GitLabReviewSource) -> bool {
    item.location.is_some()
        && item
            .commit
            .as_ref()
            .is_some_and(|commit| commit.matches(&source.diff_refs.head_sha))
}

#[expect(dead_code)]
fn inline_body(item: &Feedback) -> String {
    format!("{}\n\n{}", item.body, marker(&item.fingerprint))
}

#[expect(dead_code)]
fn conversation_body(
    review: &PreparedReview,
    items: &[&Feedback],
    include_summary: bool,
) -> String {
    let body = review.aggregate(items, include_summary);
    if body.trim().is_empty() {
        String::new()
    } else {
        format!("{body}\n\n{CONVERSATION_MARKER}")
    }
}

#[cfg_attr(not(test), expect(dead_code))]
fn check_body(body: &str) -> Result<(), PublishFailure> {
    if body.chars().take(MAX_NOTE_CHARACTERS + 1).count() > MAX_NOTE_CHARACTERS {
        Err(PublishFailure::NoteTooLong)
    } else {
        Ok(())
    }
}

#[expect(dead_code)]
fn note_url(repository: &Repository, number: NonZeroU64, id: NonZeroU64) -> String {
    format!("https://gitlab.com/{repository}/-/merge_requests/{number}#note_{id}")
}

#[cfg(test)]
mod tests;
