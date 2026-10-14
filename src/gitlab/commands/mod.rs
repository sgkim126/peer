pub mod api;
mod discussions;
mod error;
mod merge_request;
mod types;

pub use api::Collection;
pub use discussions::{
    CreateCommitDiscussionInput, CreateNoteInput, create_commit_discussion, create_note,
};
pub use error::CommandError;
pub use merge_request::{CommitInput, MergeRequestInput, get_merge_request, list_mr_commits};
pub use types::{CommitRef, MergeRequest, PublishedDiscussion, PublishedNote};

#[cfg(all(test, unix))]
mod tests;
