use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize, Serializer, de};

use crate::console::Console;

use super::{GitError, InvalidCommitHashReason};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHash(String);

impl CommitHash {
    const MIN_LEN: usize = 7;
    const MAX_LEN: usize = 64;

    pub async fn resolve(rev: &str, dir: &Path, console: Console) -> Result<Self, GitError> {
        let rev_commit = format!("{rev}^{{commit}}");
        let output = super::run_git(
            &["rev-parse", "--verify", "--end-of-options", &rev_commit],
            dir,
            console,
        )
        .await
        .map_err(|err| match err {
            GitError::NonZeroExit { .. } => GitError::InvalidRevision(rev.to_string()),
            err => err,
        })?;
        let commit_hash = output.trim();
        Self::new(commit_hash)
    }

    pub fn new(s: &str) -> Result<Self, GitError> {
        if s.len() < Self::MIN_LEN {
            return Err(GitError::InvalidCommitHash {
                value: s.to_string(),
                reason: InvalidCommitHashReason::TooShort,
            });
        }
        if Self::MAX_LEN < s.len() {
            return Err(GitError::InvalidCommitHash {
                value: s.to_string(),
                reason: InvalidCommitHashReason::TooLong,
            });
        }
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(GitError::InvalidCommitHash {
                value: s.to_string(),
                reason: InvalidCommitHashReason::InvalidCharacter,
            });
        }
        Ok(Self(s.to_string()))
    }

    /// Returns true when the hashes are identical or one is a prefix of the other.
    pub fn matches(&self, other: &Self) -> bool {
        self.0.starts_with(&other.0) || other.0.starts_with(&self.0)
    }
}

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for CommitHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for CommitHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for CommitHash {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(&s).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    async fn create_repo_with_commit() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let console = Console::default();

        super::super::run_git(&["init"], tmp.path(), console)
            .await
            .unwrap();
        super::super::run_git(
            &["config", "user.email", "test@example.com"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();
        super::super::run_git(&["config", "user.name", "Test User"], tmp.path(), console)
            .await
            .unwrap();
        super::super::run_git(
            &["commit", "--allow-empty", "-m", "initial commit"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();

        tmp
    }

    #[tokio::test]
    async fn resolve_resolves_head_to_its_full_commit_hash() {
        let tmp = create_repo_with_commit().await;
        let console = Console::default();
        let expected = super::super::run_git(
            &["rev-parse", "--verify", "HEAD^{commit}"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();

        let hash = CommitHash::resolve("HEAD", tmp.path(), console)
            .await
            .unwrap();

        assert_eq!(hash.as_ref(), expected.trim());
    }

    #[tokio::test]
    async fn resolve_peels_an_annotated_tag_to_its_commit_hash() {
        let tmp = create_repo_with_commit().await;
        let console = Console::default();
        super::super::run_git(
            &["tag", "-a", "v1.0.0", "-m", "release"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();

        let head = CommitHash::resolve("HEAD", tmp.path(), console)
            .await
            .unwrap();
        let tag = CommitHash::resolve("v1.0.0", tmp.path(), console)
            .await
            .unwrap();

        assert_eq!(tag, head);
    }

    #[tokio::test]
    async fn resolve_resolves_revision_starting_with_hyphen() {
        let tmp = create_repo_with_commit().await;
        let console = Console::default();
        super::super::run_git(
            &["update-ref", "refs/heads/-release", "HEAD"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();

        let head = CommitHash::resolve("HEAD", tmp.path(), console)
            .await
            .unwrap();
        let revision = CommitHash::resolve("-release", tmp.path(), console)
            .await
            .unwrap();

        assert_eq!(revision, head);
    }

    #[tokio::test]
    async fn resolve_makes_error_with_unknown_name() {
        let tmp = create_repo_with_commit().await;
        let console = Console::default();
        super::super::run_git(
            &["tag", "-a", "v1.0.0", "-m", "release"],
            tmp.path(),
            console,
        )
        .await
        .unwrap();

        let err = CommitHash::resolve("1.0.0", tmp.path(), console)
            .await
            .unwrap_err();

        assert_matches!(err, GitError::InvalidRevision(rev) if rev == "1.0.0");
    }

    #[test]
    fn commit_hash_valid_is_accepted() {
        let hash1 = "a".repeat(7);
        CommitHash::new(&hash1).unwrap();

        let hash2 = "b".repeat(64);
        CommitHash::new(&hash2).unwrap();
    }

    #[test]
    fn matches_identical_and_abbreviated_hashes() {
        let full = CommitHash::new("abc1234567890").unwrap();
        let abbreviated = CommitHash::new("abc1234").unwrap();
        let different = CommitHash::new("def5678").unwrap();

        assert!(full.matches(&full));
        assert!(full.matches(&abbreviated));
        assert!(abbreviated.matches(&full));
        assert!(!full.matches(&different));
    }

    #[test]
    fn commit_hash_too_short_is_rejected() {
        let hash = "c".repeat(6);
        assert_matches!(
            CommitHash::new(&hash),
            Err(GitError::InvalidCommitHash {
                value,
                reason: InvalidCommitHashReason::TooShort,
            }) if value == hash,
        );
    }

    #[test]
    fn commit_hash_too_long_is_rejected() {
        let hash = "d".repeat(65);
        assert_matches!(
            CommitHash::new(&hash),
            Err(GitError::InvalidCommitHash {
                value,
                reason: InvalidCommitHashReason::TooLong,
            }) if value == hash,
        );
    }

    #[test]
    fn commit_hash_uppercase_is_rejected() {
        let hash = "DEADBEEF";
        assert_matches!(
            CommitHash::new(hash),
            Err(GitError::InvalidCommitHash {
                value,
                reason: InvalidCommitHashReason::InvalidCharacter,
            }) if value == hash,
        );
    }

    #[test]
    fn commit_hash_non_hex_chars_are_rejected() {
        let hash = "xyzxyzx";
        assert_matches!(
            CommitHash::new(hash),
            Err(GitError::InvalidCommitHash {
                value,
                reason: InvalidCommitHashReason::InvalidCharacter,
            }) if value == hash,
        );
    }

    #[test]
    fn commit_hash_space_is_rejected() {
        let hash = "dead be";
        assert_matches!(
            CommitHash::new(hash),
            Err(GitError::InvalidCommitHash {
                value,
                reason: InvalidCommitHashReason::InvalidCharacter,
            }) if value == hash,
        );
    }

    #[test]
    fn commit_hash_display_matches_input() {
        let h = CommitHash::new("deadbeef123456a").unwrap();

        assert_eq!(h.to_string(), "deadbeef123456a");
    }

    #[test]
    fn commit_hash_as_ref_matches_input() {
        let h = CommitHash::new("deadbeef123456a").unwrap();

        assert_eq!(h.as_ref(), "deadbeef123456a");
    }
}
