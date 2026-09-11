use serde::Deserialize;
use serde_json::{Value, json};

use crate::stage::FileLocation;

#[derive(Deserialize)]
pub struct ChangedFile {
    pub filename: String,
    pub previous_filename: Option<String>,
}

pub fn comment_position(files: &[ChangedFile], location: &FileLocation) -> Option<Value> {
    if location.file.is_empty() {
        return None;
    }
    let file = files.iter().find(|file| {
        file.filename == location.file || file.previous_filename.as_deref() == Some(&location.file)
    })?;
    Some(json!({
        "path": file.filename,
        "subject_type": "file"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_comments_follow_renamed_paths() {
        let files = vec![ChangedFile {
            filename: "new.png".into(),
            previous_filename: Some("old.png".into()),
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
}
