use serde_json::json;

use super::*;

#[tokio::test]
async fn review_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "id":2,
        "state":"COMMENTED",
        "submitted_at":null,
        "user":null,
        "body":"review",
        "commit_id":"abc1234"
    }]))]);
    assert_eq!(
        list_reviews(&gh.client, input(&repository()))
            .await
            .unwrap()[0]
            .body
            .as_deref(),
        Some("review")
    );
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/reviews?per_page=100"
    );
}
