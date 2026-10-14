use std::num::NonZeroU64;

use crate::glab::GlabClient;

use super::super::Repository;
use super::{CommandError, MergeRequest, api};

#[derive(Clone, Copy)]
pub struct MergeRequestInput<'a> {
    pub repository: &'a Repository,
    pub number: NonZeroU64,
}

impl MergeRequestInput<'_> {
    pub fn path(self) -> String {
        format!(
            "{}/merge_requests/{}",
            self.repository.api_path(),
            self.number
        )
    }
}

pub async fn get_merge_request(
    client: &GlabClient,
    input: MergeRequestInput<'_>,
) -> Result<MergeRequest, CommandError> {
    api::get(client, &input.path()).await
}
