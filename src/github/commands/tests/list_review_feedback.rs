use serde_json::json;

use super::*;

#[tokio::test]
async fn review_feedback_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "body":"feedback",
        "html_url":null
    }]))]);
    let feedback: Vec<FeedbackComment> = list_review_feedback(&gh.client, input(&repository()))
        .await
        .unwrap();
    assert_eq!(feedback[0].body, "feedback");
    assert_eq!(feedback[0].url, None);
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/comments?per_page=100"
    );
}

#[tokio::test]
async fn review_feedback_selects_browser_url_when_rest_also_returns_api_url() {
    let gh = FakeGh::new(vec![Reply::json(json!([
        {
            "body":"feedback",
            "url":"https://api.github.com/repos/owner/repo/pulls/comments/1",
            "html_url":"https://github.com/owner/repo/pull/123#discussion_r1"
        },
        {
            "body":"no browser URL",
            "url":"https://api.github.com/repos/owner/repo/pulls/comments/2"
        }
    ]))]);
    let comments = list_review_feedback(&gh.client, input(&repository()))
        .await
        .unwrap();
    assert_eq!(comments[0].body, "feedback");
    assert_eq!(
        comments[0].url.as_deref(),
        Some("https://github.com/owner/repo/pull/123#discussion_r1")
    );
    assert_eq!(comments[1].body, "no browser URL");
    assert_eq!(comments[1].url, None);
}
