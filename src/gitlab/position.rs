use serde::Deserialize;
use serde_json::{Value, json};

use crate::stage::FileLocation;

use super::DiffRefs;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ChangedFile {
    pub old_path: String,
    pub new_path: String,
    pub diff: Option<String>,
    pub new_file: bool,
    pub renamed_file: bool,
    pub deleted_file: bool,
    pub collapsed: bool,
    pub too_large: bool,
}

/// Maps locations in the reviewed head to GitLab diff positions.
///
/// The caller must verify that `refs` identify the reviewed diff. `FileLocation`
/// has no side, so deleted lines and old paths cannot identify head line numbers.
pub fn comment_position(
    files: &[ChangedFile],
    location: &FileLocation,
    refs: &DiffRefs,
) -> Option<Value> {
    if !valid_path(&location.file) {
        return None;
    }
    let mut matches = files
        .iter()
        .filter(|file| file.old_path == location.file || file.new_path == location.file);
    let file = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    if !valid_path(&file.old_path) {
        return None;
    }
    if !valid_path(&file.new_path) {
        return None;
    }
    if file.new_file && file.deleted_file {
        return None;
    }
    if file.renamed_file && (file.new_file || file.deleted_file) {
        return None;
    }

    let mut position = json!({
        "base_sha": refs.base_sha,
        "start_sha": refs.start_sha,
        "head_sha": refs.head_sha,
        "old_path": file.old_path,
        "new_path": file.new_path,
        "position_type": "file",
    });
    let Some(line) = location.line else {
        return Some(position);
    };
    if line == 0 {
        return None;
    }
    if file.new_path != location.file {
        return None;
    }
    if file.deleted_file {
        return None;
    }
    if file.collapsed {
        return None;
    }
    if file.too_large {
        return None;
    }
    let old_line = head_line(file.diff.as_deref()?, line)?;
    position["position_type"] = json!("text");
    position["new_line"] = json!(line);
    if let Some(old_line) = old_line {
        position["old_line"] = json!(old_line);
    }
    Some(position)
}

fn valid_path(path: &str) -> bool {
    !path.contains('\0') && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

struct Hunk {
    old: u64,
    old_end: u64,
    new: u64,
    new_end: u64,
}

impl Hunk {
    fn parse(header: &str) -> Option<Self> {
        let header = header.strip_prefix("@@ -")?;
        let (old, rest) = header.split_once(" +")?;
        let (new, suffix) = rest.split_once(" @@")?;
        if !suffix.is_empty() && !suffix.starts_with(' ') {
            return None;
        }
        let (old, old_end) = range(old)?;
        let (new, new_end) = range(new)?;
        if old == old_end && new == new_end {
            return None;
        }
        Some(Self {
            old,
            old_end,
            new,
            new_end,
        })
    }

    fn complete(&self) -> bool {
        self.old == self.old_end && self.new == self.new_end
    }
}

fn range(value: &str) -> Option<(u64, u64)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    if ![start, count]
        .iter()
        .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let start = u64::from(start.parse::<u32>().ok()?);
    let count = u64::from(count.parse::<u32>().ok()?);
    let end = start + count;
    if start == 0 && count != 0 {
        return None;
    }
    if end > u64::from(u32::MAX) + 1 {
        return None;
    }
    Some((start, end))
}

