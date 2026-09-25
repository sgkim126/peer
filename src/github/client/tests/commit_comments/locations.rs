use super::*;

#[tokio::test]
async fn resolves_paginated_comments_using_one_commit_patch() {
    let mut added = commit_comment(1, "abc1234");
    added["position"] = json!(3);
    added["line"] = json!(999);
    let mut context = commit_comment(2, "abc1234");
    context["position"] = json!(5);
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([added])).header(concat!(
            "Link: <{base}repos/owner/repo/commits/abc1234/comments?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        Reply::json(json!([context])),
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
            "patch": "@@ -10,3 +10,4 @@\n before\n-old\n+new\n+extra\n after",
        }]})),
        pull(),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    let threads = input.context.comments;
    assert_eq!(threads.len(), 2);
    let added = threads[0].location.as_ref().unwrap();
    assert_eq!(added.path, "src/main.rs");
    assert_eq!(added.line.map(NonZeroU32::get), Some(11));
    assert_eq!(threads[0].commit.as_ref().unwrap().as_ref(), "abc1234");
    let context = threads[1].location.as_ref().unwrap();
    assert_eq!(context.path, "src/main.rs");
    assert_eq!(context.line.map(NonZeroU32::get), Some(13));

    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    assert!(requests[6].starts_with(concat!(
        "GET /repos/owner/repo/commits/abc1234/comments?",
        "per_page=100&page=2 "
    )));
    assert!(requests[7].starts_with("GET /repos/owner/repo/commits/abc1234?per_page=100 "));
    assert!(requests[8].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn resolves_target_and_source_comments_using_their_respective_repository_patches() {
    let pull = pull_with_source(Some("contributor/fork"), "abc1234", 1);
    let mut target = commit_comment(1, "abc1234");
    target["position"] = json!(2);
    let mut source = commit_comment(2, "abc1234");
    source["position"] = json!(2);
    let server = Server::start(vec![
        Reply::json(pull.clone()),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([target])),
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
            "patch": "@@ -10 +10 @@\n-old\n+target",
        }]})),
        Reply::json(json!([source])),
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
            "patch": "@@ -20 +20 @@\n-old\n+source",
        }]})),
        Reply::json(pull),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    let threads = input.context.comments;
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].comments[0].body, "Commit comment 1");
    assert_eq!(
        threads[0]
            .location
            .as_ref()
            .unwrap()
            .line
            .map(NonZeroU32::get),
        Some(10)
    );
    assert_eq!(threads[1].comments[0].body, "Commit comment 2");
    assert_eq!(
        threads[1]
            .location
            .as_ref()
            .unwrap()
            .line
            .map(NonZeroU32::get),
        Some(20)
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 10);
    assert!(requests[6].starts_with("GET /repos/owner/repo/commits/abc1234?per_page=100 "));
    assert!(requests[8].starts_with("GET /repos/contributor/fork/commits/abc1234?per_page=100 "));
}

#[tokio::test]
async fn retains_only_the_previous_path_for_comments_on_deleted_lines_after_a_rename() {
    let mut comment = commit_comment(1, "abc1234");
    comment["position"] = json!(1);
    comment["line"] = json!(10);
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([comment])),
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "previous_filename": "src/old.rs",
            "status": "renamed",
            "patch": "@@ -10 +10 @@\n-old\n+new",
        }]})),
        pull(),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    let thread = &input.context.comments[0];
    assert_eq!(thread.comments[0].body, "Commit comment 1");
    assert_eq!(thread.commit, None);
    let location = thread.location.as_ref().unwrap();
    assert_eq!(location.path, "src/old.rs");
    assert_eq!(location.line, None);
}

#[tokio::test]
async fn retains_comment_and_path_when_the_file_has_no_patch() {
    let mut comment = commit_comment(1, "abc1234");
    comment["position"] = json!(2);
    comment["line"] = json!(10);
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([comment])),
        Reply::json(json!({"files": [{
            "filename": "src/main.rs",
            "status": "modified",
        }]})),
        pull(),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    let thread = &input.context.comments[0];
    assert_eq!(thread.comments[0].body, "Commit comment 1");
    assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "abc1234");
    let location = thread.location.as_ref().unwrap();
    assert_eq!(location.path, "src/main.rs");
    assert_eq!(location.line, None);
    assert_eq!(server.requests().len(), 8);
}

#[tokio::test]
async fn preserves_comments_and_paths_when_commit_files_are_unavailable() {
    let mut comment = commit_comment(1, "abc1234");
    comment["position"] = json!(2);
    let mut failure = Reply::json(json!({}));
    failure.status = 500;
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([comment])),
        failure,
        pull(),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    let thread = &input.context.comments[0];
    assert_eq!(thread.comments[0].body, "Commit comment 1");
    assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "abc1234");
    let location = thread.location.as_ref().unwrap();
    assert_eq!(location.path, "src/main.rs");
    assert_eq!(location.line, None);
    let requests = server.requests();
    assert_eq!(requests.len(), 8);
    assert!(requests[6].starts_with("GET /repos/owner/repo/commits/abc1234?per_page=100 "));
    assert!(requests[7].starts_with("GET /repos/owner/repo/pulls/123 "));
}

#[tokio::test]
async fn does_not_load_a_patch_for_a_comment_from_a_different_commit() {
    let mut comment = commit_comment(1, "def5678");
    comment["position"] = json!(2);
    comment["line"] = json!(10);
    let server = Server::start(vec![
        pull(),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([])),
        Reply::json(json!([{"sha": "abc1234"}])),
        Reply::json(json!([comment])),
        pull(),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    let thread = &input.context.comments[0];
    assert_eq!(thread.comments[0].body, "Commit comment 1");
    assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "def5678");
    let location = thread.location.as_ref().unwrap();
    assert_eq!(location.path, "src/main.rs");
    assert_eq!(location.line, None);
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[6].starts_with("GET /repos/owner/repo/pulls/123 "));
}
