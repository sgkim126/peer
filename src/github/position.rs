use std::num::NonZeroU32;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::stage::FileLocation;

#[derive(Debug, Default, Deserialize)]
pub struct ChangedFile {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub patch: Option<String>,
    pub status: Option<String>,
}

/// Maps locations in the reviewed head to GitHub PR diff positions.
///
/// The caller must verify that the diff is for the location's commit.
/// `FileLocation` has no side, so deleted lines and old paths cannot identify
/// head line numbers. File comments may still follow an old path after a rename.
pub fn comment_position(files: &[ChangedFile], location: &FileLocation) -> Option<Value> {
    let file = changed_file(files, &location.file)?;
    let Some(line) = location.line else {
        return Some(json!({
            "path": file.filename,
            "subject_type": "file"
        }));
    };
    if line == 0 {
        return None;
    }
    if file.filename != location.file {
        return None;
    }
    if file.status.as_deref() == Some("removed") {
        return None;
    }
    if !is_new_line_in_patch(file.patch.as_deref()?, line) {
        return None;
    }
    Some(json!({
        "path": file.filename,
        "line": line,
        "side": "RIGHT"
    }))
}

fn changed_file<'a>(files: &'a [ChangedFile], path: &str) -> Option<&'a ChangedFile> {
    if !valid_path(path) {
        return None;
    }
    let mut matches = files
        .iter()
        .filter(|file| file.filename == path || file.previous_filename.as_deref() == Some(path));
    let file = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    if !valid_path(&file.filename) {
        return None;
    }
    if file
        .previous_filename
        .as_deref()
        .is_some_and(|path| !valid_path(path))
    {
        return None;
    }
    Some(file)
}

