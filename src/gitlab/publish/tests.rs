use super::*;

use std::assert_matches;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OLD: &str = "dddddddddddddddddddddddddddddddddddddddd";

struct Reply {
    status: u16,
    body: String,
    headers: Vec<String>,
    disconnect: bool,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: 200,
            body: body.to_string(),
            headers: Vec::new(),
            disconnect: false,
        }
    }

    fn failure(status: u16, body: Value) -> Self {
        Self {
            status,
            ..Self::json(body)
        }
    }
}

struct Server {
    base: Url,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let address = base.to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let header_end = request
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let content_length = String::from_utf8_lossy(&request[..header_end])
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                if reply.disconnect {
                    continue;
                }
                let headers = reply
                    .headers
                    .iter()
                    .map(|header| format!("{}\r\n", header.replace("{base}", &address)))
                    .collect::<String>();
                let response = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status,
                    reply.body.len(),
                    headers,
                    reply.body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            base,
            requests,
            task,
        }
    }

    fn client(&self) -> GitLabClient {
        GitLabClient::new("test-token", self.base.clone(), Duration::from_secs(2)).unwrap()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn posts(&self) -> Vec<(String, Value)> {
        self.requests()
            .into_iter()
            .filter(|request| request.starts_with("POST "))
            .map(|request| {
                let (headers, body) = request.split_once("\r\n\r\n").unwrap();
                (
                    headers.lines().next().unwrap().to_string(),
                    serde_json::from_str(body).unwrap(),
                )
            })
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn repository() -> Repository {
    Repository::parse("group/subgroup/project").unwrap()
}

fn number() -> NonZeroU64 {
    NonZeroU64::new(42).unwrap()
}

fn source() -> Value {
    json!({
        "provider": "gitlab", "project_id": 123, "iid": 42, "source_project_id": 456,
        "diff_refs": {
            "base_sha": "a".repeat(40), "head_sha": HEAD, "start_sha": "c".repeat(40),
        },
    })
}

fn merge_request() -> Reply {
    Reply::json(json!({
        "title": "Review", "description": "Description", "project_id": 123, "iid": 42,
        "source_project_id": 456, "target_project_id": 123,
        "source_branch": "feature", "target_branch": "main", "sha": HEAD,
        "diff_refs": source()["diff_refs"],
    }))
}

fn input(messages: &[&str]) -> RenderDocument {
    serde_json::from_value(json!({
        "source": source(), "ordered_commits": [OLD, HEAD], "stages": [],
        "findings": messages.iter().map(|message| json!({
            "commit": HEAD, "severity": "high", "message": message,
            "file": "src/main.rs", "line": 5,
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

fn question_input() -> RenderDocument {
    let mut input = input(&[]);
    input.questions = serde_json::from_value(json!([{
        "category": "rationale",
        "question": "Why?",
        "evidence": "Evidence",
        "why_it_matters": "Reason",
        "related_commits": [HEAD],
        "location": { "commit": HEAD, "file": "src/main.rs", "line": 5 },
    }]))
    .unwrap();
    input
}

fn commits() -> Reply {
    Reply::json(json!([{ "id": HEAD }, { "id": OLD }]))
}

fn empty_discussions() -> Reply {
    Reply::json(json!([]))
}

fn diffs() -> Reply {
    Reply::json(json!([{
        "old_path": "src/main.rs", "new_path": "src/main.rs",
        "diff": "@@ -5 +5 @@\n-old\n+new",
    }]))
}

fn before_inline() -> Vec<Reply> {
    vec![
        merge_request(),
        commits(),
        empty_discussions(),
        diffs(),
        merge_request(),
    ]
}

fn created(id: u64) -> Reply {
    Reply::json(json!({ "id": "discussion", "notes": [{ "id": id }] }))
}

fn known(body: &str, id: u64) -> Reply {
    Reply::json(json!([{
        "id": "discussion", "individual_note": false,
        "notes": [{ "id": id, "body": body, "resolved": true }],
    }]))
}

#[tokio::test]
async fn publishes_head_findings_as_discussions_with_gitlab_positions() {
    let mut replies = before_inline();
    replies.extend([merge_request(), created(17)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(
        report.urls,
        ["https://gitlab.com/group/subgroup/project/-/merge_requests/42#note_17"]
    );
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(
        posts[0]
            .0
            .contains("/projects/group%2Fsubgroup%2Fproject/merge_requests/42/discussions ")
    );
    assert_eq!(posts[0].1["position"]["base_sha"], "a".repeat(40));
    assert_eq!(posts[0].1["position"]["start_sha"], "c".repeat(40));
    assert_eq!(posts[0].1["position"]["head_sha"], HEAD);
    assert_eq!(posts[0].1["position"]["new_line"], 5);
    assert!(posts[0].1["position"].get("old_line").is_none());
}

#[tokio::test]
async fn publishes_head_questions_regardless_of_related_commits() {
    for related_commits in [&[HEAD][..], &[OLD, HEAD], &[], &[OLD]] {
        let mut input = question_input();
        input.questions[0].related_commits = related_commits
            .iter()
            .map(|commit| CommitHash::new(commit).unwrap())
            .collect();
        let mut replies = before_inline();
        replies.extend([merge_request(), created(17)]);
        let server = Server::start(replies).await;
        let report = server
            .client()
            .publish(&repository(), number(), &input)
            .await
            .unwrap();
        assert_eq!(report.published, 1, "{related_commits:?}");
        assert_eq!(report.inline, 1, "{related_commits:?}");
        let posts = server.posts();
        assert_eq!(posts.len(), 1);
        assert!(posts[0].0.contains("/discussions "));
        assert_eq!(posts[0].1["position"]["head_sha"], HEAD);
        assert_eq!(posts[0].1["position"]["new_line"], 5);
        assert!(posts[0].1["body"].as_str().unwrap().contains("Why?"));
    }
}

#[tokio::test]
async fn unlocated_questions_fall_back_to_independent_discussions() {
    let mut input = question_input();
    input.questions[0].location = None;
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        created(19),
    ])
    .await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].0.contains("/merge_requests/42/discussions "));
    assert!(posts[0].1.get("position").is_none());
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(!body.contains(CONVERSATION_MARKER));
    assert!(body.contains("Why?"));
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.contains("/diffs"))
    );
}

#[tokio::test]
async fn questions_at_earlier_commits_fall_back_to_independent_discussions() {
    let mut input = question_input();
    input.questions[0].location.as_mut().unwrap().commit = CommitHash::new(OLD).unwrap();
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        created(19),
    ])
    .await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].0.contains("/merge_requests/42/discussions "));
    assert!(posts[0].1.get("position").is_none());
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(!body.contains(CONVERSATION_MARKER));
    assert!(body.contains("Why?"));
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.contains("/diffs"))
    );
}

