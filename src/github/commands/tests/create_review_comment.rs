use std::num::NonZeroU32;

use serde_json::json;

use super::*;

const COMMENT_BODY: &str = "line one\n\nline two $(literal) `literal` \\\"";

async fn review_comment_request(target: ReviewCommentTarget) -> Value {
    let gh = FakeGh::new(vec![Reply::json(json!({
        "html_url":"https://github.com/owner/repo/pull/123#discussion_r1"
    }))]);
    create_review_comment(
        &gh.client,
        CreateReviewCommentInput {
            repository: &repository(),
            number: number(),
            commit_id: &commit(),
            body: COMMENT_BODY,
            target: &target,
        },
    )
    .await
    .unwrap();

    let args = gh.args(1);
    assert!(args.windows(2).any(|args| args == ["--method", "POST"]));
    assert!(args.windows(2).any(|args| args == ["--input", "-"]));
    assert!(args.iter().all(|arg| !arg.contains(COMMENT_BODY)));
    serde_json::from_slice(&gh.stdin(1)).unwrap()
}

#[tokio::test]
async fn review_creation_serializes_line_targets_without_file_fields() {
    let body = review_comment_request(ReviewCommentTarget::Line {
        path: "a.rs".into(),
        line: NonZeroU32::new(7).unwrap(),
    })
    .await;
    assert_eq!(
        body,
        json!({
            "body": COMMENT_BODY,
            "commit_id":"abc1234",
            "path":"a.rs",
            "line":7,
            "side":"RIGHT"
        })
    );
}

#[tokio::test]
async fn review_creation_serializes_file_targets_without_line_fields() {
    let body = review_comment_request(ReviewCommentTarget::File {
        path: "a.rs".into(),
    })
    .await;
    assert_eq!(
        body,
        json!({
            "body": COMMENT_BODY,
            "commit_id":"abc1234",
            "path":"a.rs",
            "subject_type":"file"
        })
    );
}

#[tokio::test]
async fn review_write_with_invalid_json_may_have_published() {
    let gh = FakeGh::new(vec![Reply::api(200, &[], "invalid JSON")]);
    let error = create_review_comment(
        &gh.client,
        CreateReviewCommentInput {
            repository: &repository(),
            number: number(),
            commit_id: &commit(),
            body: "feedback",
            target: &ReviewCommentTarget::File {
                path: "a.rs".into(),
            },
        },
    )
    .await
    .unwrap_err();
    assert!(error.may_have_published());
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn review_write_with_server_error_may_have_published() {
    let gh = FakeGh::new(vec![Reply::api(503, &[], "{}")]);
    let error = create_review_comment(
        &gh.client,
        CreateReviewCommentInput {
            repository: &repository(),
            number: number(),
            commit_id: &commit(),
            body: "feedback",
            target: &ReviewCommentTarget::File {
                path: "a.rs".into(),
            },
        },
    )
    .await
    .unwrap_err();
    assert!(error.may_have_published());
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn review_write_with_http_408_may_have_published() {
    let gh = FakeGh::new(vec![Reply::api(408, &[], "{}")]);
    let error = create_review_comment(
        &gh.client,
        CreateReviewCommentInput {
            repository: &repository(),
            number: number(),
            commit_id: &commit(),
            body: "feedback",
            target: &ReviewCommentTarget::File {
                path: "a.rs".into(),
            },
        },
    )
    .await
    .unwrap_err();
    assert!(error.may_have_published());
    assert_eq!(gh.requests(), 1);
}
