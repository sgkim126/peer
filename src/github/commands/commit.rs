use serde::Deserialize;

use crate::gh::GhClient;
use crate::git::CommitHash;
use crate::github::Repository;
use crate::github::position::ChangedFile;

use super::{CommandError, api};

#[derive(Clone, Copy)]
pub struct CommitInput<'a> {
    pub repository: &'a Repository,
    pub commit: &'a CommitHash,
}

#[derive(Deserialize)]
struct CommitFiles {
    files: Vec<ChangedFile>,
}

pub async fn get_commit_files(
    client: &GhClient,
    input: CommitInput<'_>,
) -> Result<Vec<ChangedFile>, CommandError> {
    Ok(api::pages(
        client,
        &format!("repos/{}/commits/{}", input.repository, input.commit),
        false,
        |page: CommitFiles| page.files,
    )
    .await?
    .items)
}
