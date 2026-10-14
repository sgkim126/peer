use serde_json::json;

use super::*;

#[tokio::test]
async fn reads_commit_feedback_without_context_fields() {
    let server = Server::start(vec![Reply::json(json!([{
        "notes": [{
            "id": 3,
            "body": "marker"
        }]
    }]))])
    .await;
    let commit = commit();

    let feedback = list_commit_feedback(
        &client(&server),
        CommitInput {
            repository: &repository(),
            commit: &commit,
        },
    )
    .await
    .unwrap();

    assert_eq!(feedback[0].notes[0].id.get(), 3);
    assert_eq!(feedback[0].notes[0].body, "marker");
    assert_eq!(
        server.invocation(0).0[1],
        format!(
            "https://gitlab.com/api/v4/projects/group%2Fproject/repository/commits/{commit}/discussions?per_page=100"
        )
    );
}
