use std::assert_matches;

use serde_json::json;

use super::*;

#[tokio::test]
async fn lists_renamed_commit_files() {
    let server = Server::start(vec![Reply::json(json!([{
        "old_path": "old.rs",
        "new_path": "new.rs",
        "renamed_file": true
    }]))])
    .await;
    let commit = commit();

    let files = list_commit_diff(
        &client(&server),
        CommitInput {
            repository: &repository(),
            commit: &commit,
        },
    )
    .await
    .unwrap();

    assert_matches!(
        files.as_slice(),
        [ChangedFile {
            renamed_file: true,
            ..
        }]
    );
    assert_eq!(files[0].old_path, "old.rs");
    assert_eq!(files[0].new_path, "new.rs");
    assert_eq!(
        server.invocation(0).0[1],
        format!(
            "https://gitlab.com/api/v4/projects/group%2Fproject/repository/commits/{commit}/diff?per_page=100"
        )
    );
}
