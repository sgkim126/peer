use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::stage::FileLocation;

#[derive(Deserialize)]
pub struct ChangedFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub patch: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Side {
    Left,
    Right,
}

pub fn comment_position(files: &[ChangedFile], location: &FileLocation) -> Option<Value> {
    if location.file.is_empty() {
        return None;
    }
    let file = files.iter().find(|file| {
        file.filename == location.file || file.previous_filename.as_deref() == Some(&location.file)
    })?;
    let Some(line) = location.line else {
        return Some(json!({
            "path": file.filename,
            "subject_type": "file"
        }));
    };
    let side = changed_side(file.patch.as_deref()?, line)?;
    Some(json!({
        "path": file.filename,
        "line": line,
        "side": side
    }))
}

fn range(value: &str) -> Option<(u64, u64)> {
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    let start = u64::from(start.parse::<u32>().ok()?);
    let count = u64::from(count.parse::<u32>().ok()?);
    Some((start, start + count))
}

fn changed_side(patch: &str, line: u32) -> Option<Side> {
    if line == 0 {
        return None;
    }
    let target = u64::from(line);
    let mut hunk = None;
    let mut deleted = false;
    for row in patch.lines() {
        if let Some(header) = row.strip_prefix("@@ -") {
            let (old, rest) = header.split_once(" +")?;
            let (new, _) = rest.split_once(" @@")?;
            let (old, old_end) = range(old)?;
            let (new, new_end) = range(new)?;
            hunk = Some((old, old_end, new, new_end));
            continue;
        }
        let (old, old_end, new, new_end) = hunk.as_mut()?;
        match row.as_bytes().first() {
            Some(b'+') if *new < *new_end => {
                if *new == target {
                    return Some(Side::Right);
                }
                *new += 1;
            }
            Some(b'-') if *old < *old_end => {
                if *old == target {
                    deleted = true;
                }
                *old += 1;
            }
            Some(b' ') if *old < *old_end && *new < *new_end => {
                *old += 1;
                *new += 1;
            }
            Some(b'\\') => {}
            _ => return None,
        }
    }
    deleted.then_some(Side::Left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_comments_follow_renamed_paths() {
        let files = vec![ChangedFile {
            filename: "new.png".into(),
            previous_filename: Some("old.png".into()),
            patch: None,
        }];
        let location = FileLocation {
            file: "old.png".into(),
            line: None,
        };
        assert_eq!(
            comment_position(&files, &location),
            Some(json!({
                "path": "new.png",
                "subject_type": "file"
            }))
        );
    }

    #[test]
    fn missing_files_have_no_comment_position() {
        let location = FileLocation {
            file: "missing".into(),
            line: None,
        };
        assert_eq!(comment_position(&[], &location), None);
    }

    #[test]
    fn locates_added_line_on_right() {
        let patch = "@@ -10,0 +11,2 @@\n+first\n+second";
        assert_eq!(changed_side(patch, 12), Some(Side::Right));
    }

    #[test]
    fn locates_deleted_line_on_left() {
        let patch = "@@ -4,2 +3,0 @@\n-first\n-second";
        assert_eq!(changed_side(patch, 5), Some(Side::Left));
    }

    #[test]
    fn prefers_right_side_for_replaced_line() {
        let patch = "@@ -5 +5 @@\n-old\n+new";
        assert_eq!(changed_side(patch, 5), Some(Side::Right));
    }

    #[test]
    fn locates_added_line_in_later_hunk() {
        let patch = "@@ -1 +1 @@\n-old\n+new\n@@ -20 +22 @@\n-old\n+new";
        assert_eq!(changed_side(patch, 22), Some(Side::Right));
    }

    #[test]
    fn locates_deleted_line_in_later_hunk() {
        let patch = "@@ -1 +1 @@\n-old\n+new\n@@ -20 +22 @@\n-old\n+new";
        assert_eq!(changed_side(patch, 20), Some(Side::Left));
    }

    #[test]
    fn prefers_added_line_in_later_hunk_over_earlier_deletion() {
        let patch = "@@ -2 +1,0 @@\n-deleted\n@@ -10,0 +2 @@\n+added";
        assert_eq!(changed_side(patch, 2), Some(Side::Right));
    }

    #[test]
    fn locates_replaced_line_between_context_lines() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 5), Some(Side::Right));
    }

    #[test]
    fn zero_line_has_no_changed_side() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 0), None);
    }

    #[test]
    fn line_before_hunk_has_no_changed_side() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 3), None);
    }

    #[test]
    fn leading_context_line_has_no_changed_side() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 4), None);
    }

    #[test]
    fn trailing_context_line_has_no_changed_side() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 6), None);
    }

    #[test]
    fn line_after_hunk_has_no_changed_side() {
        let patch = "@@ -4,3 +4,3 @@\n context\n-old\n+new\n context";
        assert_eq!(changed_side(patch, 7), None);
    }

    #[test]
    fn empty_patch_has_no_changed_side() {
        assert_eq!(changed_side("", 5), None);
    }

    #[test]
    fn invalid_hunk_header_has_no_changed_side() {
        assert_eq!(changed_side("@@ invalid @@", 5), None);
    }

    #[test]
    fn added_row_in_zero_length_hunk_has_no_changed_side() {
        let patch = "@@ -0,0 +5,0 @@\n+invalid";
        assert_eq!(changed_side(patch, 5), None);
    }

    #[test]
    fn hunk_start_outside_u32_range_has_no_changed_side() {
        let patch = "@@ -1 +4294967296 @@\n-old\n+new";
        assert_eq!(changed_side(patch, 5), None);
    }

    #[test]
    fn line_comments_require_a_patch() {
        let files = vec![ChangedFile {
            filename: "src/main.rs".into(),
            previous_filename: None,
            patch: None,
        }];
        let location = FileLocation {
            file: "src/main.rs".into(),
            line: Some(1),
        };
        assert_eq!(comment_position(&files, &location), None);
    }
}
