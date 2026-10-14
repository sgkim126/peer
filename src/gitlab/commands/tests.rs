mod api;
mod create_note;
mod get_merge_request;

use std::num::NonZeroU64;
use std::time::Duration;

use crate::git::CommitHash;
use crate::gitlab::Repository;
use crate::gitlab::test_support::{Reply, Server};
use crate::glab::GlabClient;

use super::*;

fn client(server: &Server) -> GlabClient {
    server.glab(Duration::from_secs(10))
}

fn repository() -> Repository {
    Repository::parse("group/project").unwrap()
}

#[expect(dead_code)]
fn commit() -> CommitHash {
    CommitHash::new(&"a".repeat(40)).unwrap()
}

fn input(repository: &Repository) -> MergeRequestInput<'_> {
    MergeRequestInput {
        repository,
        number: NonZeroU64::new(7).unwrap(),
    }
}
