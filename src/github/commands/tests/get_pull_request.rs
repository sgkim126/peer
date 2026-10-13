use serde_json::json;

use super::*;

#[tokio::test]
async fn pull_request_read_selects_its_typed_endpoint() {
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
    let pull = get_pull_request(&gh.client, input(&repository()))
        .await
        .unwrap();
    assert_eq!(pull.title, "Title");
    assert_eq!(pull.commits, 1);
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123"
    );
}
