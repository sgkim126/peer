use serde_json::json;

use super::*;

#[tokio::test]
async fn pull_request_commit_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "sha":"abc1234"
    }]))]);
    let commits = list_pr_commits(&gh.client, input(&repository()))
        .await
        .unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].sha, commit());
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/commits?per_page=100"
    );
}
