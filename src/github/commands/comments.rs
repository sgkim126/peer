use std::num::NonZeroU64;

use serde::Deserialize;
use url::Url;

use crate::gh::{GhClient, GhInvocation};
use crate::git::CommitHash;
use crate::github::Repository;

use super::{
    Collection, CommandError, CommitComment, IssueComment, PullRequestInput, PullRequestReview,
    ReviewComment, api,
};

#[derive(Debug, Deserialize)]
pub struct FeedbackComment {
    pub body: String,
    #[serde(rename = "html_url")]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct PublishedComment {
    #[serde(rename = "html_url")]
    pub url: String,
}

pub struct ListCommitCommentsInput<'a> {
    pub repository: &'a Repository,
    pub commit: &'a CommitHash,
    pub skip_inaccessible: bool,
}

pub struct CreateConversationCommentInput<'a> {
    pub repository: &'a Repository,
    pub number: NonZeroU64,
    pub body: &'a str,
}

pub async fn list_issue_comments(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<IssueComment>, CommandError> {
    api::list(client, &format!("{}/comments", input.issue_path())).await
}

pub async fn list_reviews(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<PullRequestReview>, CommandError> {
    api::list(client, &format!("{}/reviews", input.path())).await
}

pub async fn list_review_comments(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<ReviewComment>, CommandError> {
    api::list(client, &format!("{}/comments", input.path())).await
}

pub async fn list_commit_comments(
    client: &GhClient,
    input: ListCommitCommentsInput<'_>,
) -> Result<Collection<CommitComment>, CommandError> {
    api::pages(
        client,
        &format!(
            "repos/{}/commits/{}/comments",
            input.repository, input.commit
        ),
        input.skip_inaccessible,
        |page: Vec<CommitComment>| page,
    )
    .await
}

pub async fn list_review_feedback(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<FeedbackComment>, CommandError> {
    api::list(client, &format!("{}/comments", input.path())).await
}

pub async fn list_conversation_feedback(
    client: &GhClient,
    input: PullRequestInput<'_>,
) -> Result<Vec<FeedbackComment>, CommandError> {
    #[derive(Deserialize)]
    struct ConversationComments {
        comments: Vec<ConversationComment>,
    }

    #[derive(Deserialize)]
    struct ConversationComment {
        body: String,
        url: Option<String>,
    }

    let output = client
        .run(&GhInvocation {
            args: vec![
                "pr".into(),
                "view".into(),
                input.number.to_string(),
                "--repo".into(),
                input.repo_arg(),
                "--json".into(),
                "comments".into(),
            ],
            stdin: None,
        })
        .await?;
    // gh pr view completes all comment pages before emitting its JSON output.
    Ok(api::decode::<ConversationComments>(output, "gh pr view")?
        .comments
        .into_iter()
        .map(|comment| FeedbackComment {
            body: comment.body,
            url: comment.url,
        })
        .collect())
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn create_conversation_comment(
    client: &GhClient,
    input: CreateConversationCommentInput<'_>,
) -> Result<PublishedComment, CommandError> {
    let output = client
        .run(&GhInvocation {
            args: vec![
                "pr".into(),
                "comment".into(),
                input.number.to_string(),
                "--repo".into(),
                format!("github.com/{}", input.repository),
                "--body-file".into(),
                "-".into(),
            ],
            stdin: Some(input.body.as_bytes().to_vec()),
        })
        .await?;
    if !output.status.success() {
        return Err(api::nonzero(output));
    }
    let invalid = || CommandError::InvalidResponse {
        command: "gh pr comment".into(),
    };
    let text = std::str::from_utf8(&output.stdout).map_err(|_| invalid())?;
    let text = text.trim();
    let url = Url::parse(text).map_err(|_| invalid())?;
    if text.is_empty() || text.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(invalid());
    }
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(invalid());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid());
    }
    Ok(PublishedComment { url: text.into() })
}