#[tokio::test]
async fn questions_in_unchanged_files_fall_back_to_independent_discussions() {
    let mut input = question_input();
    input.questions[0].location.as_mut().unwrap().file.file = "src/unchanged.rs".into();
    let mut replies = before_inline();
    replies.extend([merge_request(), created(19)]);
    let server = Server::start(replies).await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].0.contains("/merge_requests/42/discussions "));
    assert!(posts[0].1.get("position").is_none());
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(!body.contains(CONVERSATION_MARKER));
    assert!(body.contains("Why?"));
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.contains("/diffs"))
    );
}

#[tokio::test]
async fn questions_outside_diff_hunks_fall_back_to_independent_discussions() {
    let mut input = question_input();
    input.questions[0].location.as_mut().unwrap().file.line = Some(6);
    let mut replies = before_inline();
    replies.extend([merge_request(), created(19)]);
    let server = Server::start(replies).await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();

    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].0.contains("/merge_requests/42/discussions "));
    assert!(posts[0].1.get("position").is_none());
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(!body.contains(CONVERSATION_MARKER));
    assert!(body.contains("Why?"));
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.contains("/diffs"))
    );
}

#[tokio::test]
async fn publishes_one_summary_and_deduplicates_repeated_input_findings() {
    let mut input = input(&["Repeated issue", "Repeated issue"]);
    input.summary = Some(crate::review::ReviewSummary {
        peer_version: "0.16.1".into(),
    });
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        created(17),
        merge_request(),
        Reply::json(json!({ "id": 18 })),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 2);
    assert_eq!(report.inline, 1);
    assert_eq!(report.skipped, 1);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(posts[1].0.contains("/notes "));
    let summary = posts[1].1["body"].as_str().unwrap();
    assert!(summary.contains(CONVERSATION_MARKER));
    assert_eq!(fingerprints(summary).len(), 1);
    assert!(!summary.contains("Repeated issue"));
}

