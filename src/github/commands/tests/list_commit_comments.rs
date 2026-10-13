use std::assert_matches;

use serde_json::json;

use super::*;

#[tokio::test]
async fn commit_comment_read_selects_its_endpoint_and_preserves_inaccessible_repositories() {
    let gh = FakeGh::new(vec![
        Reply::json(json!([{
            "id":4,
            "created_at":"2026-01-01T00:00:00Z",
            "user":null,
            "body":"commit comment",
            "path":"a.rs",
            "commit_id":"abc1234",
            "position":7
        }])),
        Reply::api(404, &[], "not JSON"),
    ]);
    let repository = repository();
    let comments = list_commit_comments(
        &gh.client,
        ListCommitCommentsInput {
            repository: &repository,
            commit: &commit(),
            skip_inaccessible: false,
        },
    )
    .await
    .unwrap();
    assert!(comments.repository_accessible);
    assert_eq!(comments.items.len(), 1);
    assert_eq!(comments.items[0].body, "commit comment");
    assert_eq!(comments.items[0].commit_id, commit());
    assert_eq!(comments.items[0].position, Some(7));
    let comments = list_commit_comments(
        &gh.client,
        ListCommitCommentsInput {
            repository: &repository,
            commit: &commit(),
            skip_inaccessible: true,
        },
    )
    .await
    .unwrap();
    assert!(!comments.repository_accessible);
    assert_matches!(comments.items[..], []);
    for index in 1..=2 {
        assert_eq!(
            gh.args(index)[1],
            "https://api.github.com/repos/owner/repo/commits/abc1234/comments?per_page=100"
        );
    }
}
