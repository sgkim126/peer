use std::assert_matches;

use serde_json::json;

use super::super::api;
use super::*;

#[tokio::test]
async fn get_uses_explicit_api_contract() {
    let gh = FakeGh::new(vec![Reply::json(json!({
        "title": "Title",
        "body": null,
        "commits": 1,
        "base": {
            "sha":"0123456"
        },
        "head": {
            "sha":"abc1234"
        }
    }))]);
    let pull: Value = api::get(&gh.client, "repos/owner/repo/pulls/123")
        .await
        .unwrap();
    assert_eq!(pull["title"], "Title");
    assert_eq!(pull["commits"], 1);
    assert_eq!(
        gh.args(1),
        [
            "api",
            "https://api.github.com/repos/owner/repo/pulls/123",
            "--hostname",
            "github.com",
            "--method",
            "GET",
            "--include",
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "X-GitHub-Api-Version: 2026-03-10",
        ]
    );
    assert_matches!(gh.stdin(1)[..], []);
}

#[tokio::test]
async fn parses_gh_mixed_line_endings() {
    let gh = FakeGh::new(vec![Reply::plain(
        "HTTP/2.0 200 OK\nContent-Type: application/json\r\n\r\n{\"title\":\"Title\"}",
        0,
    )]);
    let pull: Value = api::get(&gh.client, "repos/owner/repo/pulls/123")
        .await
        .unwrap();
    assert_eq!(pull["title"], "Title");
}

#[tokio::test]
async fn parses_gh_headerless_lf_response() {
    // CRLF pairs inside the body must not be mistaken for the header boundary.
    let gh = FakeGh::new(vec![Reply::plain(
        "HTTP/2.0 200 OK\n\n{\r\n\r\n\"title\":\"Title\"}",
        0,
    )]);
    let pull: Value = api::get(&gh.client, "repos/owner/repo/pulls/123")
        .await
        .unwrap();
    assert_eq!(pull["title"], "Title");
}

#[tokio::test]
async fn successful_posts_do_not_interpret_pagination_links() {
    let gh = FakeGh::new(vec![Reply::api(
        201,
        &["Link: <not-a-url>; rel=\"next\""],
        r#"{"html_url":"https://github.com/owner/repo/pull/123#discussion_r1"}"#,
    )]);
    let published: Value = api::post(
        &gh.client,
        "repos/owner/repo/pulls/123/comments",
        &json!({
            "body":"feedback",
            "commit_id":"abc1234",
            "path":"a.rs",
            "subject_type":"file"
        }),
    )
    .await
    .unwrap();
    assert!(
        published["html_url"]
            .as_str()
            .unwrap()
            .ends_with("#discussion_r1")
    );
}

#[tokio::test]
async fn preserves_definitive_http_errors_with_malformed_bodies() {
    let gh = FakeGh::new(vec![Reply::api(403, &[], "not JSON")]);
    let error = api::post::<Value>(
        &gh.client,
        "repos/owner/repo/pulls/123/comments",
        &json!({
            "body":"feedback",
            "commit_id":"abc1234",
            "path":"a.rs",
            "subject_type":"file"
        }),
    )
    .await
    .unwrap_err();
    assert_matches!(
        error,
        CommandError::Api {
            status: 403,
            rate_limited: false,
            permission_denied: false,
            ..
        }
    );
    assert!(!error.may_have_published());
}
