use std::num::NonZeroU64;

use serde::{Deserialize, Deserializer};

use crate::git::CommitHash;
use crate::gitlab::DiffRefs;

#[derive(Debug, Deserialize)]
pub struct PublishedNote {
    pub id: NonZeroU64,
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
