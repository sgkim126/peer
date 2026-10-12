use std::num::NonZeroU32;

use serde_json::json;

use super::*;

#[tokio::test]
async fn review_creation_serializes_exclusive_line_and_file_targets() {
    let gh = FakeGh::new(vec![
        Reply::json(json!({
            "html_url":"https://github.com/owner/repo/pull/123#discussion_r1"
        })),
        Reply::json(json!({
            "html_url":"https://github.com/owner/repo/pull/123#discussion_r2"
        })),
    ]);
    let body = "line one\n\nline two $(literal) `literal` \\\"";
    for target in [
        ReviewCommentTarget::Line {
            path: "a.rs".into(),
            line: NonZeroU32::new(7).unwrap(),
        },
        ReviewCommentTarget::File {
            path: "a.rs".into(),
        },
    ] {
        create_review_comment(
            &gh.client,
            CreateReviewCommentInput {
                repository: &repository(),
                number: number(),
                commit_id: &commit(),
                body,
                target: &target,
            },
        )
        .await
        .unwrap();
    }
    let line: Value = serde_json::from_slice(&gh.stdin(1)).unwrap();
    assert_eq!(
        line,
        json!({
            "body":body,
            "commit_id":"abc1234",
            "path":"a.rs",
            "line":7,
            "side":"RIGHT"
        })
    );
    let file: Value = serde_json::from_slice(&gh.stdin(2)).unwrap();
    assert_eq!(
        file,
        json!({
            "body":body,
            "commit_id":"abc1234",
            "path":"a.rs",
            "subject_type":"file"
        })
    );
    let args = gh.args(1);
    assert!(args.windows(2).any(|args| args == ["--method", "POST"]));
    assert!(args.windows(2).any(|args| args == ["--input", "-"]));
    assert!(args.iter().all(|arg| !arg.contains(body)));
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
