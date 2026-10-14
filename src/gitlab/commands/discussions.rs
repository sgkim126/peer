use std::num::NonZeroU32;

use serde::Serialize;

use crate::git::CommitHash;
use crate::glab::GlabClient;

use super::{
    Collection, CommandError, CommitInput, Discussion, FeedbackDiscussion, MergeRequestInput,
    PublishedDiscussion, PublishedNote, api,
};

pub struct ListCommitDiscussionsInput<'a> {
    pub project_id: u64,
    pub commit: &'a CommitHash,
    pub skip_inaccessible_project: bool,
}

pub async fn list_mr_discussions(
    client: &GlabClient,
    input: MergeRequestInput<'_>,
) -> Result<Vec<Discussion>, CommandError> {
    api::list(client, &format!("{}/discussions", input.path())).await
}

pub async fn list_commit_discussions(
    client: &GlabClient,
    input: ListCommitDiscussionsInput<'_>,
) -> Result<Collection<Discussion>, CommandError> {
    let mut collection = api::pages::<Discussion>(
        client,
        &format!(
            "projects/{}/repository/commits/{}/discussions",
            input.project_id, input.commit,
        ),
        input.skip_inaccessible_project,
    )
    .await?;
    // Commit notes omit commit_id; keep the requested SHA even on replies whose
    // root note has been deleted.
    for note in collection
        .items
        .iter_mut()
        .flat_map(|discussion| &mut discussion.notes)
    {
        note.commit_id.get_or_insert_with(|| input.commit.clone());
    }
    Ok(collection)
}

pub async fn list_mr_feedback(
    client: &GlabClient,
    input: MergeRequestInput<'_>,
) -> Result<Vec<FeedbackDiscussion>, CommandError> {
    api::list(client, &format!("{}/discussions", input.path())).await
}

pub async fn list_commit_feedback(
    client: &GlabClient,
    input: CommitInput<'_>,
) -> Result<Vec<FeedbackDiscussion>, CommandError> {
    api::list(client, &format!("{}/discussions", input.path())).await
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscussionPosition {
    pub base_sha: CommitHash,
    pub start_sha: CommitHash,
    pub head_sha: CommitHash,
    pub old_path: String,
    pub new_path: String,
    #[serde(flatten)]
    pub kind: PositionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "position_type", rename_all = "lowercase")]
pub enum PositionKind {
    File,
    Text {
        new_line: NonZeroU32,
        #[serde(skip_serializing_if = "Option::is_none")]
        old_line: Option<NonZeroU32>,
    },
}

pub struct DiscussionTarget<'a> {
    pub commit: &'a CommitHash,
    pub position: &'a DiscussionPosition,
}

pub struct CreateMrDiscussionInput<'a> {
    pub merge_request: MergeRequestInput<'a>,
    pub body: &'a str,
    pub target: Option<DiscussionTarget<'a>>,
}

pub async fn create_mr_discussion(
    client: &GlabClient,
    input: CreateMrDiscussionInput<'_>,
) -> Result<PublishedDiscussion, CommandError> {
    #[derive(Serialize)]
    struct Body<'a> {
        body: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit_id: Option<&'a CommitHash>,
        #[serde(skip_serializing_if = "Option::is_none")]
        position: Option<&'a DiscussionPosition>,
    }
    let body = Body {
        body: input.body,
        commit_id: input.target.as_ref().map(|target| target.commit),
        position: input.target.as_ref().map(|target| target.position),
    };
    api::post(
        client,
        &format!("{}/discussions", input.merge_request.path()),
        &body,
    )
    .await
}

pub struct CreateCommitDiscussionInput<'a> {
    pub commit: CommitInput<'a>,
    pub body: &'a str,
}

pub async fn create_commit_discussion(
    client: &GlabClient,
    input: CreateCommitDiscussionInput<'_>,
) -> Result<PublishedDiscussion, CommandError> {
    api::post(
        client,
        &format!("{}/discussions", input.commit.path()),
        &NoteBody { body: input.body },
    )
    .await
}

pub struct CreateNoteInput<'a> {
    pub merge_request: MergeRequestInput<'a>,
    pub body: &'a str,
}

pub async fn create_note(
    client: &GlabClient,
    input: CreateNoteInput<'_>,
) -> Result<PublishedNote, CommandError> {
    // `glab api` uses the raw HTTP client. The high-level note command uses the
    // SDK's retrying client, which cannot preserve uncertain-write recovery.
    api::post(
        client,
        &format!("{}/notes", input.merge_request.path()),
        &NoteBody { body: input.body },
    )
    .await
}

#[derive(Serialize)]
struct NoteBody<'a> {
    body: &'a str,
}
