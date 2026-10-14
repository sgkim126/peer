use std::assert_matches;

use serde_json::json;

use super::*;

#[tokio::test]
async fn reads_merge_request_feedback_without_context_fields() {
    let server = Server::start(vec![Reply::json(json!([{
        "notes": [{
            "id": 3,
            "body": "marker",
            "internal": true
        }]
    }]))])
    .await;

    let feedback = list_mr_feedback(&client(&server), input(&repository()))
        .await
        .unwrap();

    let note = &feedback[0].notes[0];
    assert_eq!(note.id.get(), 3);
    assert_eq!(note.body, "marker");
    assert_matches!(
        note,
        FeedbackNote {
            internal: true,
            confidential: false,
            system: false,
            ..
        }
    );
    assert_eq!(
        server.invocation(0).0[1],
        "https://gitlab.com/api/v4/projects/group%2Fproject/merge_requests/7/discussions?per_page=100"
    );
}
