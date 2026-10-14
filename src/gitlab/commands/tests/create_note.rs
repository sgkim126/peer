use serde_json::json;

use super::*;

#[tokio::test]
async fn posts_merge_request_summary() {
    let server = Server::start(vec![
        Reply::json(json!({
            "id": 5
        }))
        .status(201),
    ])
    .await;

    let result = create_note(
        &client(&server),
        CreateNoteInput {
            merge_request: input(&repository()),
            body: "summary",
        },
    )
    .await
    .unwrap();

    assert_eq!(result.id.get(), 5);
    assert_eq!(
        server.posts(),
        vec![(
            "POST /api/v4/projects/group%2Fproject/merge_requests/7/notes HTTP/1.1".into(),
            json!({
                "body": "summary"
            })
        )]
    );
}
