use std::fmt;

use serde::{Deserialize, Serialize, Serializer, de};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitHash(String);

impl CommitHash {
    const MIN_LEN: usize = 7;
    const MAX_LEN: usize = 40;

    pub fn new(s: &str) -> Result<Self, String> {
        if s.len() < Self::MIN_LEN || s.len() > Self::MAX_LEN {
            return Err(format!(
                "commit hash must be {}-{} characters, got {}: {s:?}",
                Self::MIN_LEN,
                Self::MAX_LEN,
                s.len(),
            ));
        }
        if !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("commit hash must be lowercase hex: {s:?}"));
        }
        Ok(Self(s.to_string()))
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

    #[test]
    fn commit_hash_valid_is_accepted() {
        let hash1 = "deadbee";

        assert_eq!(hash1.len(), 7);
        CommitHash::new(hash1).unwrap();

        let hash2 = "abc123def456789012345678901234567890abcd";

        assert_eq!(hash2.len(), 40);
        CommitHash::new(hash2).unwrap();
    }

    #[test]
    fn commit_hash_too_short_is_rejected() {
        let hash = "deadbe";

        assert_eq!(hash.len(), 6);
        CommitHash::new(hash).unwrap_err();
    }

    #[test]
    fn commit_hash_too_long_is_rejected() {
        let hash = "abc123def456789012345678901234567890abcde";
        assert_eq!(hash.len(), 41);

        CommitHash::new(hash).unwrap_err();
    }

    #[test]
    fn commit_hash_uppercase_is_rejected() {
        CommitHash::new("DEADBEEF").unwrap_err();
    }

    #[test]
    fn commit_hash_non_hex_chars_are_rejected() {
        CommitHash::new("xyzxyzx").unwrap_err();
        CommitHash::new("dead be").unwrap_err();
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
