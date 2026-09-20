use super::*;

use std::assert_matches;

use serde_json::{Value, json};

fn merge_request() -> MergeRequest {
    serde_json::from_value(json!({
        "iid": 123, "project_id": 5, "target_project_id": 5,
        "source_project_id": 9, "source_branch": "feature", "target_branch": "main",
        "title": "Title", "description": "Description", "sha": "abc1234",
        "diff_refs": {"base_sha": "0123456", "head_sha": "abc1234", "start_sha": "9876543"}
    }))
    .unwrap()
}

fn note(id: u64) -> Value {
    json!({"id": id, "body": format!("Comment {id}"), "author": {"username": format!("user-{id}")},
        "created_at": "2026-01-01T00:00:00Z", "system": false})
}

fn position() -> Value {
    json!({"position_type": "text", "head_sha": "abc1234", "new_path": "src/new.rs", "old_path": "src/old.rs", "new_line": 10})
}

fn context(value: Value) -> ReviewContext {
    review_context(merge_request(), serde_json::from_value(value).unwrap())
}

#[test]
fn groups_general_notes_and_replies_in_stable_order() {
    let context = context(json!([
        {"id": "b", "notes": [note(3)]},
        {"id": "a", "notes": [note(2), note(1)]},
    ]));
    assert_eq!(context.comments.len(), 2);
    assert_eq!(
        context.comments[0]
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect::<Vec<_>>(),
        ["Comment 1", "Comment 2"]
    );
    assert_eq!(context.comments[1].comments[0].body, "Comment 3");
}

#[test]
fn excludes_system_internal_and_confidential_notes_without_discarding_public_replies() {
    let mut system = note(1);
    system["system"] = json!(true);
    let mut internal = note(2);
    internal["internal"] = json!(true);
    let mut confidential = note(3);
    confidential["confidential"] = json!(true);
    let context = context(json!([
        {"id": "a", "notes": [system, internal, confidential, note(4)]},
        {"id": "b", "notes": []},
    ]));
    assert_eq!(context.comments.len(), 1);
    assert_eq!(context.comments[0].comments[0].body, "Comment 4");
}

#[test]
fn skips_peer_summary_notes_but_keeps_human_replies_and_inline_feedback() {
    let mut summary = note(1);
    summary["body"] = json!(format!("Review summary\r\n {CONVERSATION_MARKER} \r\n"));
    let mut inline = note(3);
    inline["body"] = json!("Finding\n<!-- peer-review:v1:0123456789 -->");
    let context = context(json!([
        {"id": "a", "notes": [summary.clone(), note(2)]},
        {"id": "b", "notes": [summary]},
        {"id": "c", "notes": [inline]},
    ]));
    assert_eq!(context.comments.len(), 2);
    assert_eq!(context.comments[0].comments.len(), 1);
    assert_eq!(context.comments[0].comments[0].body, "Comment 2");
    assert!(context.comments[1].comments[0].body.starts_with("Finding"));
}

#[test]
fn does_not_treat_quoted_or_embedded_markers_as_peer_summary_notes() {
    for body in [
        format!("> {CONVERSATION_MARKER}"),
        format!("prefix {CONVERSATION_MARKER}"),
        format!("{CONVERSATION_MARKER} suffix"),
        "<!-- peer-review:conversation:v2 -->".into(),
    ] {
        let mut note = note(1);
        note["body"] = json!(body);
        assert_eq!(
            context(json!([{"id": "a", "notes": [note]}]))
                .comments
                .len(),
            1
        );
    }
}

#[test]
fn retains_resolved_discussion_comments_without_resolution_metadata() {
    let mut root = note(1);
    root["resolvable"] = json!(true);
    root["resolved"] = json!(true);
    let context = context(json!([{"id": "a", "notes": [root, note(2)]}]));
    assert_eq!(
        serde_json::to_value(context.comments).unwrap(),
        json!([{"comments": [
            {"author": "user-1", "body": "Comment 1"},
            {"author": "user-2", "body": "Comment 2"},
        ]}])
    );
}