/// Returns the old coordinate for context, or `None` for an added line.
/// The outer option is absent unless the entire patch is well formed.
fn head_line(patch: &str, line: u32) -> Option<Option<u64>> {
    let target = u64::from(line);
    let mut hunk: Option<Hunk> = None;
    let mut found = None;
    let mut previous_was_line = false;
    for row in patch.lines() {
        if row.starts_with("@@") {
            let next = Hunk::parse(row)?;
            if let Some(previous) = &hunk
                && (!previous.complete()
                    || next.old < previous.old_end
                    || next.new < previous.new_end)
            {
                return None;
            }
            hunk = Some(next);
            previous_was_line = false;
            continue;
        }
        let hunk = hunk.as_mut()?;
        match row.as_bytes().first() {
            Some(b'+') if hunk.new < hunk.new_end => {
                if hunk.new == target {
                    found = Some(None);
                }
                hunk.new += 1;
            }
            Some(b'-') if hunk.old < hunk.old_end => {
                hunk.old += 1;
            }
            Some(b' ') if hunk.old < hunk.old_end && hunk.new < hunk.new_end => {
                if hunk.new == target {
                    found = Some(Some(hunk.old));
                }
                hunk.old += 1;
                hunk.new += 1;
            }
            Some(b'\\') if row == "\\ No newline at end of file" && previous_was_line => {
                previous_was_line = false;
                continue;
            }
            _ => return None,
        }
        previous_was_line = true;
    }
    hunk?.complete().then_some(found).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    use crate::git::CommitHash;

    fn refs() -> DiffRefs {
        DiffRefs {
            base_sha: CommitHash::new("a".repeat(40).as_str()).unwrap(),
            head_sha: CommitHash::new("b".repeat(40).as_str()).unwrap(),
            start_sha: CommitHash::new("c".repeat(40).as_str()).unwrap(),
        }
    }

    fn file(patch: &str) -> ChangedFile {
        ChangedFile {
            old_path: "src/main.rs".into(),
            new_path: "src/main.rs".into(),
            diff: Some(patch.into()),
            ..ChangedFile::default()
        }
    }

    fn location(line: Option<u32>) -> FileLocation {
        FileLocation {
            file: "src/main.rs".into(),
            line,
        }
    }

    #[test]
    fn defaults_missing_optional_diff_metadata() {
        let file: ChangedFile = serde_json::from_value(json!({
            "old_path": "src/main.rs",
            "new_path": "src/main.rs",
        }))
        .unwrap();

        assert_eq!(file.diff, None);
        assert!(!file.collapsed);
        assert!(!file.too_large);
    }

    #[test]
    fn rejects_text_positions_when_diff_is_missing() {
        let mut file = file("");
        file.diff = None;

        assert_matches!(comment_position(&[file], &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_file_positions_when_old_path_is_missing() {
        let file: ChangedFile = serde_json::from_value(json!({
            "new_path": "src/main.rs",
        }))
        .unwrap();

        assert_matches!(comment_position(&[file], &location(None), &refs()), None);
    }

    #[test]
    fn rejects_file_positions_when_new_path_is_missing() {
        let file: ChangedFile = serde_json::from_value(json!({
            "old_path": "src/main.rs",
        }))
        .unwrap();

        assert_matches!(comment_position(&[file], &location(None), &refs()), None);
    }

    #[test]
    fn locates_addition_using_all_three_distinct_diff_refs() {
        let files = [file("@@ -10,0 +11,2 @@\n+first\n+second")];
        assert_eq!(
            comment_position(&files, &location(Some(12)), &refs()),
            Some(json!({
                "base_sha": "a".repeat(40),
                "start_sha": "c".repeat(40),
                "head_sha": "b".repeat(40),
                "position_type": "text",
                "old_path": "src/main.rs",
                "new_path": "src/main.rs",
                "new_line": 12,
            }))
        );
    }

    #[test]
    fn maps_shifted_context_to_both_line_numbers() {
        let files = [file("@@ -4,2 +4,3 @@ name\n context\n+added\n context")];
        let position = comment_position(&files, &location(Some(6)), &refs()).unwrap();
        assert_eq!(position["old_line"], 5);
        assert_eq!(position["new_line"], 6);
    }

    #[test]
    fn replacement_uses_head_coordinate_without_guessing_deleted_line() {
        let files = [file("@@ -5 +5 @@\n-old\n+new")];
        let position = comment_position(&files, &location(Some(5)), &refs()).unwrap();
        assert_eq!(position["new_line"], 5);
        assert_matches!(position.get("old_line"), None);
    }

    #[test]
    fn deletion_does_not_alias_a_head_line_with_the_same_number() {
        let files = [file("@@ -4,2 +3,0 @@\n-first\n-second")];
        assert_matches!(comment_position(&files, &location(Some(4)), &refs()), None);

        let files = [file("@@ -4,2 +4 @@\n-deleted\n context")];
        let position = comment_position(&files, &location(Some(4)), &refs()).unwrap();
        assert_eq!(position["old_line"], 5);
        assert_eq!(position["new_line"], 4);
    }

    #[test]
    fn maps_addition_in_later_hunk() {
        let files = [file("@@ -2 +1,0 @@\n-deleted\n@@ -10,0 +10 @@\n+added")];
        let position = comment_position(&files, &location(Some(10)), &refs()).unwrap();
        assert_eq!(position["new_line"], 10);
        assert_matches!(position.get("old_line"), None);
    }

    #[test]
    fn rejects_text_positions_for_old_paths_after_rename() {
        let mut file = file("@@ -1 +1 @@\n-old\n+new");
        file.new_path = "src/renamed.rs".into();
        file.renamed_file = true;

        assert_matches!(comment_position(&[file], &location(Some(1)), &refs()), None);
    }

    #[test]
    fn maps_text_positions_for_new_paths_after_rename() {
        let mut file = file("@@ -1 +1 @@\n-old\n+new");
        file.new_path = "src/renamed.rs".into();
        file.renamed_file = true;
        let location = FileLocation {
            file: "src/renamed.rs".into(),
            line: Some(1),
        };

        let position = comment_position(&[file], &location, &refs()).unwrap();
        assert_eq!(position["position_type"], "text");
        assert_eq!(position["old_path"], "src/main.rs");
        assert_eq!(position["new_path"], "src/renamed.rs");
        assert_eq!(position["new_line"], 1);
    }

    #[test]
    fn maps_file_positions_for_old_paths_after_rename() {
        let mut file = file("@@ -1 +1 @@\n-old\n+new");
        file.new_path = "src/renamed.rs".into();
        file.renamed_file = true;

        let position = comment_position(&[file], &location(None), &refs()).unwrap();
        assert_eq!(position["position_type"], "file");
        assert_eq!(position["old_path"], "src/main.rs");
        assert_eq!(position["new_path"], "src/renamed.rs");
        assert_matches!(position.get("new_line"), None);
    }

    #[test]
    fn rejects_ambiguous_paths() {
        let mut renamed = file("@@ -1 +1 @@\n-old\n+new");
        renamed.new_path = "src/renamed.rs".into();
        let files = [renamed, file("@@ -0,0 +1 @@\n+added")];
        assert_matches!(comment_position(&files, &location(None), &refs()), None);
        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn file_positions_do_not_require_text_diffs() {
        let mut file = file("");
        file.diff = None;
        file.too_large = true;
        let position = comment_position(&[file], &location(None), &refs()).unwrap();
        assert_eq!(position["position_type"], "file");
        assert_matches!(position.get("old_line"), None);
        assert_matches!(position.get("new_line"), None);
    }

    #[test]
    fn rejects_text_positions_for_collapsed_diffs() {
        let mut file = file("@@ -0,0 +1 @@\n+added");
        file.collapsed = true;

        assert_matches!(comment_position(&[file], &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_text_positions_for_oversized_diffs() {
        let mut file = file("@@ -0,0 +1 @@\n+added");
        file.too_large = true;

        assert_matches!(comment_position(&[file], &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_text_positions_for_deleted_files() {
        let mut file = file("@@ -0,0 +1 @@\n+added");
        file.deleted_file = true;

        assert_matches!(comment_position(&[file], &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_empty_paths() {
        assert!(!valid_path(""));
    }

    #[test]
    fn rejects_absolute_paths() {
        assert!(!valid_path("/src/main.rs"));
    }

    #[test]
    fn rejects_parent_directory_components_in_paths() {
        assert!(!valid_path("src/../main.rs"));
    }

    #[test]
    fn rejects_current_directory_components_in_paths() {
        assert!(!valid_path("./src/main.rs"));
    }

    #[test]
    fn rejects_empty_path_components() {
        assert!(!valid_path("src//main.rs"));
    }

    #[test]
    fn rejects_null_bytes_in_paths() {
        assert!(!valid_path("a\0b"));
    }

    #[test]
    fn rejects_positions_when_no_files_changed() {
        assert_matches!(comment_position(&[], &location(None), &refs()), None);
    }

    #[test]
    fn rejects_line_zero() {
        let files = [file("@@ -5 +5 @@\n-old\n+new")];

        assert_matches!(comment_position(&files, &location(Some(0)), &refs()), None);
    }

    #[test]
    fn rejects_lines_before_a_hunk() {
        let files = [file("@@ -5 +5 @@\n-old\n+new")];

        assert_matches!(comment_position(&files, &location(Some(4)), &refs()), None);
    }

    #[test]
    fn rejects_lines_after_a_hunk() {
        let files = [file("@@ -5 +5 @@\n-old\n+new")];

        assert_matches!(comment_position(&files, &location(Some(6)), &refs()), None);
    }

    #[test]
    fn accepts_no_newline_markers() {
        let files = [file(
            "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file",
        )];

        let position = comment_position(&files, &location(Some(1)), &refs()).unwrap();
        assert_eq!(position["new_line"], 1);
    }

    #[test]
    fn accepts_u32_maximum_line_number() {
        let files = [file("@@ -4294967295 +4294967295 @@\n-old\n+new")];

        let position = comment_position(&files, &location(Some(u32::MAX)), &refs()).unwrap();
        assert_eq!(position["new_line"], u32::MAX);
    }

    #[test]
    fn rejects_empty_patches_for_text_positions() {
        assert_matches!(
            comment_position(&[file("")], &location(Some(1)), &refs()),
            None
        );
    }

    #[test]
    fn rejects_invalid_hunk_header_syntax() {
        assert!(Hunk::parse("@@ invalid @@").is_none());
    }

    #[test]
    fn rejects_truncated_hunks_even_when_the_target_line_is_present() {
        let files = [file("@@ -0,0 +1,2 @@\n+first")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_truncated_later_hunks_after_finding_the_target_line() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -5 +5,2 @@\n-old\n+new")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_unprefixed_patch_rows_after_finding_the_target_line() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\ntruncated")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_added_rows_exceeding_the_declared_hunk_length() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n+extra")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_overlapping_hunks_after_finding_the_target_line() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -1 +1 @@\n-old\n+new")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_hunks_with_two_empty_ranges() {
        assert!(Hunk::parse("@@ -0,0 +1,0 @@").is_none());
    }

    #[test]
    fn rejects_nonempty_hunk_ranges_starting_at_zero() {
        assert_matches!(range("0"), None);
    }

    #[test]
    fn rejects_hunk_range_starts_exceeding_u32_maximum() {
        assert_matches!(range("4294967296"), None);
    }

    #[test]
    fn rejects_hunk_ranges_extending_past_u32_maximum() {
        let files = [file("@@ -0,0 +4294967295,2 @@\n+one\n+two")];

        assert_matches!(
            comment_position(&files, &location(Some(u32::MAX)), &refs()),
            None
        );
    }

    #[test]
    fn rejects_signed_hunk_range_starts() {
        let files = [file("@@ -0,0 ++1 @@\n+invalid")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_hunk_header_suffixes_without_a_leading_space() {
        let files = [file("@@ -0,0 +1 @@invalid\n+invalid")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_no_newline_markers_before_any_hunk_row() {
        let files = [file(
            "@@ -0,0 +1 @@\n\\ No newline at end of file\n+invalid",
        )];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }

    #[test]
    fn rejects_unsupported_backslash_markers_after_finding_the_target_line() {
        let files = [file("@@ -0,0 +1 @@\n+valid\n\\ unsupported marker")];

        assert_matches!(comment_position(&files, &location(Some(1)), &refs()), None);
    }
}
