use std::assert_matches;

use crate::gh::GhRunError;

use super::*;

#[tokio::test]
async fn conversation_feedback_accepts_optional_urls_and_rejects_partial_failure() {
    let gh = FakeGh::new(vec![
        Reply::plain(
            r#"{"comments":[{"body":"a","url":"https://github.com/comment"},{"body":"b"},{"body":"c","url":null}]}"#,
            0,
        ),
        Reply::plain(r#"{"comments":[{"body":"partial"}]}"#, 1),
    ]);
    let comments = list_conversation_feedback(&gh.client, input(&repository()))
        .await
        .unwrap();
    assert_eq!(comments.len(), 3);
    assert_eq!(
        comments[0].url.as_deref(),
        Some("https://github.com/comment")
    );
    assert_eq!(comments[1].url, None);
    assert_eq!(comments[2].url, None);
    assert_eq!(
        gh.args(1),
        [
            "pr",
            "view",
            "123",
            "--repo",
            "github.com/owner/repo",
            "--json",
            "comments"
        ]
    );
    let error = list_conversation_feedback(&gh.client, input(&repository())).await;
    assert_matches!(
        error,
        Err(CommandError::Run(GhRunError::NonZeroExit {
            status: Some(1),
            ..
        }))
    );
}
