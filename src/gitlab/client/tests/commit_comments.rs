use super::*;

fn discussion(id: &str, notes: Vec<Value>) -> Reply {
    Reply::json(json!([{"id": id, "notes": notes}]))
}

#[tokio::test]
async fn loads_commit_discussions_and_replies_from_every_commit_and_project_page() {
    let mut root = note(1);
    root["commit_id"] = Value::Null;
    let mut explicit_commit = note(3);
    explicit_commit["commit_id"] = json!("abc1234");
    let mut source_reply = note(5);
    source_reply["commit_id"] = Value::Null;
    let server = Server::start(vec![
        Reply::json(merge_request()),
        Reply::json(json!([{"id": "abc1234"}, {"id": "2345678"}])),
        discussion("mr", vec![note(100)]),
        discussion("target-first", vec![note(2), root]).header(concat!(
            "Link: <{base}projects/5/repository/commits/abc1234/discussions?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        discussion("target-next-page", vec![explicit_commit]),
        discussion("source-first", vec![note(4)]).header(concat!(
            "Link: <{base}projects/9/repository/commits/abc1234/discussions?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        discussion("source-first", vec![source_reply]),
        discussion("target-second-commit", vec![note(6)]),
        discussion("source-second-commit", vec![note(7)]),
        Reply::json(merge_request()),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 6);
    for (body, commit, replies) in [
        ("Comment 100", None, vec!["Comment 100"]),
        ("Comment 1", Some("abc1234"), vec!["Comment 1", "Comment 2"]),
        ("Comment 3", Some("abc1234"), vec!["Comment 3"]),
        ("Comment 4", Some("abc1234"), vec!["Comment 4", "Comment 5"]),
        ("Comment 6", Some("2345678"), vec!["Comment 6"]),
        ("Comment 7", Some("2345678"), vec!["Comment 7"]),
    ] {
        let thread = input
            .context
            .comments
            .iter()
            .find(|thread| thread.comments[0].body == body)
            .unwrap();
        assert_eq!(thread.commit.as_ref().map(CommitHash::as_ref), commit);
        assert_eq!(
            thread
                .comments
                .iter()
                .map(|comment| comment.body.as_str())
                .collect::<Vec<_>>(),
            replies
        );
        assert!(
            thread
                .comments
                .iter()
                .all(|comment| comment.author == "alice")
        );
    }

    let requests = server.requests();
    assert_eq!(requests.len(), 10);
    for (index, path) in [
        (
            3,
            "projects/5/repository/commits/abc1234/discussions?per_page=100",
        ),
        (
            4,
            "projects/5/repository/commits/abc1234/discussions?per_page=100&page=2",
        ),
        (
            5,
            "projects/9/repository/commits/abc1234/discussions?per_page=100",
        ),
        (
            6,
            "projects/9/repository/commits/abc1234/discussions?per_page=100&page=2",
        ),
        (
            7,
            "projects/5/repository/commits/2345678/discussions?per_page=100",
        ),
        (
            8,
            "projects/9/repository/commits/2345678/discussions?per_page=100",
        ),
        (9, "projects/group%2Fsubgroup%2Fproject/merge_requests/123"),
    ] {
        assert!(requests[index].starts_with(&format!("GET /api/v4/{path} ")));
    }
}

#[tokio::test]
async fn loads_a_same_project_commit_only_once() {
    let mut merge_request = merge_request();
    merge_request["source_project_id"] = json!(5);
    let server = Server::start(vec![
        Reply::json(merge_request.clone()),
        commits(),
        discussions(),
        discussion("native", vec![note(1)]),
        Reply::json(merge_request),
    ])
    .await;

    let input = server
        .client()
        .review_input(&repository(), number())
        .await
        .unwrap();

    assert_eq!(input.context.comments.len(), 1);
    assert_eq!(input.context.comments[0].comments[0].body, "Comment 1");
    assert_eq!(
        input.context.comments[0]
            .commit
            .as_ref()
            .map(CommitHash::as_ref),
        Some("abc1234")
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 5);
    assert!(requests[3].starts_with(
        "GET /api/v4/projects/5/repository/commits/abc1234/discussions?per_page=100 "
    ));
    assert!(
        requests[4]
            .starts_with("GET /api/v4/projects/group%2Fsubgroup%2Fproject/merge_requests/123 ")
    );
}

#[tokio::test]
async fn a_later_source_commit_discussion_page_failure_discards_the_input() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        commits(),
        discussion("mr", vec![note(100)]),
        discussion("target", vec![note(1)]),
        discussion("source", vec![note(2)]).header(concat!(
            "Link: <{base}projects/9/repository/commits/abc1234/discussions?",
            "per_page=100&page=2>; rel=\"next\"",
        )),
        Reply::json(json!({})).status(503),
    ])
    .await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 503, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 6);
    assert!(requests[5].starts_with(concat!(
        "GET /api/v4/projects/9/repository/commits/abc1234/discussions?",
        "per_page=100&page=2 "
    )));
}

#[tokio::test]
async fn a_later_source_commit_discussion_failure_discards_the_input() {
    let server = Server::start(vec![
        Reply::json(merge_request()),
        Reply::json(json!([{"id": "abc1234"}, {"id": "2345678"}])),
        discussion("mr", vec![note(100)]),
        discussion("target-first", vec![note(1)]),
        discussion("source-first", vec![note(2)]),
        discussion("target-second", vec![note(3)]),
        Reply::json(json!({})).status(404),
    ])
    .await;

    assert_matches!(
        server.client().review_input(&repository(), number()).await,
        Err(GitLabError::Api { status: 404, .. })
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 7);
    assert!(requests[6].starts_with(
        "GET /api/v4/projects/9/repository/commits/2345678/discussions?per_page=100 "
    ));
}
