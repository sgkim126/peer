use serde::Serialize;

use crate::git::CommitHash;
use crate::glab::GlabClient;

use super::{
    Collection, CommandError, CommitInput, Discussion, MergeRequestInput, PublishedDiscussion,
    PublishedNote, api,
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
