use std::collections::HashSet;
use std::fmt;
use std::num::NonZeroU64;

use serde::Deserialize;
use serde_json::json;

use crate::feedback::{Feedback, PreparedReview, fingerprints, marker};
use crate::git::CommitHash;
use crate::render::RenderDocument;
use crate::stage::StageTarget;

use super::client::MergeRequest;
use super::position::{ChangedFile, comment_position};
use super::{CONVERSATION_MARKER, GitLabClient, GitLabError, GitLabReviewSource, Repository};

const MAX_NOTE_CHARACTERS: usize = 1_000_000;

#[derive(Clone, Debug, Default)]
pub struct PublishReport {
    pub urls: Vec<String>,
    pub published: usize,
    pub skipped: usize,
    pub inline: usize,
    pub recovered: usize,
}

impl PublishReport {
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
struct CommitRef {
    id: CommitHash,
}

#[derive(Deserialize)]
struct PublishedNote {
    id: NonZeroU64,
}

#[derive(Deserialize)]
struct PublishedDiscussion {
    notes: Vec<PublishedNote>,
}

#[derive(Deserialize)]
struct ExistingDiscussion {
    notes: Vec<ExistingNote>,
}

#[derive(Deserialize)]
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
struct ExistingFeedback {
    fingerprints: HashSet<String>,
    notes: Vec<(HashSet<String>, String)>,
}

impl GitLabClient {
    pub async fn publish(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        input: &RenderDocument,
    ) -> Result<PublishReport, PublishError> {
        let mut report = PublishReport::default();
        self.publish_review(repository, number, input, &mut report)
            .await
            .map_err(|reason| PublishError {
                report: report.clone(),
                reason: Box::new(reason),
            })?;
        Ok(report)
    }

    async fn publish_review(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        input: &RenderDocument,
        report: &mut PublishReport,
    ) -> Result<(), PublishFailure> {
        let review = PreparedReview::for_gitlab(input, repository);
        if review.items.is_empty() && review.summary_fingerprint.is_none() {
            return Ok(());
        }
        let merge_request = self.merge_request(repository, number).await?;
        let source = merge_request.source(number)?;
        if input
            .source
            .as_ref()
            .is_some_and(|expected| expected != &source)
        {
            return Err(PublishFailure::SnapshotMismatch);
        }
        let prefix = format!("{}/merge_requests/{number}", repository.api_path());
        let commits = self.list::<CommitRef>(&format!("{prefix}/commits")).await?;
        validate_commits(input, &commits, &source)?;

        let mut seen = self
            .existing_feedback(repository, number)
            .await?
            .fingerprints;
        let remaining: Vec<_> = review
            .items
            .iter()
            .filter(|item| {
                let is_new = seen.insert(item.fingerprint.clone());
                report.skipped += usize::from(!is_new);
                is_new
            })
            .collect();
        let include_summary = review
            .summary_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| {
                let is_new = seen.insert(fingerprint.clone());
                report.skipped += usize::from(!is_new);
                is_new
            });
        if remaining.is_empty() && !include_summary {
            return Ok(());
        }
        // Feedback items and the summary each have their own note limit.
        let summary = summary_body(&review, include_summary);
        check_body(&summary)?;
        for item in &remaining {
            check_body(&inline_body(item))?;
        }
        let has_head_location = remaining.iter().any(|item| head_location(item, &source));
        let files = if has_head_location {
            self.list::<ChangedFile>(&format!("{prefix}/diffs"))
                .await
                .or_else(|err| match err {
                    GitLabError::Api { status: 413, .. } => Ok(vec![]),
                    _ => Err(err),
                })?
        } else {
            Vec::new()
        };
        self.assert_current(repository, number, &merge_request)
            .await?;

