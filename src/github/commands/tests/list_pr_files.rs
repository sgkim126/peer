use serde_json::json;

use super::*;

#[tokio::test]
async fn pull_request_file_read_selects_its_typed_endpoint() {
    let gh = FakeGh::new(vec![Reply::json(json!([{
        "filename":"a.rs",
        "previous_filename":"old.rs",
        "patch":"@@ -0,0 +1 @@\n+a"
    }]))]);
    assert_eq!(
        list_pr_files(&gh.client, input(&repository()))
            .await
            .unwrap()[0]
            .previous_filename
            .as_deref(),
        Some("old.rs")
    );
    assert_eq!(
        gh.args(1)[1],
        "https://api.github.com/repos/owner/repo/pulls/123/files?per_page=100"
    );
}
