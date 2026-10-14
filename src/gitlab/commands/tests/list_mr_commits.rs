use serde_json::json;

use super::*;

#[tokio::test]
async fn lists_merge_request_commits() {
    let commit = commit();
    let server = Server::start(vec![Reply::json(json!([{
        "id": commit,
        "parent_ids": []
    }]))])
    .await;

    let commits = list_mr_commits(&client(&server), input(&repository()))
        .await
        .unwrap();

    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].id, commit);
    assert_eq!(commits[0].parent_ids, Some(vec![]));
    assert_eq!(
        server.invocation(0).0[1],
        "https://gitlab.com/api/v4/projects/group%2Fproject/merge_requests/7/commits?per_page=100"
    );
}