#[tokio::test]
async fn legacy_input_can_use_unambiguous_abbreviated_head_hashes() {
    let mut input = input(&["Issue"]);
    input.source = None;
    input.findings[0].commit = CommitHash::new(&HEAD[..7]).unwrap();
    input.ordered_commits[1] = CommitHash::new(&HEAD[..7]).unwrap();
    let mut replies = before_inline();
    replies.extend([merge_request(), created(17)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 1);
    assert_eq!(server.posts()[0].1["position"]["head_sha"], HEAD);
}

#[tokio::test]
async fn earlier_commit_locations_fall_back_to_independent_discussions() {
    let mut input = input(&["Old issue"]);
    input.findings[0].commit = CommitHash::new(OLD).unwrap();
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        created(19),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 1);
    assert!(posts[0].0.contains("/merge_requests/42/discussions "));
    assert!(
        !posts[0].1["body"]
            .as_str()
            .unwrap()
            .contains(CONVERSATION_MARKER)
    );
    assert!(
        !server
            .requests()
            .iter()
            .any(|request| request.contains("/diffs"))
    );
}

#[tokio::test]
async fn resolved_notes_and_input_duplicates_are_skipped() {
    let input = input(&["Old issue", "Old issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut first_page = empty_discussions();
    first_page.headers.push(
        "Link: <{base}projects/group%2Fsubgroup%2Fproject/merge_requests/42/discussions?per_page=100&page=2>; rel=\"next\"".into(),
    );
    let server = Server::start(vec![
        merge_request(),
        commits(),
        first_page,
        known(&inline_body(&review.items[0]), 17),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 2);
    assert_eq!(report.published, 0);
    assert_eq!(server.posts(), vec![]);
    assert!(server.requests()[3].contains("page=2"));
}

#[tokio::test]
async fn internal_notes_do_not_suppress_public_feedback() {
    let input = input(&["Issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies[2] = Reply::json(json!([{ "notes": [{
        "id": 1, "body": inline_body(&review.items[0]), "internal": true,
    }] }]));
    replies.extend([merge_request(), created(17)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.skipped, 0);
    assert_eq!(report.published, 1);
}

