mod client;
mod error;
mod mapping;
mod repository;

pub use self::error::GitLabError;

#[cfg_attr(not(test), expect(dead_code))]
pub const CONVERSATION_MARKER: &str = "<!-- peer-review:conversation:v1 -->";
