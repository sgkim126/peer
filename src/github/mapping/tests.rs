use super::*;

use std::assert_matches;

use serde_json::{Value, json};

use super::super::feedback::{fingerprints, marker};

fn pull() -> PullRequest {
    PullRequest {
        title: "Title".into(),
        body: Some("Description".into()),
        base: super::super::client::CommitRef {
            sha: crate::git::CommitHash::new("0123456").unwrap(),
            repo: None,
        },
        head: super::super::client::CommitRef {
            sha: crate::git::CommitHash::new("abc1234").unwrap(),
            repo: None,
        },
        commits: 1,
    }
}

fn review(id: u64, state: &str, body: Value) -> PullRequestReview {
    serde_json::from_value(json!({
        "id": id, "state": state, "body": body, "submitted_at": "2026-01-01T00:00:00Z",
        "user": {"login": "reviewer[bot]", "type": "Bot"}, "commit_id": "abc1234",
    }))
    .unwrap()
}

fn inline(id: u64, parent: Option<u64>) -> Value {
    json!({
        "id": id, "in_reply_to_id": parent, "created_at": "2026-01-01T00:00:00Z",
        "user": {"login": "reviewer[bot]", "type": "Bot"}, "body": format!("Inline {id}"),
        "path": "src/root.rs", "commit_id": "abc1234", "original_commit_id": "def5678",
        "line": 42, "original_line": 7, "side": "RIGHT",
    })
}

fn with_inline(comments: Vec<Value>) -> ReviewContext {
    review_context(
        pull(),
        vec![],
        vec![],
        serde_json::from_value(json!(comments)).unwrap(),
        vec![],
    )
}

#[test]
fn conversation_marker_is_recognized_as_the_entire_body() {
    assert!(is_peer_conversation(CONVERSATION_MARKER));
}

#[test]
fn conversation_marker_is_recognized_after_review_text() {
    let body = format!("Review\n\n{CONVERSATION_MARKER}");
    assert!(is_peer_conversation(&body));
}

#[test]
fn conversation_marker_is_recognized_with_surrounding_whitespace_and_crlf() {
    let body = format!("Review\r\n \t{CONVERSATION_MARKER} \t\r\nMore text");
    assert!(is_peer_conversation(&body));
}

#[test]
fn empty_body_is_not_a_peer_conversation() {
    assert!(!is_peer_conversation(""));
}

#[test]
fn fingerprint_marker_is_not_a_peer_conversation() {
    let body = marker(&"a".repeat(64));
    assert!(!is_peer_conversation(&body));
}

#[test]
fn quoted_conversation_marker_is_not_recognized() {
    let body = format!("> {CONVERSATION_MARKER}");
    assert!(!is_peer_conversation(&body));
}

#[test]
fn conversation_marker_with_leading_text_is_not_recognized() {
    let body = format!("Quoted: {CONVERSATION_MARKER}");
    assert!(!is_peer_conversation(&body));
}

#[test]
fn conversation_marker_with_trailing_text_is_not_recognized() {
    let body = format!("{CONVERSATION_MARKER} extra text");
    assert!(!is_peer_conversation(&body));
}

#[test]
fn incomplete_conversation_marker_is_not_recognized() {
    assert!(!is_peer_conversation("<!-- peer-review:conversation:v1"));
}

#[test]
fn unsupported_conversation_marker_version_is_not_recognized() {
    let body = CONVERSATION_MARKER.replace(":v1", ":v2");
    assert!(!is_peer_conversation(&body));
}

#[test]
fn conversation_marker_contains_no_fingerprints() {
    assert!(fingerprints(CONVERSATION_MARKER).is_empty());
}

