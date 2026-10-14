pub mod api;
mod discussions;
mod error;
mod merge_request;
mod types;

pub use api::Collection;
pub use discussions::{
    CreateCommitDiscussionInput, CreateNoteInput, ListCommitDiscussionsInput,
    create_commit_discussion, create_note, list_commit_discussions, list_mr_discussions,
};
pub use error::CommandError;
pub use merge_request::{CommitInput, MergeRequestInput, get_merge_request, list_mr_commits};
pub use types::{CommitRef, Discussion, MergeRequest, Note, PublishedDiscussion, PublishedNote};

#[cfg(all(test, unix))]
mod tests;