fn valid_path(path: &str) -> bool {
    !path.contains('\0') && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

#[derive(Debug, PartialEq)]
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

/// Returns whether `line` is an added or context line in a valid, complete patch.
fn is_new_line_in_patch(patch: &str, line: u32) -> bool {
    let mut found = false;
    let valid = visit_patch_lines(patch, |_, patch_line| {
        found |= matches!(patch_line, PatchLine::New(number) if number.get() == line);
    })
    .is_some();
    valid && found
}

enum PatchLine {
    New(NonZeroU32),
    Old,
}

/// Returns `Some(())` if the entire patch is valid and complete, or `None` if it
/// is empty, malformed, or incomplete. Callbacks may run before validation
/// fails, so callers must check the return value before using collected results.
fn visit_patch_lines(patch: &str, mut visit: impl FnMut(u32, PatchLine)) -> Option<()> {
    let mut hunk: Option<Hunk> = None;
    let mut previous_was_line = false;
    for (position, row) in patch.lines().enumerate() {
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
        let line = match row.as_bytes().first() {
            Some(b'+') if hunk.new < hunk.new_end => {
                let line = NonZeroU32::new(hunk.new.try_into().ok()?)?;
                hunk.new += 1;
                PatchLine::New(line)
            }
            Some(b'-') if hunk.old < hunk.old_end => {
                hunk.old += 1;
                PatchLine::Old
            }
            Some(b' ') if hunk.old < hunk.old_end && hunk.new < hunk.new_end => {
                let line = NonZeroU32::new(hunk.new.try_into().ok()?)?;
                hunk.old += 1;
                hunk.new += 1;
                PatchLine::New(line)
            }
            Some(b'\\') if row == "\\ No newline at end of file" && previous_was_line => {
                previous_was_line = false;
                continue;
            }
            _ => return None,
        };
        visit(position.try_into().ok()?, line);
        previous_was_line = true;
    }
    hunk?.complete().then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(patch: &str) -> ChangedFile {
        ChangedFile {
            filename: "src/main.rs".into(),
            patch: Some(patch.into()),
            ..ChangedFile::default()
        }
    }

    fn location(line: Option<u32>) -> FileLocation {
        FileLocation {
            file: "src/main.rs".into(),
            line,
        }
    }

    fn right(line: u32) -> Value {
        json!({
            "path": "src/main.rs",
            "line": line,
            "side": "RIGHT"
        })
    }

    #[test]
    fn file_comments_follow_renamed_paths_without_a_patch() {
        let files = [serde_json::from_value(json!({
            "filename": "src/new.rs",
            "previous_filename": "src/main.rs"
        }))
        .unwrap()];
        assert_eq!(
            comment_position(&files, &location(None)),
            Some(json!({
                "path": "src/new.rs",
                "subject_type": "file"
            }))
        );
    }

    #[test]
    fn line_comments_reject_old_paths_after_rename() {
        let mut changed = file("@@ -1 +1 @@\n-old\n+new");
        changed.previous_filename = Some("src/main.rs".into());
        changed.filename = "src/new.rs".into();
        assert_eq!(comment_position(&[changed], &location(Some(1))), None);
    }

    #[test]
    fn line_comments_use_new_paths_after_rename() {
        let mut changed = file("@@ -1 +1 @@\n-old\n+new");
        changed.previous_filename = Some("src/old.rs".into());
        assert_eq!(
            comment_position(&[changed], &location(Some(1))),
            Some(right(1))
        );
    }

    #[test]
    fn rejects_ambiguous_paths_for_file_comments() {
        let mut renamed = file("@@ -1 +1 @@\n-old\n+new");
        renamed.filename = "src/new.rs".into();
        renamed.previous_filename = Some("src/main.rs".into());
        let files = [renamed, file("@@ -0,0 +1 @@\n+new")];
        assert_eq!(comment_position(&files, &location(None)), None);
    }

    #[test]
    fn rejects_ambiguous_paths_for_line_comments() {
        let mut renamed = file("@@ -1 +1 @@\n-old\n+new");
        renamed.filename = "src/new.rs".into();
        renamed.previous_filename = Some("src/main.rs".into());
        let files = [file("@@ -0,0 +1 @@\n+new"), renamed];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
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
    fn rejects_file_comments_for_invalid_paths_after_rename() {
        let changed = ChangedFile {
            filename: "../new.rs".into(),
            previous_filename: Some("src/main.rs".into()),
            ..ChangedFile::default()
        };
        assert_eq!(comment_position(&[changed], &location(None)), None);
    }

    #[test]
    fn rejects_invalid_previous_paths_even_when_current_path_matches() {
        let mut changed = file("@@ -0,0 +1 @@\n+new");
        changed.previous_filename = Some("../old.rs".into());
        assert_eq!(comment_position(&[changed], &location(None)), None);
    }

    #[test]
    fn missing_files_have_no_comment_position() {
        assert_eq!(comment_position(&[], &location(None)), None);
        assert_eq!(comment_position(&[], &location(Some(1))), None);
    }

    #[test]
    fn line_comments_require_a_patch() {
        let changed = ChangedFile {
            patch: None,
            ..file("")
        };
        assert_eq!(comment_position(&[changed], &location(Some(1))), None);
    }

    #[test]
    fn removed_files_only_allow_file_comments() {
        let changed = ChangedFile {
            status: Some("removed".into()),
            ..file("@@ -1 +1 @@\n-old\n+new")
        };
        let files = [changed];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
        assert_eq!(
            comment_position(&files, &location(None)),
            Some(json!({
                "path": "src/main.rs",
                "subject_type": "file"
            }))
        );
    }

    #[test]
    fn locates_added_lines_on_right() {
        let files = [file("@@ -10,0 +11,2 @@\n+first\n+second")];
        assert_eq!(
            comment_position(&files, &location(Some(12))),
            Some(right(12))
        );
    }

    #[test]
    fn locates_context_before_and_after_replaced_lines() {
        let files = [file("@@ -4,3 +4,3 @@ name\n context\n-old\n+new\n context")];
        for line in [4, 5, 6] {
            assert_eq!(
                comment_position(&files, &location(Some(line))),
                Some(right(line))
            );
        }
    }

    #[test]
    fn shifted_context_uses_post_change_line_number() {
        let files = [file("@@ -4,2 +4,3 @@\n context\n+added\n context")];
        assert_eq!(comment_position(&files, &location(Some(6))), Some(right(6)));
    }

    #[test]
    fn deleted_line_numbers_do_not_select_left_side() {
        let files = [file("@@ -4,2 +3,0 @@\n-first\n-second")];
        assert_eq!(comment_position(&files, &location(Some(4))), None);
        assert_eq!(comment_position(&files, &location(Some(5))), None);
    }

    #[test]
    fn deletion_does_not_hide_context_at_the_same_number() {
        let files = [file("@@ -4,2 +4 @@\n-deleted\n context")];
        assert_eq!(comment_position(&files, &location(Some(4))), Some(right(4)));
    }

    #[test]
    fn locates_post_change_line_in_later_hunk() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -20 +22 @@\n-old\n+new")];
        assert_eq!(
            comment_position(&files, &location(Some(22))),
            Some(right(22))
        );
        assert_eq!(comment_position(&files, &location(Some(20))), None);
    }

    #[test]
    fn rejects_zero_and_lines_outside_hunks() {
        let files = [file("@@ -4,3 +4,3 @@\n context\n-old\n+new\n context")];
        for line in [0, 3, 7] {
            assert_eq!(comment_position(&files, &location(Some(line))), None);
        }
    }

    #[test]
    fn accepts_no_newline_markers() {
        let files = [file(
            "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n\\ No newline at end of file",
        )];
        assert_eq!(comment_position(&files, &location(Some(1))), Some(right(1)));
    }

    #[test]
    fn accepts_maximum_line_number() {
        let files = [file("@@ -4294967295 +4294967295 @@\n-old\n+new")];
        assert_eq!(
            comment_position(&files, &location(Some(u32::MAX))),
            Some(right(u32::MAX))
        );
    }

    #[test]
    fn rejects_empty_patches_for_line_comments() {
        assert_eq!(comment_position(&[file("")], &location(Some(1))), None);
    }

    #[test]
    fn rejects_invalid_hunk_header_syntax() {
        assert_eq!(Hunk::parse("@@ invalid @@"), None);
    }

    #[test]
    fn rejects_hunk_header_suffixes_without_a_leading_space() {
        let files = [file("@@ -0,0 +1 @@invalid\n+invalid")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_signed_hunk_range_starts() {
        let files = [file("@@ -0,0 ++1 @@\n+invalid")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_hunks_with_two_empty_ranges() {
        assert_eq!(Hunk::parse("@@ -0,0 +1,0 @@"), None);
    }

    #[test]
    fn rejects_nonempty_hunk_ranges_starting_at_zero() {
        assert_eq!(range("0"), None);
    }

    #[test]
    fn rejects_hunk_range_starts_exceeding_u32_maximum() {
        assert_eq!(range("4294967296"), None);
    }

    #[test]
    fn rejects_hunk_ranges_extending_past_u32_maximum() {
        let files = [file("@@ -0,0 +4294967295,2 @@\n+one\n+two")];
        assert_eq!(comment_position(&files, &location(Some(u32::MAX))), None);
    }

    #[test]
    fn rejects_truncated_hunks_even_when_the_target_line_is_present() {
        let files = [file("@@ -0,0 +1,2 @@\n+first")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_truncated_old_ranges_after_finding_the_target_line() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -5,2 +5 @@\n-old\n+new")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_truncated_hunks_before_starting_the_next_hunk() {
        let files = [file("@@ -1 +1,2 @@\n-old\n+new\n@@ -5 +5 @@\n-old\n+new")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_added_rows_exceeding_the_declared_hunk_length() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n+extra")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_deleted_rows_exceeding_the_declared_hunk_length() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n-extra")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_context_rows_exceeding_the_declared_hunk_length() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n extra")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_unprefixed_patch_rows_after_finding_the_target_line() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\ntruncated")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_overlapping_old_line_ranges() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -1 +5 @@\n-old\n+new")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_overlapping_new_line_ranges() {
        let files = [file("@@ -1 +1 @@\n-old\n+new\n@@ -5 +1 @@\n-old\n+new")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_hunks_that_return_to_earlier_line_numbers() {
        let files = [file("@@ -5 +5 @@\n-old\n+new\n@@ -1 +1 @@\n-old\n+new")];
        assert_eq!(comment_position(&files, &location(Some(5))), None);
    }

    #[test]
    fn rejects_no_newline_markers_immediately_after_a_later_hunk_header() {
        let files = [file(
            "@@ -1 +1 @@\n-old\n+new\n@@ -5 +5 @@\n\\ No newline at end of file\n-old\n+new",
        )];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_unsupported_backslash_markers_after_finding_the_target_line() {
        let files = [file("@@ -0,0 +1 @@\n+valid\n\\ unsupported marker")];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }

    #[test]
    fn rejects_consecutive_no_newline_markers() {
        let files = [file(
            "@@ -0,0 +1 @@\n+valid\n\\ No newline at end of file\n\\ No newline at end of file",
        )];
        assert_eq!(comment_position(&files, &location(Some(1))), None);
    }
}
