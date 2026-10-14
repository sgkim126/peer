use std::assert_matches;

use serde_json::json;

use super::*;

#[tokio::test]
async fn gets_merge_request_from_encoded_namespace() {
    let server = Server::start(vec![Reply::json(json!({
        "title": "Review",
        "description": null,
        "project_id": 5,
        "iid": 7,
        "source_project_id": null,
        "target_project_id": 5,
        "source_branch": "feature",
        "target_branch": "main",
        "sha": null,
        "diff_refs": {
            "base_sha": null,
            "head_sha": null,
            "start_sha": null
        }
    }))])
    .await;
    let repository = Repository::parse("group/subgroup/project").unwrap();

    let result = get_merge_request(&client(&server), input(&repository))
        .await
        .unwrap();

    assert_eq!(result.title, "Review");
    assert_matches!(result.diff_refs, None);
    assert_eq!(
        server.invocation(0).0[1],
        "https://gitlab.com/api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/7"
    );
}
