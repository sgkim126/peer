use std::assert_matches;

use serde::Deserialize;
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
async fn paginates_lists_in_api_order() {
    let gh = FakeGh::new(vec![
        Reply::api(
            200,
            &[
                "Link: <https://api.github.com/repos/owner/repo/pulls/123/commits?page=2>; rel=\"next\"",
            ],
            r#"[{"sha":"0123456"}]"#,
        ),
        Reply::json(json!([{
            "sha":"abc1234"
        }])),
    ]);
    let commits = api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits")
        .await
        .unwrap();
    assert_eq!(
        commits,
        vec![json!({"sha":"0123456"}), json!({"sha":"abc1234"})]
    );
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/commits?per_page=100"
    );
    assert_eq!(
        gh.args(2)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/commits?page=2"
    );
}

#[tokio::test]
async fn paginates_nested_file_objects_in_api_order() {
    #[derive(Deserialize)]
    struct CommitFiles {
        files: Vec<Value>,
    }

    let gh = FakeGh::new(vec![
        Reply::api(
            200,
            &[
                "Link: <https://api.github.com/repos/owner/repo/commits/abc1234?page=2>; rel=\"next\"",
            ],
            r#"{"files":[{"filename":"a.rs"}]}"#,
        ),
        Reply::json(json!({
            "files":[{
                "filename":"b.rs"
            }]
        })),
    ]);
    let files = api::pages(
        &gh.client,
        "repos/owner/repo/commits/abc1234",
        false,
        |page: CommitFiles| page.files,
    )
    .await
    .unwrap();
    assert_eq!(
        files.items,
        vec![json!({"filename":"a.rs"}), json!({"filename":"b.rs"})]
    );
    assert!(files.repository_accessible);
}

#[tokio::test]
async fn skips_source_repository_missing_on_first_page() {
    let gh = FakeGh::new(vec![Reply::api(404, &[], "not JSON")]);
    let comments = api::pages(
        &gh.client,
        "repos/owner/repo/commits/abc1234/comments",
        true,
        |page: Vec<Value>| page,
    )
    .await
    .unwrap();
    assert!(!comments.repository_accessible);
    assert_matches!(comments.items[..], []);
}

#[tokio::test]
async fn skips_source_repository_requiring_sso_on_first_page() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &["X-GitHub-SSO: required; url=https://github.com/sso"],
        "not JSON",
    )]);
    let comments = api::pages(
        &gh.client,
        "repos/owner/repo/commits/abc1234/comments",
        true,
        |page: Vec<Value>| page,
    )
    .await
    .unwrap();
    assert!(!comments.repository_accessible);
    assert_matches!(comments.items[..], []);
}

#[tokio::test]
async fn skips_source_repository_denied_to_integration_on_first_page() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &[],
        r#"{"message":"Resource not accessible by integration"}"#,
    )]);
    let comments = api::pages(
        &gh.client,
        "repos/owner/repo/commits/abc1234/comments",
        true,
        |page: Vec<Value>| page,
    )
    .await
    .unwrap();
    assert!(!comments.repository_accessible);
    assert_matches!(comments.items[..], []);
}

#[tokio::test]
async fn rejects_missing_source_page_after_an_empty_first_page() {
    let gh = FakeGh::new(vec![
        Reply::api(
            200,
            &[
                "Link: <https://api.github.com/repos/owner/repo/commits/abc1234/comments?page=2>; rel=\"next\"",
            ],
            "[]",
        ),
        Reply::api(404, &[], "{}"),
    ]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api { status: 404, .. }),
    );
    assert_eq!(gh.requests(), 2);
}

#[tokio::test]
async fn recognizes_429_as_rate_limited_despite_malformed_body() {
    let gh = FakeGh::new(vec![Reply::api(429, &[], "not JSON")]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api {
            rate_limited: true,
            permission_denied: false,
            ..
        }),
    );
}

#[tokio::test]
async fn recognizes_exhausted_quota_despite_malformed_body() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &["X-RateLimit-Remaining: 0"],
        "not JSON",
    )]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api {
            rate_limited: true,
            permission_denied: false,
            ..
        }),
    );
}

#[tokio::test]
async fn retry_after_takes_precedence_over_permission_denial() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &["Retry-After: 30"],
        r#"{"message":"Resource not accessible by integration"}"#,
    )]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api {
            rate_limited: true,
            permission_denied: false,
            ..
        }),
    );
}

#[tokio::test]
async fn secondary_rate_limit_takes_precedence_over_sso_requirement() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &["X-GitHub-SSO: required"],
        r#"{"message":"You have exceeded a secondary rate limit."}"#,
    )]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api {
            rate_limited: true,
            permission_denied: false,
            ..
        }),
    );
}

#[tokio::test]
async fn recognizes_primary_rate_limit_message_without_headers() {
    let gh = FakeGh::new(vec![Reply::api(
        403,
        &[],
        r#"{"message":"API rate limit exceeded for user."}"#,
    )]);
    assert_matches!(
        api::pages(
            &gh.client,
            "repos/owner/repo/commits/abc1234/comments",
            true,
            |page: Vec<Value>| page,
        )
        .await,
        Err(CommandError::Api {
            rate_limited: true,
            permission_denied: false,
            ..
        }),
    );
}

#[tokio::test]
async fn rejects_next_link_to_another_host() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &["Link: <https://example.com/page>; rel=\"next\""],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn rejects_next_link_to_http() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &["Link: <http://api.github.com/page>; rel=\"next\""],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn rejects_next_link_with_username() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &["Link: <https://user@api.github.com/page>; rel=\"next\""],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn rejects_malformed_next_link() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &["Link: <not-a-url>; rel=\"next\""],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn rejects_multiple_next_links() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &[
            "Link: <https://api.github.com/a>; rel=\"next\", <https://api.github.com/b>; rel=\"next\"",
        ],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn rejects_next_link_back_to_the_current_page() {
    let gh = FakeGh::new(vec![Reply::api(
        200,
        &[
            "Link: <https://api.github.com/repos/owner/repo/pulls/123/commits?per_page=100>; rel=\"next\"",
        ],
        "[]",
    )]);
    assert_matches!(
        api::list::<Value>(&gh.client, "repos/owner/repo/pulls/123/commits").await,
        Err(CommandError::InvalidPagination),
    );
    assert_eq!(gh.requests(), 1);
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
