use super::*;

use std::assert_matches;

#[test]
fn treats_an_empty_token_as_missing() {
    assert_matches!(private_token(""), Err(GitLabError::MissingToken));
}

#[test]
fn treats_a_space_only_token_as_missing() {
    assert_matches!(private_token(" "), Err(GitLabError::MissingToken));
}

#[test]
fn treats_control_whitespace_only_tokens_as_missing() {
    assert_matches!(private_token("\t\r\n"), Err(GitLabError::MissingToken));
}

#[test]
fn rejects_tokens_with_leading_spaces() {
    assert_matches!(private_token(" leading"), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_trailing_spaces() {
    assert_matches!(private_token("trailing "), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_embedded_spaces() {
    assert_matches!(private_token("with space"), Err(GitLabError::InvalidToken));
}

#[test]
fn rejects_tokens_with_embedded_newlines() {
    assert_matches!(
        private_token("secret\nheader"),
        Err(GitLabError::InvalidToken)
    );
}

#[test]
fn rejects_tokens_with_null_bytes() {
    assert_matches!(private_token("secret\0"), Err(GitLabError::InvalidToken));
}

#[test]
fn private_token_headers_are_sensitive() {
    let token = private_token("glpat-secret-value").unwrap();

    assert!(token.is_sensitive());
}

#[test]
fn debug_output_does_not_expose_private_tokens() {
    let token = private_token("glpat-secret-value").unwrap();

    assert!(!format!("{token:?}").contains("glpat-secret-value"));
}