#[tokio::test]
async fn rejects_mismatched_base_sha_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["diff_refs"]["base_sha"] = json!("e".repeat(40));
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_mismatched_head_sha_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["diff_refs"]["head_sha"] = json!("e".repeat(40));
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_mismatched_start_sha_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["diff_refs"]["start_sha"] = json!("e".repeat(40));
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_mismatched_project_ids_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["project_id"] = json!(999);
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_mismatched_merge_request_numbers_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["iid"] = json!(999);
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_mismatched_source_project_ids_before_writes() {
    let mut input = input(&["Issue"]);
    let mut stale = source();
    stale["source_project_id"] = json!(999);
    input.source = Some(serde_json::from_value(stale).unwrap());
    let server = Server::start(vec![merge_request()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_legacy_input_when_ordered_commits_do_not_end_at_head() {
    let mut input = input(&["Issue"]);
    input.source = None;
    input.ordered_commits = vec![CommitHash::new(OLD).unwrap()];
    let server = Server::start(vec![merge_request(), commits()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::InvalidReviewCommits);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_legacy_findings_outside_the_merge_request() {
    let mut input = input(&["Issue"]);
    input.source = None;
    input.findings[0].commit = CommitHash::new(&"e".repeat(40)).unwrap();
    let server = Server::start(vec![merge_request(), commits()]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();

    assert_matches!(error.reason.as_ref(), PublishFailure::InvalidReviewCommits);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_empty_remote_commit_lists_before_writes() {
    let server = Server::start(vec![merge_request(), Reply::json(json!([]))]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();

    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::IncompleteCommits)
    );
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_remote_commit_lists_without_head_before_writes() {
    let server = Server::start(vec![merge_request(), Reply::json(json!([{ "id": OLD }]))]).await;

    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();

    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::IncompleteCommits)
    );
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn rejects_duplicate_remote_commits_before_writes() {
    let server = Server::start(vec![
        merge_request(),
        Reply::json(json!([{ "id": OLD }, { "id": HEAD }, { "id": HEAD }])),
    ])
    .await;

    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();

    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::IncompleteCommits)
    );
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn detects_merge_request_changes_after_loading_diffs() {
    let mut replies = before_inline();
    let mut changed: Value = serde_json::from_str(&replies[4].body).unwrap();
    changed["diff_refs"]["base_sha"] = json!("e".repeat(40));
    replies[4] = Reply::json(changed);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(error.reason.as_ref(), PublishFailure::SnapshotMismatch);
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn reports_successful_comments_when_the_merge_request_changes_mid_batch() {
    let mut replies = before_inline();
    let mut changed: Value = serde_json::from_str(&merge_request().body).unwrap();
    changed["target_branch"] = json!("other-target");
    replies.extend([merge_request(), created(17), Reply::json(changed)]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["First", "Second"]))
        .await
        .unwrap_err();
    assert_eq!(error.report.published, 1);
    assert_eq!(error.report.inline, 1);
    assert!(error.to_string().contains("#note_17"));
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn rerunning_a_partial_publication_posts_only_the_remaining_feedback() {
    let input = input(&["First", "Second"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        created(17),
        merge_request(),
        Reply::failure(403, json!({ "message": "forbidden" })),
    ]);
    let mut retry = before_inline();
    retry[2] = known(&inline_body(&review.items[0]), 17);
    retry.extend([merge_request(), created(18)]);
    replies.extend(retry);
    let server = Server::start(replies).await;
    let client = server.client();

    let error = client
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();
    assert_eq!(error.report.published, 1);
    assert_eq!(error.report.urls.len(), 1);
    assert!(error.report.urls[0].ends_with("#note_17"));

    let report = client
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(report.skipped, 1);
    assert!(report.urls[0].ends_with("#note_18"));

    let posts = server.posts();
    assert_eq!(posts.len(), 3);
    assert!(posts[0].1["body"].as_str().unwrap().contains("First"));
    assert!(posts[1].1["body"].as_str().unwrap().contains("Second"));
    assert_eq!(posts[1].1, posts[2].1);
}

#[tokio::test]
async fn explicit_position_errors_fall_back_to_independent_discussions() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(400, json!({ "message": { "position": ["is invalid"] } })),
        merge_request(),
        created(18),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(posts[1].0.contains("/merge_requests/42/discussions "));
}

#[tokio::test]
async fn bad_request_commit_validation_failures_fall_back_to_unpositioned_mr_threads() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(400, json!({ "message": { "commit_id": ["is invalid"] } })),
        merge_request(),
        created(20),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(
        posts
            .iter()
            .all(|(path, _)| path.contains("/merge_requests/42/discussions "))
    );
    assert!(posts[0].1.get("position").is_some());
    assert!(posts[1].1.get("position").is_none());
    assert!(posts[1].1.get("commit_id").is_none());
    assert_eq!(posts[0].1["body"], posts[1].1["body"]);
}

#[tokio::test]
async fn unprocessable_entity_commit_validation_failures_fall_back_to_unpositioned_mr_threads() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(422, json!({ "message": { "commit_id": ["is invalid"] } })),
        merge_request(),
        created(20),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(
        posts
            .iter()
            .all(|(path, _)| path.contains("/merge_requests/42/discussions "))
    );
    assert!(posts[0].1.get("position").is_some());
    assert!(posts[1].1.get("position").is_none());
    assert!(posts[1].1.get("commit_id").is_none());
    assert_eq!(posts[0].1["body"], posts[1].1["body"]);
}

#[tokio::test]
async fn bad_requests_without_position_errors_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(400, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api {
            status: 400,
            position_invalid: false,
            ..
        })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn unauthorized_inline_posts_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(401, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api { status: 401, .. })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn forbidden_inline_posts_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(403, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api { status: 403, .. })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn not_found_inline_posts_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(404, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api { status: 404, .. })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn unprocessable_inline_posts_without_position_errors_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(422, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api {
            status: 422,
            position_invalid: false,
            ..
        })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn rate_limited_inline_posts_stop_without_fallback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(429, json!({ "message": "rejected" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Api(GitLabError::Api { status: 429, .. })
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn unavailable_diff_falls_back_but_diff_authentication_errors_stop() {
    let mut replies = before_inline();
    replies[3] = Reply::json(json!([{
        "old_path": "src/main.rs", "new_path": "src/main.rs", "too_large": true,
    }]));
    replies.extend([merge_request(), created(19)]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input(&["Issue"]))
        .await
        .unwrap();
    assert_eq!(report.inline, 0);
    assert!(
        server.posts()[0]
            .0
            .contains("/merge_requests/42/discussions ")
    );

    let mut replies = before_inline();
    replies.truncate(4);
    replies[3] = Reply::failure(403, json!({ "message": "forbidden" }));
    let server = Server::start(replies).await;
    assert_matches!(
        server
            .client()
            .publish(&repository(), number(), &input(&["Issue"]))
            .await,
        Err(_)
    );
    assert_eq!(server.posts(), vec![]);
}

#[tokio::test]
async fn confirms_inline_posts_after_server_errors_without_fallback() {
    let input = input(&["Issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(500, json!({ "message": "failed" })),
        known(&inline_body(&review.items[0]), 17),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn confirms_inline_posts_after_undecodable_responses_without_fallback() {
    let input = input(&["Issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::json(json!({})),
        known(&inline_body(&review.items[0]), 17),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn confirms_inline_posts_after_disconnects_without_fallback() {
    let input = input(&["Issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply {
            disconnect: true,
            ..Reply::json(json!({}))
        },
        known(&inline_body(&review.items[0]), 17),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn confirms_inline_posts_with_empty_note_lists_without_fallback() {
    let input = input(&["Issue"]);
    let review = PreparedReview::for_gitlab(&input, &repository());
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::json(json!({ "notes": [] })),
        known(&inline_body(&review.items[0]), 17),
    ]);
    let server = Server::start(replies).await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 1);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn missing_confirmation_stops_before_remaining_feedback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(503, json!({})),
        empty_discussions(),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["First", "Second"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Unconfirmed {
            request: Some(GitLabError::Api { status: 503, .. }),
            verification: None,
        }
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn forbidden_confirmation_stops_before_remaining_feedback() {
    let mut replies = before_inline();
    replies.extend([
        merge_request(),
        Reply::failure(503, json!({})),
        Reply::failure(403, json!({ "message": "forbidden" })),
    ]);
    let server = Server::start(replies).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input(&["First", "Second"]))
        .await
        .unwrap_err();
    assert_matches!(
        error.reason.as_ref(),
        PublishFailure::Unconfirmed {
            request: Some(GitLabError::Api { status: 503, .. }),
            verification: Some(GitLabError::Api { status: 403, .. }),
        }
    );
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn uncertain_fallback_posts_are_confirmed_separately_before_remaining_feedback() {
    let mut input = input(&["First", "Second"]);
    for finding in &mut input.findings {
        finding.location = None;
    }
    let review = PreparedReview::for_gitlab(&input, &repository());
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        Reply::failure(500, json!({})),
        known(&inline_body(&review.items[0]), 19),
        merge_request(),
        created(20),
    ])
    .await;
    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 2);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 0);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(posts[0].1["body"].as_str().unwrap().contains("First"));
    assert!(!posts[0].1["body"].as_str().unwrap().contains("Second"));
    assert!(posts[1].1["body"].as_str().unwrap().contains("Second"));
    assert!(posts.iter().all(|(path, _)| path.contains("/discussions ")));
}

#[tokio::test]
async fn rejects_oversized_feedback_before_writes() {
    let input = input(&[&"x".repeat(MAX_NOTE_CHARACTERS)]);
    let server = Server::start(vec![merge_request(), commits(), empty_discussions()]).await;
    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();
    assert_matches!(error.reason.as_ref(), PublishFailure::NoteTooLong);
    assert_eq!(server.posts(), vec![]);
}

#[test]
fn note_size_limit_counts_unicode_characters() {
    assert_matches!(check_body(&"한".repeat(MAX_NOTE_CHARACTERS)), Ok(()));
    assert_matches!(check_body(&"한".repeat(MAX_NOTE_CHARACTERS + 1)), Err(_));
}

#[tokio::test]
async fn fallback_items_are_separate_from_each_other_and_the_summary() {
    let mut input = input(&["First", "Second", "First"]);
    for finding in &mut input.findings {
        finding.location = None;
    }
    input.summary = Some(crate::review::ReviewSummary {
        peer_version: "0.16.2".into(),
    });
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        created(17),
        merge_request(),
        created(18),
        merge_request(),
        Reply::json(json!({ "id": 19 })),
    ])
    .await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 3);
    assert_eq!(report.inline, 0);
    assert_eq!(report.skipped, 1);
    let posts = server.posts();
    assert_eq!(posts.len(), 3);
    for (index, message) in ["First", "Second"].into_iter().enumerate() {
        assert!(posts[index].0.contains("/merge_requests/42/discussions "));
        assert!(posts[index].1.get("position").is_none());
        let body = posts[index].1["body"].as_str().unwrap();
        assert!(body.contains(message));
        assert!(!body.contains(CONVERSATION_MARKER));
        assert!(!body.contains("Review summary"));
        assert_eq!(fingerprints(body).len(), 1);
    }
    assert!(!posts[0].1["body"].as_str().unwrap().contains("Second"));
    assert!(!posts[1].1["body"].as_str().unwrap().contains("First"));
    assert!(posts[2].0.contains("/notes "));
    let summary = posts[2].1["body"].as_str().unwrap();
    assert!(summary.contains("Review summary"));
    assert!(summary.contains(CONVERSATION_MARKER));
    assert!(!summary.contains("First"));
    assert!(!summary.contains("Second"));
    assert_eq!(fingerprints(summary).len(), 1);
}

#[tokio::test]
async fn existing_combined_notes_still_suppress_items_and_summary() {
    let mut input = input(&["First", "Second"]);
    input.summary = Some(crate::review::ReviewSummary {
        peer_version: "0.16.2".into(),
    });
    let review = PreparedReview::for_gitlab(&input, &repository());
    let legacy_body = format!(
        "{}\n\n{CONVERSATION_MARKER}",
        review.aggregate(&review.items.iter().collect::<Vec<_>>(), true),
    );
    let server = Server::start(vec![merge_request(), commits(), known(&legacy_body, 17)]).await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 0);
    assert_eq!(report.skipped, 3);
    assert_matches!(server.posts()[..], []);
}

#[tokio::test]
async fn uncertain_summary_posts_are_confirmed_with_the_summary_marker() {
    let mut input = input(&[]);
    input.summary = Some(crate::review::ReviewSummary {
        peer_version: "0.16.2".into(),
    });
    let review = PreparedReview::for_gitlab(&input, &repository());
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        Reply::failure(500, json!({})),
        known(&summary_body(&review, true), 19),
    ])
    .await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 1);
    assert_eq!(report.recovered, 1);
    assert_eq!(report.inline, 0);
    assert_eq!(server.posts().len(), 1);
    assert!(server.posts()[0].0.contains("/notes "));
}

#[tokio::test]
async fn an_unconfirmed_fallback_stops_before_posting_the_next_item() {
    let mut input = input(&["First", "Second"]);
    for finding in &mut input.findings {
        finding.location = None;
    }
    let review = PreparedReview::for_gitlab(&input, &repository());
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        Reply::failure(500, json!({})),
        known(&inline_body(&review.items[1]), 19),
    ])
    .await;

    let error = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap_err();
    assert_matches!(error.reason.as_ref(), PublishFailure::Unconfirmed { .. });
    assert_eq!(error.report.published, 0);
    assert_eq!(server.posts().len(), 1);
}

#[tokio::test]
async fn individual_notes_can_exceed_the_limit_in_aggregate() {
    let first = "a".repeat(MAX_NOTE_CHARACTERS / 2 + 1);
    let second = "b".repeat(MAX_NOTE_CHARACTERS / 2 + 1);
    let mut input = input(&[&first, &second]);
    for finding in &mut input.findings {
        finding.location = None;
    }
    let server = Server::start(vec![
        merge_request(),
        commits(),
        empty_discussions(),
        merge_request(),
        merge_request(),
        created(17),
        merge_request(),
        created(18),
    ])
    .await;

    let report = server
        .client()
        .publish(&repository(), number(), &input)
        .await
        .unwrap();
    assert_eq!(report.published, 2);
    let posts = server.posts();
    assert_eq!(posts.len(), 2);
    assert!(posts.iter().all(|(_, payload)| {
        payload["body"].as_str().unwrap().chars().count() < MAX_NOTE_CHARACTERS
    }));
}
