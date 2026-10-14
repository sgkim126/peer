use serde_json::json;

use super::*;

#[tokio::test]
async fn lists_merge_request_context_discussions() {
    let server = Server::start(vec![Reply::json(json!([{
        "id": "thread",
        "notes": [{
            "id": 11,
            "body": "body",
            "author": {
                "username": "reviewer"
            },
            "created_at": "2026-10-12",
            "system": false,
            "commit_id": null,
            "position": null
        }]
    }]))])
    .await;

    let discussions = list_mr_discussions(&client(&server), input(&repository()))
        .await
        .unwrap();

    assert_eq!(discussions.len(), 1);
    assert_eq!(discussions[0].id, "thread");
    assert_eq!(discussions[0].notes[0].body, "body");
    assert_eq!(
        server.invocation(0).0[1],
        "https://gitlab.com/api/v4/projects/group%2Fproject/merge_requests/7/discussions?per_page=100"
    );
}
