use serde::Serialize;

use crate::glab::GlabClient;

use super::{CommandError, MergeRequestInput, PublishedNote, api};

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
