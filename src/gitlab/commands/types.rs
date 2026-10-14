use std::num::{NonZeroU32, NonZeroU64};

use serde::{Deserialize, Deserializer};

use crate::git::CommitHash;
use crate::gitlab::DiffRefs;

#[derive(Debug, Deserialize)]
pub struct CommitRef {
    pub id: CommitHash,
    pub parent_ids: Option<Vec<CommitHash>>,
}

#[derive(Debug, Deserialize)]
pub struct PublishedNote {
    pub id: NonZeroU64,
}

#[derive(Debug, Deserialize)]
pub struct PublishedDiscussion {
    pub notes: Vec<PublishedNote>,
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub title: String,
    pub description: Option<String>,
    pub project_id: u64,
    pub iid: u64,
    pub source_project_id: Option<u64>,
    pub target_project_id: u64,
    pub source_branch: String,
    pub target_branch: String,
    pub sha: Option<CommitHash>,
    #[serde(default, deserialize_with = "deserialize_diff_refs")]
    pub diff_refs: Option<DiffRefs>,
}

fn deserialize_diff_refs<'de, D>(deserializer: D) -> Result<Option<DiffRefs>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct OptionalDiffRefs {
        base_sha: Option<CommitHash>,
        head_sha: Option<CommitHash>,
        start_sha: Option<CommitHash>,
    }
    let refs = Option::<OptionalDiffRefs>::deserialize(deserializer)?;
    Ok(refs.and_then(|refs| {
        Some(DiffRefs {
            base_sha: refs.base_sha?,
            head_sha: refs.head_sha?,
            start_sha: refs.start_sha?,
        })
    }))
}

#[derive(Debug, Deserialize)]
pub struct Discussion {
    pub id: String,
    pub notes: Vec<Note>,
}

#[derive(Debug, Deserialize)]
pub struct Note {
    pub id: u64,
    pub body: String,
    pub author: Option<User>,
    pub created_at: String,
    pub system: bool,
    #[serde(default)]
    pub internal: bool,
    #[serde(default)]
    pub confidential: bool,
    pub commit_id: Option<CommitHash>,
    pub position: Option<Position>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub username: String,
}

#[derive(Debug, Deserialize)]
pub struct Position {
    pub position_type: String,
    pub head_sha: Option<CommitHash>,
    pub new_path: Option<String>,
    pub old_path: Option<String>,
    pub new_line: Option<NonZeroU32>,
    pub old_line: Option<NonZeroU32>,
}
