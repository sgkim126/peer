pub mod api;
mod discussions;
mod error;
mod merge_request;
mod types;

pub use api::Collection;
pub use discussions::{CreateNoteInput, create_note};
pub use error::CommandError;
pub use merge_request::{MergeRequestInput, get_merge_request};
pub use types::{MergeRequest, PublishedNote};

#[cfg(all(test, unix))]
mod tests;
