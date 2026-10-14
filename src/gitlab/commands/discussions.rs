use serde::Serialize;

use crate::glab::GlabClient;

use super::{
    CommandError, CommitInput, MergeRequestInput, PublishedDiscussion, PublishedNote, api,
};

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
