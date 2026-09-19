use std::fmt;

use super::GitLabError;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct Repository(String);

impl Repository {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn parse(value: &str) -> Result<Self, GitLabError> {
        let mut parts = value.split('/');
        // These paths identify existing repositories. GitLab's stricter rules
        // for creating or renaming paths would reject some legacy repositories.
        let valid = |part: &str| {
            !part.is_empty()
                && !part.starts_with('-')
                && !matches!(part, "." | "..")
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        };
        if !parts.next().is_some_and(valid) {
            return Err(GitLabError::InvalidRepository);
        }
        if !parts.next().is_some_and(valid) {
            return Err(GitLabError::InvalidRepository);
        }
        if !parts.all(valid) {
            return Err(GitLabError::InvalidRepository);
        }
        Ok(Self(value.to_string()))
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub fn api_path(&self) -> String {
        // The entire namespace/project path is one GitLab API parameter. All
        // accepted characters except '/' are already safe path characters.
        format!("projects/{}", self.0.replace('/', "%2F"))
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn accepts_projects_in_a_top_level_group() {
        let repository = Repository::parse("group/project").unwrap();

        assert_eq!(repository.to_string(), "group/project");
    }

    #[test]
    fn preserves_the_original_project_path() {
        let repository = Repository::parse("Group.Name/sub_group/project-1").unwrap();

        assert_eq!(repository.to_string(), "Group.Name/sub_group/project-1");
    }

    #[test]
    fn accepts_projects_in_nested_subgroups() {
        let repository = Repository::parse("a/b/c/d").unwrap();

        assert_eq!(repository.to_string(), "a/b/c/d");
    }

    #[test]
    fn accepts_legacy_project_names_ending_in_a_dot() {
        let repository = Repository::parse("group/project.").unwrap();

        assert_eq!(repository.to_string(), "group/project.");
    }

    #[test]
    fn accepts_legacy_subgroup_names_with_consecutive_dots() {
        let repository = Repository::parse("group/a..b/project").unwrap();

        assert_eq!(repository.to_string(), "group/a..b/project");
    }

    #[test]
    fn accepts_legacy_namespace_names_starting_with_an_underscore() {
        let repository = Repository::parse("_group/project").unwrap();

        assert_eq!(repository.to_string(), "_group/project");
    }

    #[test]
    fn encodes_the_full_project_path_as_one_parameter() {
        let repository = Repository::parse("Group.Name/sub_group/project-1").unwrap();
        assert_eq!(
            repository.api_path(),
            "projects/Group.Name%2Fsub_group%2Fproject-1"
        );
    }

    #[test]
    fn rejects_an_empty_project_path() {
        assert_matches!(Repository::parse(""), Err(GitLabError::InvalidRepository));
    }

    #[test]
    fn rejects_a_namespace_without_a_project() {
        assert_matches!(
            Repository::parse("group"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_an_empty_namespace() {
        assert_matches!(
            Repository::parse("/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_an_empty_project_name() {
        assert_matches!(
            Repository::parse("group/"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_an_empty_subgroup() {
        assert_matches!(
            Repository::parse("group//project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_namespace_names_starting_with_a_hyphen() {
        assert_matches!(
            Repository::parse("-group/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_project_names_starting_with_a_hyphen() {
        assert_matches!(
            Repository::parse("group/-project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_subgroup_names_starting_with_a_hyphen() {
        assert_matches!(
            Repository::parse("group/-subgroup/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_nested_project_names_starting_with_a_hyphen() {
        assert_matches!(
            Repository::parse("group/subgroup/-project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_the_current_directory_as_a_namespace() {
        assert_matches!(
            Repository::parse("./project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_parent_directory_traversal_in_the_namespace() {
        assert_matches!(
            Repository::parse("group/../project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_the_current_directory_as_a_subgroup() {
        assert_matches!(
            Repository::parse("group/./project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_the_parent_directory_as_a_project() {
        assert_matches!(
            Repository::parse("group/project/.."),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_leading_whitespace_in_the_namespace() {
        assert_matches!(
            Repository::parse(" group/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_trailing_whitespace_in_the_project_name() {
        assert_matches!(
            Repository::parse("group/project "),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_spaces_inside_project_names() {
        assert_matches!(
            Repository::parse("group/pro ject"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_query_strings_in_project_paths() {
        assert_matches!(
            Repository::parse("group/project?query"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_url_fragments_in_project_paths() {
        assert_matches!(
            Repository::parse("group/project#fragment"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_an_encoded_namespace_separator() {
        assert_matches!(
            Repository::parse("group%2Fproject"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_an_encoded_subgroup_separator() {
        assert_matches!(
            Repository::parse("group/sub%2Fproject"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_full_repository_urls() {
        assert_matches!(
            Repository::parse("https://gitlab.com/group/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_backslashes_as_path_separators() {
        assert_matches!(
            Repository::parse("group\\project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_non_ascii_namespace_names() {
        assert_matches!(
            Repository::parse("소유자/project"),
            Err(GitLabError::InvalidRepository)
        );
    }

    #[test]
    fn rejects_numeric_project_ids() {
        assert_matches!(
            Repository::parse("123"),
            Err(GitLabError::InvalidRepository)
        );
    }
}
