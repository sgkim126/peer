use super::*;

use std::assert_matches;

#[test]
fn note_size_limit_counts_unicode_characters() {
    assert_matches!(check_body(&"한".repeat(MAX_NOTE_CHARACTERS)), Ok(()));
    assert_matches!(check_body(&"한".repeat(MAX_NOTE_CHARACTERS + 1)), Err(_));
}
