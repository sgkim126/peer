use serde_json::json;

use super::*;

#[tokio::test]
async fn issue_comment_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "id":1,"created_at":"2026-01-01T00:00:00Z","user":null,"body":"conversation"
    }]))]);
    assert_eq!(
        list_issue_comments(&gh.client, input(&repository()))
            .await
            .unwrap()[0]
            .body,
        "conversation"
    );
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/issues/123/comments?per_page=100"
    );
}
