use std::fmt;

use super::GitHubError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    pub fn parse(value: &str) -> Result<Self, GitHubError> {
        let Some((owner, name)) = value.split_once('/') else {
            return Err(GitHubError::InvalidRepository);
        };
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        };
        if !valid(owner) || !valid(name) {
            return Err(GitHubError::InvalidRepository);
        }
        Ok(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn preserves_repository_spelling() {
        let value = "owner/repository";
        assert_eq!(Repository::parse(value).unwrap().to_string(), value);
    }

    #[test]
    fn preserves_repository_spelling_with_mixed_case_and_punctuation() {
        let value = "Org.Name/project_name-1";
        assert_eq!(Repository::parse(value).unwrap().to_string(), value);
    }

    #[test]
    fn rejects_empty_repository() {
        assert_matches!(Repository::parse(""), Err(_));
    }

    #[test]
    fn rejects_repository_without_separator() {
        assert_matches!(Repository::parse("owner"), Err(_));
    }

    #[test]
    fn rejects_empty_owner() {
        assert_matches!(Repository::parse("/repo"), Err(_));
    }

    #[test]
    fn rejects_empty_repository_name() {
        assert_matches!(Repository::parse("owner/"), Err(_));
    }

    #[test]
    fn rejects_repository_with_extra_separator() {
        assert_matches!(Repository::parse("a/b/c"), Err(_));
    }

    #[test]
    fn rejects_repository_name_with_spaces() {
        assert_matches!(Repository::parse("a/b c"), Err(_));
    }

    #[test]
    fn rejects_repository_with_query() {
        assert_matches!(Repository::parse("a/b?c"), Err(_));
    }

    #[test]
    fn rejects_repository_with_fragment() {
        assert_matches!(Repository::parse("a/b#c"), Err(_));
    }

    #[test]
    fn rejects_leading_whitespace() {
        assert_matches!(Repository::parse(" owner/repo"), Err(_));
    }

    #[test]
    fn rejects_trailing_whitespace() {
        assert_matches!(Repository::parse("owner/repo "), Err(_));
    }

    #[test]
    fn rejects_repository_url() {
        assert_matches!(Repository::parse("https://github.com/owner/repo"), Err(_));
    }

    #[test]
    fn rejects_non_ascii_owner() {
        assert_matches!(Repository::parse("소유자/repo"), Err(_));
    }
}
