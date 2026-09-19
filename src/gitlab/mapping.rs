use std::collections::{BTreeMap, HashSet};

use crate::context::{
    ReviewCommentLocation, ReviewCommentThread, ReviewContext, ReviewThreadComment,
};

use super::CONVERSATION_MARKER;
use super::client::{Discussion, MergeRequest, Note};

pub fn review_context(merge_request: MergeRequest, discussions: Vec<Discussion>) -> ReviewContext {
    // Merge repeated discussion entries before sorting and filtering. A reply
    // must survive even when its root note has been removed or is our summary.
    let mut groups: BTreeMap<String, Vec<Note>> = BTreeMap::new();
    for discussion in discussions {
        groups
            .entry(discussion.id)
            .or_default()
            .extend(discussion.notes);
    }
    let mut threads = Vec::new();
    for (id, mut notes) in groups {
        notes.retain(|note| !note.system && !note.internal && !note.confidential);
        notes
            .sort_by(|left, right| (&left.created_at, left.id).cmp(&(&right.created_at, right.id)));
        let mut seen = HashSet::new();
        notes.retain(|note| seen.insert(note.id));
        let Some(root) = notes.first() else { continue };
        let sort_key = (root.created_at.clone(), id);
        let mut commit = root.commit_id.clone();
        let location = root.position.as_ref().and_then(|position| {
            commit = position.head_sha.clone().or(commit.clone());
            let new_path = position.new_path.as_ref().filter(|path| !path.is_empty());
            let old_path = position.old_path.as_ref().filter(|path| !path.is_empty());
            if position.position_type == "text"
                && position.head_sha.is_some()
                && position.new_line.is_some()
                && let Some(path) = new_path
            {
                return Some(ReviewCommentLocation {
                    path: path.clone(),
                    line: position.new_line,
                });
            }
            // Old-side, file-level, image, and incomplete positions cannot be
            // represented as a line on the head tree. Retain their path only.
            let path = if position.old_line.is_some() && position.new_line.is_none() {
                // The old path may have been renamed or deleted from the head
                // tree. Do not pair that path with the head commit.
                commit = None;
                old_path.or(new_path)
            } else {
                new_path.or(old_path)
            }?;
            Some(ReviewCommentLocation {
                path: path.clone(),
                line: None,
            })
        });
        let comments: Vec<_> = notes
            .into_iter()
            .filter(|note| {
                !note
                    .body
                    .lines()
                    .any(|line| line.trim() == CONVERSATION_MARKER)
            })
            .map(|note| ReviewThreadComment {
                author: note
                    .author
                    .map_or_else(|| "unknown".into(), |author| author.username),
                body: note.body,
            })
            .collect();
        if !comments.is_empty() {
            threads.push((
                sort_key,
                ReviewCommentThread {
                    commit,
                    location,
                    comments,
                },
            ));
        }
    }
    threads.sort_by(|left, right| left.0.cmp(&right.0));
    ReviewContext {
        title: Some(merge_request.title),
        body: Some(merge_request.description.unwrap_or_default()),
        comments: threads.into_iter().map(|(_, thread)| thread).collect(),
    }
}

#[cfg(test)]
mod tests;
