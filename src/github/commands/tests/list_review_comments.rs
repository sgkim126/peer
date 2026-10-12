use serde_json::json;

use super::*;

#[tokio::test]
async fn review_comment_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "id":3,
        "created_at":"2026-01-01T00:00:00Z",
        "user":null,
        "body":"inline",
        "path":"a.rs",
        "commit_id":"abc1234",
        "line":7,
        "side":"RIGHT"
    }]))]);
    assert_eq!(
        list_review_comments(&gh.client, input(&repository()))
            .await
            .unwrap()[0]
            .body,
        "inline"
    );
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/comments?per_page=100"
    );
}
