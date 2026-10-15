use std::assert_matches;
use std::path::PathBuf;
use std::time::Duration;

use crate::gh::GhRunError;

use super::*;

#[tokio::test]
async fn conversation_creation_keeps_body_on_stdin_and_parses_url() {
    let gh = FakeGh::new(vec![Reply::plain(
        "https://github.com/owner/repo/pull/123#issuecomment-1\n",
        0,
    )]);
    let body = "feedback\n\n<!-- peer -->";
    let published: PublishedComment = create_conversation_comment(
        &gh.client,
        CreateConversationCommentInput {
            repository: &repository(),
            number: number(),
            body,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        published.url,
        "https://github.com/owner/repo/pull/123#issuecomment-1"
    );
    assert_eq!(
        gh.args(1),
        [
            "pr",
            "comment",
            "123",
            "--repo",
            "github.com/owner/repo",
            "--body-file",
            "-"
        ]
    );
    assert_eq!(gh.stdin(1), body.as_bytes());
}

async fn assert_conversation_write_may_have_published(reply: Reply) {
    let gh = FakeGh::new(vec![reply]);
    let error = create_conversation_comment(
        &gh.client,
        CreateConversationCommentInput {
            repository: &repository(),
            number: number(),
            body: "feedback",
        },
    )
    .await
    .unwrap_err();
    assert!(error.may_have_published());
    assert_eq!(gh.requests(), 1);
}

#[tokio::test]
async fn conversation_write_with_empty_output_may_have_published() {
    assert_conversation_write_may_have_published(Reply::plain("", 0)).await;
}

#[tokio::test]
async fn conversation_write_with_an_invalid_url_may_have_published() {
    assert_conversation_write_may_have_published(Reply::plain("not a URL", 0)).await;
}

#[tokio::test]
async fn conversation_write_with_multiple_urls_may_have_published() {
    assert_conversation_write_may_have_published(Reply::plain(
        "https://github.com/a\nhttps://github.com/b",
        0,
    ))
    .await;
}

#[tokio::test]
async fn conversation_write_with_a_failed_exit_status_may_have_published() {
    assert_conversation_write_may_have_published(Reply::plain("", 1)).await;
}

#[tokio::test]
async fn spawn_failure_cannot_have_published() {
    let dir = tempfile::tempdir().unwrap();
    let executable: PathBuf = dir.path().join("missing-gh");
    let client = GhClient::new(executable, Duration::from_secs(2));
    let error = create_conversation_comment(
        &client,
        CreateConversationCommentInput {
            repository: &repository(),
            number: number(),
            body: "feedback",
        },
    )
    .await
    .unwrap_err();
    assert_matches!(error, CommandError::Run(GhRunError::Spawn(_)));
    assert!(!error.may_have_published());
}
