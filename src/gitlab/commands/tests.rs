use std::time::Duration;

use crate::git::CommitHash;
use crate::gitlab::Repository;
use crate::gitlab::test_support::{Reply, Server};
use crate::glab::GlabClient;

use super::*;

#[cfg_attr(not(test), expect(dead_code))]
fn client(server: &Server) -> GlabClient {
    server.glab(Duration::from_secs(10))
}

#[expect(dead_code)]
fn repository() -> Repository {
    Repository::parse("group/project").unwrap()
}

#[expect(dead_code)]
fn commit() -> CommitHash {
    CommitHash::new(&"a".repeat(40)).unwrap()
}

mod api;