#[test]
fn skips_marked_conversation_comments_and_preserves_other_context() {
    let marked = format!("Peer review\n\n{CONVERSATION_MARKER}");
    let legacy = format!("Previous peer review\n\n{}", marker(&"a".repeat(64)));
    let comments = serde_json::from_value(json!([
        {"id": 5, "created_at": "2026-01-01T00:00:00Z", "user": null, "body": marked},
        {"id": 4, "created_at": "2026-01-01T00:00:00Z", "user": {"login": "other[bot]"}, "body": "Bot feedback"},
        {"id": 3, "created_at": "2026-01-01T00:00:00Z", "user": {"login": "peer[bot]"}, "body": legacy},
        {"id": 2, "created_at": "2026-01-01T00:00:00Z", "user": {"login": "peer[bot]"}, "body": marked},
        {"id": 1, "created_at": "2026-01-01T00:00:00Z", "user": {"login": "author"}, "body": "Human answer"},
    ])).unwrap();
    let mut root = inline(10, None);
    root["body"] = json!(marked);
    let context = review_context(
        pull(),
        comments,
        vec![review(1, "COMMENTED", json!(marked))],
        serde_json::from_value(json!([root, inline(11, Some(10))])).unwrap(),
        vec![],
    );

    assert_eq!(context.title.as_deref(), Some("Title"));
    assert_eq!(context.body.as_deref(), Some("Description"));
    assert_eq!(
        context
            .comments
            .iter()
            .map(|thread| thread.comments[0].body.as_str())
            .collect::<Vec<_>>(),
        [
            "Human answer",
            legacy.as_str(),
            "Bot feedback",
            &marked,
            &marked
        ]
    );
    let thread = &context.comments[4];
    assert_eq!(thread.location.as_ref().unwrap().path, "src/root.rs");
    assert_eq!(thread.comments.len(), 2);
    assert_eq!(thread.comments[1].body, "Inline 11");
}

#[test]
fn all_marked_conversation_comments_leave_no_threads() {
    let comments = serde_json::from_value(json!([{
        "id": 1, "created_at": "2026-01-01T00:00:00Z", "user": {"login": "author"},
        "body": format!("Peer review\n\n{CONVERSATION_MARKER}"),
    }]))
    .unwrap();
    let context = review_context(pull(), comments, vec![], vec![], vec![]);
    assert_eq!(context.comments, vec![]);
}

#[test]
fn includes_nonempty_submitted_reviews_and_retains_bots() {
    let mut unsubmitted = review(4, "COMMENTED", json!("Draft"));
    unsubmitted.submitted_at = None;
    let mut deleted_author = review(1, "CHANGES_REQUESTED", json!("Please change this"));
    deleted_author.user = None;
    let context = review_context(
        pull(),
        vec![],
        vec![
            review(3, "DISMISSED", json!("Historical rationale")),
            review(2, "APPROVED", json!("  Verified behavior\n")),
            review(5, "PENDING", json!("Draft")),
            review(6, "APPROVED", json!(" \n")),
            review(7, "COMMENTED", Value::Null),
            unsubmitted,
            deleted_author,
        ],
        vec![],
        vec![],
    );

    assert_eq!(context.comments.len(), 3);
    assert_eq!(context.comments[0].comments[0].author, "unknown");
    assert_eq!(context.comments[1].comments[0].author, "reviewer[bot]");
    assert_eq!(
        context.comments[1].comments[0].body,
        "  Verified behavior\n"
    );
    assert_eq!(context.comments[2].comments[0].body, "Historical rationale");
    for thread in &context.comments {
        assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "abc1234");
        assert_eq!(thread.location, None);
    }
}

#[test]
fn groups_replies_using_the_root_location_and_stable_order() {
    let mut reply = inline(11, Some(10));
    reply["path"] = json!("src/reply.rs");
    reply["line"] = json!(99);
    let mut earlier_root = inline(20, None);
    earlier_root["created_at"] = json!("2025-01-01T00:00:00Z");
    let input = vec![reply, earlier_root, inline(10, None)];
    let context = with_inline(input.clone());
    let mut reversed = input;
    reversed.reverse();
    assert_eq!(context, with_inline(reversed));
    assert_eq!(context.comments.len(), 2);
    assert_eq!(context.comments[0].comments[0].body, "Inline 20");
    let thread = &context.comments[1];
    assert_eq!(
        thread
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["Inline 10", "Inline 11"]
    );
    assert_eq!(thread.location.as_ref().unwrap().path, "src/root.rs");
    assert_eq!(thread.location.as_ref().unwrap().line.unwrap().get(), 42);
}

