use serde_json::json;

use super::*;

#[tokio::test]
async fn posts_commit_discussion() {
    let server = Server::start(vec![Reply::json(json!({
        "notes": [{
            "id": 4
        }]
    }))])
    .await;
    let commit = commit();

    let result = create_commit_discussion(
        &client(&server),
        CreateCommitDiscussionInput {
            commit: CommitInput {
                repository: &repository(),
                commit: &commit,
            },
            body: "feedback",
        },
    )
    .await
    .unwrap();

    assert_eq!(result.notes[0].id.get(), 4);
    assert_eq!(
        server.posts(),
        vec![(
            format!(
                "POST /api/v4/projects/group%2Fproject/repository/commits/{commit}/discussions HTTP/1.1"
            ),
            json!({
                "body": "feedback"
            })
        )]
    );
}
