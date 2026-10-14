use std::assert_matches;

use serde_json::json;

use super::*;

#[tokio::test]
async fn fills_missing_commit_identity_in_context_discussions() {
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
    let commit = commit();

    let result = list_commit_discussions(
        &client(&server),
        ListCommitDiscussionsInput {
            project_id: 123,
            commit: &commit,
            skip_inaccessible_project: false,
        },
    )
    .await
    .unwrap();

    assert_matches!(
        result,
        Collection {
            project_accessible: true,
            ..
        }
    );
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].notes[0].commit_id.as_ref(), Some(&commit));
    assert_eq!(
        server.invocation(0).0[1],
        format!(
            "https://gitlab.com/api/v4/projects/123/repository/commits/{commit}/discussions?per_page=100"
        )
    );
}

#[tokio::test]
async fn skips_forbidden_optional_project_on_first_page() {
    let commit = commit();
    let server = Server::start(vec![Reply::failure(
        403,
        json!({
            "message": "private"
        }),
    )])
    .await;

    let result = list_commit_discussions(
        &client(&server),
        ListCommitDiscussionsInput {
            project_id: 123,
            commit: &commit,
            skip_inaccessible_project: true,
        },
    )
    .await
    .unwrap();

    assert_matches!(
        result,
        Collection {
            project_accessible: false,
            ..
        }
    );
    assert_matches!(result.items.as_slice(), []);
}

#[tokio::test]
async fn skips_missing_optional_project_on_first_page() {
    let commit = commit();
    let server = Server::start(vec![Reply::failure(
        404,
        json!({
            "message": "private"
        }),
    )])
    .await;

    let result = list_commit_discussions(
        &client(&server),
        ListCommitDiscussionsInput {
            project_id: 123,
            commit: &commit,
            skip_inaccessible_project: true,
        },
    )
    .await
    .unwrap();

    assert_matches!(
        result,
        Collection {
            project_accessible: false,
            ..
        }
    );
    assert_matches!(result.items.as_slice(), []);
}

#[tokio::test]
async fn rejects_inaccessible_optional_project_after_first_page() {
    let commit = commit();
    let server = Server::start(vec![
        Reply::json(json!([])).header(&format!(
            "Link: <https://gitlab.com/api/v4/projects/123/repository/commits/{commit}/discussions?page=2>; rel=\"next\""
        )),
        Reply::failure(403, json!({
            "message": "private"
        })),
    ])
    .await;

    assert_matches!(
        list_commit_discussions(
            &client(&server),
            ListCommitDiscussionsInput {
                project_id: 123,
                commit: &commit,
                skip_inaccessible_project: true,
            }
        )
        .await,
        Err(CommandError::Api { status: 403, .. })
    );
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn rejects_rate_limited_optional_project() {
    let commit = commit();
    let server = Server::start(vec![
        Reply::failure(403, json!({})).header("RateLimit-Remaining: 0"),
    ])
    .await;

    assert_matches!(
        list_commit_discussions(
            &client(&server),
            ListCommitDiscussionsInput {
                project_id: 123,
                commit: &commit,
                skip_inaccessible_project: true,
            }
        )
        .await,
        Err(CommandError::Api {
            status: 403,
            rate_limited: true,
            ..
        })
    );
}