#[test]
fn retains_unresolved_discussion_comments_without_resolution_metadata() {
    let mut root = note(1);
    root["resolvable"] = json!(true);
    root["resolved"] = json!(false);
    let context = context(json!([{"id": "a", "notes": [root, note(2)]}]));
    assert_eq!(
        serde_json::to_value(context.comments).unwrap(),
        json!([{"comments": [
            {"author": "user-1", "body": "Comment 1"},
            {"author": "user-2", "body": "Comment 2"},
        ]}])
    );
}

#[test]
fn keeps_the_old_diff_head_paired_with_its_new_side_line() {
    let mut root = note(1);
    let mut old_position = position();
    old_position["head_sha"] = json!("def5678");
    root["position"] = old_position;
    let context = context(json!([{"id": "a", "notes": [note(2), root]}]));
    let thread = &context.comments[0];
    assert_eq!(thread.commit.as_ref().unwrap().as_ref(), "def5678");
    let location = thread.location.as_ref().unwrap();
    assert_eq!(location.path, "src/new.rs");
    assert_eq!(location.line.unwrap().get(), 10);
}

#[test]
fn old_side_rename_positions_keep_the_old_path_without_a_head_line() {
    let mut root = note(1);
    let mut old_position = position();
    old_position["new_line"] = Value::Null;
    old_position["old_line"] = json!(4);
    root["position"] = old_position;
    let context = context(json!([{"id": "a", "notes": [root]}]));
    assert_eq!(context.comments[0].commit, None);
    let location = context.comments[0].location.as_ref().unwrap();
    assert_eq!(location.path, "src/old.rs");
    assert_eq!(location.line, None);
}

#[test]
fn file_image_and_incomplete_positions_do_not_invent_line_locations() {
    for field in ["position_type", "head_sha", "new_path"] {
        let mut root = note(1);
        let mut incomplete = position();
        incomplete[field] = if field == "position_type" {
            json!("image")
        } else {
            Value::Null
        };
        root["position"] = incomplete;
        let context = context(json!([{"id": "a", "notes": [root]}]));
        assert_eq!(
            context.comments[0].location.as_ref().unwrap().line,
            None,
            "{field}"
        );
    }
    let mut root = note(1);
    root["position"] = json!({"position_type": "file", "new_path": "src/new.rs"});
    assert_eq!(
        context(json!([{"id": "a", "notes": [root]}])).comments[0]
            .location
            .as_ref()
            .unwrap()
            .line,
        None
    );
}

#[test]
fn retains_orphan_replies_bots_and_missing_authors() {
    let mut unknown = note(2);
    unknown["author"] = Value::Null;
    let mut bot = note(3);
    bot["author"] = json!({"username": "review-bot", "bot": true});
    let context = context(json!([{"id": "a", "notes": [unknown, bot]}]));
    assert_eq!(context.comments[0].comments[0].author, "unknown");
    assert_eq!(context.comments[0].comments[1].author, "review-bot");
    assert_eq!(context.comments[0].location, None);
}

#[test]
fn merges_repeated_discussion_entries_and_deduplicates_note_ids() {
    let context = context(json!([
        {"id": "a", "notes": [note(2), note(1)]},
        {"id": "a", "notes": [note(2), note(3)]},
    ]));
    assert_eq!(context.comments.len(), 1);
    assert_eq!(context.comments[0].comments.len(), 3);
}

#[test]
fn rejects_zero_lines_and_invalid_commit_hashes() {
    for (field, value) in [("new_line", json!(0)), ("head_sha", json!("invalid"))] {
        let mut root = note(1);
        let mut invalid = position();
        invalid[field] = value;
        root["position"] = invalid;
        assert_matches!(
            serde_json::from_value::<Discussion>(json!({"id": "a", "notes": [root]})),
            Err(_)
        );
    }
}