        for item in remaining {
            let position = head_location(item, &source)
                .then_some(item.location.as_ref())
                .flatten()
                .and_then(|location| comment_position(&files, location, &source.diff_refs));
            let candidates = position.into_iter().map(Some).chain(std::iter::once(None));
            let body = inline_body(item);
            for position in candidates {
                self.assert_current(repository, number, &merge_request)
                    .await?;
                let inline = position.is_some();
                let mut payload = json!({ "body": body });
                if let Some(position) = position {
                    payload["position"] = position;
                }
                let result = self
                    .post::<PublishedDiscussion>(&format!("{prefix}/discussions"), &payload)
                    .await;
                match result {
                    Ok(discussion) => match discussion.notes.first() {
                        Some(note) => {
                            report.record(note_url(repository, number, note.id), inline, false)
                        }
                        None => {
                            let url = self
                                .confirm_publication(repository, number, &body, None)
                                .await?;
                            report.record(url, inline, true);
                        }
                    },
                    Err(error) if error.may_have_published() => {
                        let url = self
                            .confirm_publication(repository, number, &body, Some(error))
                            .await?;
                        report.record(url, inline, true);
                    }
                    Err(GitLabError::Api {
                        status: 400 | 422,
                        position_invalid: true,
                        ..
                    }) if inline => continue,
                    Err(GitLabError::Api {
                        status: 400 | 422,
                        commit_invalid: true,
                        ..
                    }) if inline => continue,
                    Err(error) => return Err(error.into()),
                }
                break;
            }
        }
        let body = summary;
        if !body.is_empty() {
            self.assert_current(repository, number, &merge_request)
                .await?;
            match self
                .post::<PublishedNote>(&format!("{prefix}/notes"), &json!({ "body": body }))
                .await
            {
                Ok(note) => report.record(note_url(repository, number, note.id), false, false),
                Err(error) if error.may_have_published() => {
                    let url = self
                        .confirm_publication(repository, number, &body, Some(error))
                        .await?;
                    report.record(url, false, true);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

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

    async fn existing_feedback(
        &self,
        repository: &Repository,
        number: NonZeroU64,
    ) -> Result<ExistingFeedback, GitLabError> {
        let mut result = ExistingFeedback::default();
        let path = format!(
            "{}/merge_requests/{number}/discussions",
            repository.api_path()
        );
        let discussions = self.list::<ExistingDiscussion>(&path).await?;
        for note in discussions
            .into_iter()
            .flat_map(|discussion| discussion.notes)
        {
            if note.system {
                continue;
            }
            if note.internal {
                continue;
            }
            if note.confidential {
                continue;
            }
            let markers = fingerprints(&note.body);
            if !markers.is_empty() {
                result.fingerprints.extend(markers.iter().cloned());
                result
                    .notes
                    .push((markers, note_url(repository, number, note.id)));
            }
        }
        Ok(result)
    }

    async fn confirm_publication(
        &self,
        repository: &Repository,
        number: NonZeroU64,
        body: &str,
        request: Option<GitLabError>,
    ) -> Result<String, PublishFailure> {
        let expected = fingerprints(body);
        match self.existing_feedback(repository, number).await {
            Ok(existing) => {
                if !expected.is_empty()
                    && let Some((_, url)) = existing
                        .notes
                        .into_iter()
                        .find(|(markers, _)| expected.is_subset(markers))
                {
                    return Ok(url);
                }
                Err(PublishFailure::Unconfirmed {
                    request,
                    verification: None,
                })
            }
            Err(error) => Err(PublishFailure::Unconfirmed {
                request,
                verification: Some(error),
            }),
        }
    }
}

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

fn head_location(item: &Feedback, source: &GitLabReviewSource) -> bool {
    item.location.is_some()
        && item
            .commit
            .as_ref()
            .is_some_and(|commit| commit.matches(&source.diff_refs.head_sha))
}

fn inline_body(item: &Feedback) -> String {
    format!("{}\n\n{}", item.body, marker(&item.fingerprint))
}

fn summary_body(review: &PreparedReview, include_summary: bool) -> String {
    let body = review.aggregate(&[], include_summary);
    if body.trim().is_empty() {
        String::new()
    } else {
        format!("{body}\n\n{CONVERSATION_MARKER}")
    }
}

fn check_body(body: &str) -> Result<(), PublishFailure> {
    if body.chars().take(MAX_NOTE_CHARACTERS + 1).count() > MAX_NOTE_CHARACTERS {
        Err(PublishFailure::NoteTooLong)
    } else {
        Ok(())
    }
}

fn note_url(repository: &Repository, number: NonZeroU64, id: NonZeroU64) -> String {
    format!("https://gitlab.com/{repository}/-/merge_requests/{number}#note_{id}")
}

#[cfg(test)]
mod tests;
