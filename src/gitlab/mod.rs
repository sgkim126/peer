mod client;
mod error;
mod mapping;
mod position;
mod repository;

pub use self::client::{DiffRefs, GitLabClient, GitLabReviewSource};
pub use self::error::GitLabError;
pub use self::repository::Repository;

pub const CONVERSATION_MARKER: &str = "<!-- peer-review:conversation:v1 -->";
