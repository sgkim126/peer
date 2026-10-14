mod api;
mod discussions;
mod error;
mod merge_request;
mod types;

pub use api::Collection;
pub use discussions::{
    CreateCommitDiscussionInput, CreateMrDiscussionInput, CreateNoteInput, DiscussionPosition,
    DiscussionTarget, ListCommitDiscussionsInput, PositionKind, create_commit_discussion,
    create_mr_discussion, create_note, list_commit_discussions, list_commit_feedback,
    list_mr_discussions, list_mr_feedback,
};
pub use error::CommandError;
pub use merge_request::{
    CommitInput, MergeRequestInput, get_merge_request, list_commit_diff, list_mr_commits,
};
pub use types::{
    ChangedFile, CommitRef, Discussion, FeedbackDiscussion, MergeRequest, Note,
    PublishedDiscussion, PublishedNote,
};

#[cfg(test)]
pub use types::FeedbackNote;

#[cfg(all(test, unix))]
mod tests;
