use super::*;

use std::assert_matches;

use serde_json::{Value, json};

fn note(id: u64) -> Value {
    json!({"id": id, "body": format!("Comment {id}"), "author": {"username": format!("user-{id}")},
        "created_at": "2026-01-01T00:00:00Z", "system": false})
}

fn position() -> Value {
    json!({"position_type": "text", "head_sha": "abc1234", "new_path": "src/new.rs", "old_path": "src/old.rs", "new_line": 10})
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
