use serde_json::json;

use super::*;

#[tokio::test]
async fn commit_file_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!({
        "files":[{
            "filename":"a.rs"
        }]
    }))]);
    let files = get_commit_files(
        &gh.client,
        CommitInput {
            repository: &repository(),
            commit: &commit(),
        },
    )
    .await
    .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "a.rs");
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/commits/abc1234?per_page=100"
    );
}
