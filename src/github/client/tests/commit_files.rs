use super::*;

#[tokio::test]
async fn paginates_commit_files() {
    let server = Server::start(vec![
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
            "patch": "@@ -10 +10 @@\n-old\n+new",
        }]}))
        .header(concat!(
            "Link: <{base}repos/contributor/fork/commits/def5678?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        Reply::json(json!({"files": [{
            "filename": "src/other.rs",
            "status": "added",
            "patch": "@@ -0,0 +1 @@\n+added",
        }]})),
    ])
    .await;

    let files = server
        .client()
        .commit_files(
            &Repository::parse("contributor/fork").unwrap(),
            &CommitHash::new("def5678").unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(files.len(), 2);
    assert_eq!(files[0].filename, "src/main.rs");
    assert_eq!(files[0].patch.as_deref(), Some("@@ -10 +10 @@\n-old\n+new"));
    assert_eq!(files[1].filename, "src/other.rs");
    assert_eq!(files[1].patch.as_deref(), Some("@@ -0,0 +1 @@\n+added"));
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /repos/contributor/fork/commits/def5678?per_page=100 "));
    assert!(
        requests[1].starts_with("GET /repos/contributor/fork/commits/def5678?per_page=100&page=2 ")
    );
}

#[tokio::test]
async fn rejects_commit_files_when_a_later_page_fails() {
    let mut failure = Reply::json(json!({}));
    failure.status = 500;
    let server = Server::start(vec![
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
            "patch": "@@ -10 +10 @@\n-old\n+new",
        }]}))
        .header(concat!(
            "Link: <{base}repos/owner/repo/commits/abc1234?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        failure,
    ])
    .await;

    assert_matches!(
        server
            .client()
            .commit_files(&repository(), &CommitHash::new("abc1234").unwrap())
            .await,
        Err(GitHubError::Api { status: 500, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].starts_with("GET /repos/owner/repo/commits/abc1234?per_page=100&page=2 "));
}
