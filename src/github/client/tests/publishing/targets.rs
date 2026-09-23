use super::*;

fn target_replies(commits: &[&str]) -> Vec<Reply> {
    let mut pull: Value = serde_json::from_str(&pull().body).unwrap();
    pull["head"]["sha"] = json!(commits.last().unwrap());
    pull["commits"] = json!(commits.len());
    let mut replies = vec![
        Reply::json(pull.clone()),
        Reply::json(json!(
            commits
                .iter()
                .map(|sha| json!({"sha": sha}))
                .collect::<Vec<_>>()
        )),
        Reply::json(json!([])),
        Reply::json(json!([])),
    ];
    replies.extend(commits.iter().map(|_| Reply::json(json!([]))));
    replies.extend([
        Reply::json(json!([{
            "filename": "src/main.rs",
            "patch": "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context"
        }])),
        Reply::json(pull),
    ]);
    replies
}

#[tokio::test]
async fn historical_line_location_is_not_retargeted_to_the_pull_request_head() {
    let mut replies = target_replies(&["abc1234", "def5678"]);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    let requests = server.requests();
    let post = requests.last().unwrap();
    assert!(post.starts_with("POST /repos/owner/repo/issues/123/comments "));
    assert!(request_body(post).get("commit_id").is_none());
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        1
    );
}

#[tokio::test]
async fn foreign_commit_uses_a_conversation_comment() {
    let mut input = finding();
    input.findings[0].commit = CommitHash::new("fedcba9").unwrap();
    let mut replies = target_replies(&["abc1234"]);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}

#[tokio::test]
async fn ambiguous_commit_prefix_uses_a_conversation_comment() {
    let mut replies = target_replies(&["abc12341", "abc12342"]);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &finding())
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}

#[tokio::test]
async fn unique_head_prefix_uses_the_complete_api_commit_id() {
    let mut replies = target_replies(&["def5678", "abc1234567890"]);
    replies.push(created());
    let input = finding();
    let fingerprint = crate::github::feedback::PreparedReview::new(&input, &repository()).items[0]
        .fingerprint
        .clone();
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    let requests = server.requests();
    let post = requests.last().unwrap();
    assert!(post.starts_with("POST /repos/owner/repo/pulls/123/comments "));
    let params = request_body(post);
    assert_eq!(params["commit_id"], "abc1234567890");
    assert!(
        crate::github::feedback::fingerprints(params["body"].as_str().unwrap())
            .contains(&fingerprint)
    );
}

#[tokio::test]
async fn explicit_question_commit_takes_precedence_over_related_head() {
    let input = serde_json::from_value(json!({
        "ordered_commits": ["abc1234", "def5678"], "stages": [],
        "questions": [{
            "category": "rationale",
            "question": "Why?",
            "evidence": "Evidence",
            "why_it_matters": "Reason",
            "related_commits": ["def5678"],
            "location": {
                "commit": "abc1234",
                "file": "src/main.rs",
                "line": 5
            }
        }]
    }))
    .unwrap();
    let mut replies = target_replies(&["abc1234", "def5678"]);
    replies.push(created());
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert!(
        server
            .requests()
            .last()
            .unwrap()
            .starts_with("POST /repos/owner/repo/issues/123/comments ")
    );
}