#[test]
fn retains_replies_when_the_root_or_author_is_missing() {
    let mut first = inline(11, Some(10));
    first["user"] = Value::Null;
    let context = with_inline(vec![inline(12, Some(10)), first]);
    assert_eq!(context.comments.len(), 1);
    let comments = &context.comments[0].comments;
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author, "unknown");
    assert_eq!(comments[0].body, "Inline 11");
    assert_eq!(comments[1].body, "Inline 12");
}

#[test]
fn pairs_right_side_lines_with_the_matching_commit() {
    let current = inline(1, None);
    let mut outdated = inline(2, None);
    outdated["line"] = Value::Null;
    let mut left = inline(3, None);
    left["side"] = json!("LEFT");
    let mut file = inline(4, None);
    file["line"] = Value::Null;
    file["original_line"] = Value::Null;
    file["side"] = Value::Null;
    let mut unknown_original = inline(5, None);
    unknown_original["line"] = Value::Null;
    unknown_original["original_commit_id"] = Value::Null;

    for (comment, expected_commit, expected_line) in [
        (current, "abc1234", Some(42)),
        (outdated, "def5678", Some(7)),
        (left, "abc1234", None),
        (file, "abc1234", None),
        (unknown_original, "abc1234", None),
    ] {
        let context = with_inline(vec![comment]);
        let thread = &context.comments[0];
        assert_eq!(thread.commit.as_ref().unwrap().as_ref(), expected_commit);
        let location = thread.location.as_ref().unwrap();
        assert_eq!(location.path, "src/root.rs");
        assert_eq!(location.line.map(|line| line.get()), expected_line);
    }
}

#[test]
fn orders_comment_categories_chronologically_and_matches_direct_context_files() {
    let issue = serde_json::from_value(json!({
        "id": 100, "created_at": "2026-02-01T00:00:00Z", "user": {"login": "author"}, "body": "Rationale",
    })).unwrap();
    let context = review_context(
        pull(),
        vec![issue],
        vec![review(1, "APPROVED", json!("Verified"))],
        serde_json::from_value(json!([inline(10, None), inline(11, Some(10))])).unwrap(),
        serde_json::from_value(json!([native(10)])).unwrap(),
    );
    let directory = tempfile::tempdir().unwrap();
    let body = directory.path().join("body.md");
    let comments = directory.path().join("comments.json");
    std::fs::write(&body, "Description").unwrap();
    std::fs::write(
        &comments,
        r#"[
        {"commit":"abc1234","comments":[{"author":"reviewer[bot]","body":"Verified"}]},
        {"commit":"abc1234","location":{"path":"src/root.rs","line":42},"comments":[
            {"author":"reviewer[bot]","body":"Inline 10"},
            {"author":"reviewer[bot]","body":"Inline 11"}
        ]},
        {"commit":"abc1234","location":{"path":"src/root.rs"},"comments":[
            {"author":"reviewer[bot]","body":"Commit comment 10"}
        ]},
        {"comments":[{"author":"author","body":"Rationale"}]}
    ]"#,
    )
    .unwrap();
    assert_eq!(
        context,
        ReviewContext::load(Some("Title".into()), Some(&body), Some(&comments)).unwrap()
    );
}

