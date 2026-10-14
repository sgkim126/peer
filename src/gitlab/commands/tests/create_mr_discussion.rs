use std::num::NonZeroU32;

use serde_json::{Value, json};

use super::*;

fn position(kind: PositionKind) -> DiscussionPosition {
    DiscussionPosition {
        base_sha: commit(),
        start_sha: commit(),
        head_sha: commit(),
        old_path: "a.rs".into(),
        new_path: "b.rs".into(),
        kind,
    }
}

#[tokio::test]
async fn posts_text_position_and_private_body_on_stdin() {
    let server = Server::start(vec![Reply::json(json!({
        "notes": [{
            "id": 4
        }]
    }))])
    .await;
    let commit = commit();
    let position = position(PositionKind::Text {
        new_line: NonZeroU32::new(8).unwrap(),
        old_line: NonZeroU32::new(7),
    });
    let body = "private '\" $HOME $(uname)\n:fullpath";

    let result = create_mr_discussion(
        &client(&server),
        CreateMrDiscussionInput {
            merge_request: input(&repository()),
            body,
            target: Some(DiscussionTarget {
                commit: &commit,
                position: &position,
            }),
        },
    )
    .await
    .unwrap();

    assert_eq!(result.notes[0].id.get(), 4);
    let (args, stdin) = server.invocation(0);
    assert_eq!(
        args,
        [
            "api",
            "https://gitlab.com/api/v4/projects/group%2Fproject/merge_requests/7/discussions",
            "--hostname",
            "gitlab.com",
            "--method",
            "POST",
            "--include",
            "--header",
            "Accept: application/json",
            "--header",
            "Content-Type: application/json",
            "--input",
            "-",
        ]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&stdin).unwrap(),
        json!({
            "body": body,
            "commit_id": commit,
            "position": {
                "base_sha": commit,
                "start_sha": commit,
                "head_sha": commit,
                "old_path": "a.rs",
                "new_path": "b.rs",
                "new_line": 8,
                "old_line": 7,
                "position_type": "text"
            }
        })
    );
}

#[tokio::test]
async fn posts_file_position_without_line_fields() {
    let server = Server::start(vec![Reply::json(json!({
        "notes": [{
            "id": 5
        }]
    }))])
    .await;
    let commit = commit();
    let position = position(PositionKind::File);

    create_mr_discussion(
        &client(&server),
        CreateMrDiscussionInput {
            merge_request: input(&repository()),
            body: "feedback",
            target: Some(DiscussionTarget {
                commit: &commit,
                position: &position,
            }),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        serde_json::from_slice::<Value>(&server.invocation(0).1).unwrap(),
        json!({
            "body": "feedback",
            "commit_id": commit,
            "position": {
                "base_sha": commit,
                "start_sha": commit,
                "head_sha": commit,
                "old_path": "a.rs",
                "new_path": "b.rs",
                "position_type": "file"
            }
        })
    );
}

#[tokio::test]
async fn posts_conversation_discussion_without_target_fields() {
    let server = Server::start(vec![Reply::json(json!({
        "notes": [{
            "id": 3
        }]
    }))])
    .await;

    let result = create_mr_discussion(
        &client(&server),
        CreateMrDiscussionInput {
            merge_request: input(&repository()),
            body: "feedback",
            target: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(result.notes[0].id.get(), 3);
    assert_eq!(
        server.posts(),
        vec![(
            "POST /api/v4/projects/group%2Fproject/merge_requests/7/discussions HTTP/1.1".into(),
            json!({
                "body": "feedback"
            })
        )]
    );
}