#[test]
fn interleaves_all_comment_categories_by_thread_start_and_keeps_replies_grouped() {
    let at = |mut comment: Value, day| {
        comment["created_at"] = json!(format!("2026-01-{day:02}T00:00:00Z"));
        comment
    };
    let mut inputs = [
        vec![
            json!({"id": 2, "created_at": "2026-01-06T00:00:00Z", "body": "Later rationale"}),
            json!({"id": 1, "created_at": "2026-01-02T00:00:00Z", "body": "Earlier rationale"}),
        ],
        vec![
            json!({"id": 2, "submitted_at": "2026-01-08T00:00:00Z", "state": "APPROVED", "body": "Later review"}),
            json!({"id": 1, "submitted_at": "2026-01-04T00:00:00Z", "state": "COMMENTED", "body": "Earlier review"}),
        ],
        vec![
            at(inline(22, Some(20)), 10),
            at(inline(11, Some(10)), 9),
            at(inline(21, Some(20)), 7),
            at(inline(10, None), 3),
        ],
        vec![at(native(2), 5), at(native(1), 1)],
    ];
    let map = |inputs: &[Vec<Value>; 4]| {
        review_context(
            pull(),
            serde_json::from_value(json!(inputs[0])).unwrap(),
            serde_json::from_value(json!(inputs[1])).unwrap(),
            serde_json::from_value(json!(inputs[2])).unwrap(),
            serde_json::from_value(json!(inputs[3])).unwrap(),
        )
    };
    let context = map(&inputs);
    for input in &mut inputs {
        input.reverse();
    }
    assert_eq!(context, map(&inputs));
    assert_eq!(
        context
            .comments
            .iter()
            .map(|thread| {
                thread
                    .comments
                    .iter()
                    .map(|comment| comment.body.as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>(),
        [
            vec!["Commit comment 1"],
            vec!["Earlier rationale"],
            vec!["Inline 10", "Inline 11"],
            vec!["Earlier review"],
            vec!["Commit comment 2"],
            vec!["Later rationale"],
            vec!["Inline 21", "Inline 22"],
            vec!["Later review"],
        ]
    );
}

#[test]
fn rejects_invalid_commit_hashes_at_the_api_boundary() {
    let mut invalid_hash = inline(1, None);
    invalid_hash["commit_id"] = json!("invalid-hash");
    assert_matches!(
        serde_json::from_value::<ReviewComment>(invalid_hash),
        Err(_)
    );
}

#[test]
fn rejects_zero_lines_at_the_api_boundary() {
    let mut zero_line = inline(1, None);
    zero_line["line"] = json!(0);
    assert_matches!(serde_json::from_value::<ReviewComment>(zero_line), Err(_));
}

fn native(id: u64) -> Value {
    json!({
        "id": id,
        "created_at": "2026-01-01T00:00:00Z",
        "user": {
            "login": "reviewer[bot]"
        }, "body": format!("Commit comment {id}"),
        "path": "src/root.rs",
        "commit_id": "abc1234",
        "line": 14,
        "position": 4,
    })
}

fn with_native(comments: Vec<Value>) -> ReviewContext {
    review_context(
        pull(),
        vec![],
        vec![],
        vec![],
        serde_json::from_value(json!(comments)).unwrap(),
    )
}

#[test]
fn keeps_native_comments_independent_and_sorted_without_assuming_line_coordinates() {
    let mut earlier = native(3);
    earlier["created_at"] = json!("2025-01-01T00:00:00Z");
    earlier["commit_id"] = json!("def5678");
    let input = vec![native(2), earlier, native(1)];
    let context = with_native(input.clone());
    let mut reversed = input;
    reversed.reverse();
    assert_eq!(context, with_native(reversed));
    assert_eq!(context.comments.len(), 3);
    assert_eq!(
        context.comments[0].commit.as_ref().unwrap().as_ref(),
        "def5678"
    );
    for (thread, id) in context.comments.iter().zip([3, 1, 2]) {
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(thread.comments[0].body, format!("Commit comment {id}"));
        assert_eq!(thread.comments[0].author, "reviewer[bot]");
        let location = thread.location.as_ref().unwrap();
        assert_eq!(location.path, "src/root.rs");
        assert_eq!(location.line, None);
    }
}

#[test]
fn retains_native_feedback_markers_and_handles_missing_authors_and_paths() {
    let mut comment = native(1);
    comment["path"] = Value::Null;
    comment["user"] = Value::Null;
    // The conversation marker filters only PR issue comments, as before.
    let body = format!(
        "Feedback\n{}\n{CONVERSATION_MARKER}",
        marker(&"a".repeat(64))
    );
    comment["body"] = json!(body);
    let context = with_native(vec![comment]);
    assert_eq!(context.comments.len(), 1);
    let thread = &context.comments[0];
    assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "abc1234");
    assert_eq!(thread.location, None);
    assert_eq!(thread.comments[0].author, "unknown");
    assert_eq!(thread.comments[0].body, body);
}
