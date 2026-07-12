use serde::Serialize;

use crate::cache::CacheKey;
use crate::extract::{CommitList, CommitMessage, ExtractError, Extractor};
use crate::llm::context::ReviewContext;
use crate::llm::provider::ConversationTurn;

use super::tools;
use super::{CheckDefinition, PreparedCheck, PreparedCheckTarget, output_schema, system_prompt};

const SYSTEM_PROMPT: &str = r#"You are reviewing a commit series for coherence.

Your scope is the relationships between commits and the structure of the series, not the
quality or correctness of any individual commit. Assess whether:
1. Commits are ordered so that their dependencies and narrative flow are clear.
2. A later commit is merely a fixup, follow-up, revert, or reintroduction of an earlier
   commit and should be squashed, reordered, or otherwise consolidated.
3. One logical change is split across commits in a way that makes the series difficult to
   review, bisect, or integrate, or unrelated responsibilities are mixed across the series.
4. The sequence contains unnecessary backtracking, duplication, or a confusing handoff of
   responsibility between commits.
5. Commit messages, considered as a sequence, clearly communicate the progression of the
   work.

Do not report code correctness, bugs, style, tests, error handling, security issues, or
whether an individual commit message accurately describes its own diff; these are outside the
scope of this check. Do not report an issue solely because an individual commit may not build or
pass tests. Report such a concern only when the dependency or ordering between commits is the
coherence problem.

The required commit list is ordered from oldest to newest. Findings must reference the
specific commit responsible for the series-level issue. Use a diff or changed-file list only
when needed to establish a relationship between commits. Return no findings when the series is
well structured and its commits form a clear, coherent progression."#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoherenceCheck {
    commit_list: CommitList,
}

impl CoherenceCheck {
    pub async fn try_new(range: &str, extractor: &Extractor) -> Result<Self, ExtractError> {
        Ok(Self {
            commit_list: extractor.commit_list(range).await?,
        })
    }
}

impl CheckDefinition for CoherenceCheck {
    fn name(&self) -> &'static str {
        "coherence"
    }

    fn cache_key(&self, provider: &str, model: &str, review_context: &ReviewContext) -> CacheKey {
        let params = CoherenceCheckCacheParams {
            commit_list: &self.commit_list,
            review_context,
        };

        CacheKey::from_params(self.name(), provider, model, &params)
            .expect("serializing coherence check cache params cannot fail")
    }

    async fn prepare(
        &self,
        extractor: &Extractor,
        review_context: &ReviewContext,
    ) -> Result<PreparedCheck, ExtractError> {
        let mut messages = Vec::with_capacity(self.commit_list.commits.len());

        for commit in &self.commit_list.commits {
            messages.push(extractor.commit_message(commit.as_ref()).await?);
        }

        Ok(build_prepared_check(
            self.commit_list.clone(),
            messages,
            review_context,
        ))
    }
}

#[derive(Debug, Serialize)]
struct CoherenceCheckCacheParams<'a> {
    commit_list: &'a CommitList,
    review_context: &'a ReviewContext,
}

fn build_prepared_check(
    commit_list: CommitList,
    messages: Vec<CommitMessage>,
    review_context: &ReviewContext,
) -> PreparedCheck {
    let commits = commit_list.commits;
    let ordered_commits = messages
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            format!(
                "{}. {}\n{}",
                index + 1,
                message.hash,
                indent(&message.message)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut user_prompt = format!(
        "Review range {}.\n\nCommits (oldest to newest):\n{}",
        commit_list.range, ordered_commits
    );
    review_context.append_to_prompt(&mut user_prompt);

    PreparedCheck {
        conversation: vec![
            ConversationTurn::System(system_prompt(SYSTEM_PROMPT)),
            ConversationTurn::User(user_prompt),
        ],
        tools: vec![
            tools::get_commit_diff(),
            tools::get_changed_files(),
            tools::request_user_info(),
        ],
        output_schema: output_schema(),
        target: PreparedCheckTarget::Range {
            revision: commit_list.range,
            commits,
        },
    }
}

fn indent(value: &str) -> String {
    value
        .lines()
        .map(|line| format!("   {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitHash;
    use crate::llm::result::{CheckOutput, CheckTarget};

    fn hash(value: &str) -> CommitHash {
        CommitHash::new(value).unwrap()
    }

    fn prepared_check() -> PreparedCheck {
        build_prepared_check(
            CommitList {
                range: "base123..tip4567".to_string(),
                commits: vec![hash("abc1234"), hash("def5678")],
            },
            vec![
                CommitMessage {
                    hash: hash("abc1234"),
                    message: "Add parser".to_string(),
                },
                CommitMessage {
                    hash: hash("def5678"),
                    message: "Fix parser edge case".to_string(),
                },
            ],
            &ReviewContext::default(),
        )
    }

    #[test]
    fn name_is_coherence() {
        let check = CoherenceCheck {
            commit_list: CommitList {
                range: "HEAD~2..HEAD".to_string(),
                commits: vec![hash("abc1234"), hash("def5678")],
            },
        };

        assert_eq!(check.name(), "coherence");
    }

    #[test]
    fn prompt_contains_numbered_oldest_to_newest_commits() {
        let prepared = prepared_check();

        let ConversationTurn::System(system) = &prepared.conversation[0] else {
            panic!("expected system prompt");
        };
        assert!(system.contains("commit series for coherence"));
        assert!(system.contains("Tool use is optional"));

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        let first = user.find("1. abc1234").unwrap();
        let second = user.find("2. def5678").unwrap();
        assert!(first < second);
        assert!(user.contains("Add parser"));
        assert!(user.contains("Fix parser edge case"));
        assert!(!user.contains("Review context:"));
    }

    #[test]
    fn prepared_conversation_contains_review_context_when_present() {
        let prepared = build_prepared_check(
            CommitList {
                range: "base123..tip4567".to_string(),
                commits: vec![hash("abc1234"), hash("def5678")],
            },
            vec![
                CommitMessage {
                    hash: hash("abc1234"),
                    message: "Add parser".to_string(),
                },
                CommitMessage {
                    hash: hash("def5678"),
                    message: "Fix parser edge case".to_string(),
                },
            ],
            &ReviewContext {
                title: Some("Parser cleanup series".to_string()),
                body_summary: Some("Series should remain bisectable.".to_string()),
                comments_summary: None,
            },
        );

        let ConversationTurn::User(user) = &prepared.conversation[1] else {
            panic!("expected required data");
        };
        assert!(user.contains("Review context:"));
        assert!(user.contains("Title:\nParser cleanup series"));
        assert!(user.contains("Body summary:\nSeries should remain bisectable."));
    }

    #[test]
    fn prepared_check_preserves_original_range_and_commits() {
        let prepared = prepared_check();

        assert_eq!(
            prepared.result_target(),
            CheckTarget::Range("base123..tip4567".to_string())
        );
        assert_eq!(prepared.tools.len(), 3);
        assert_eq!(prepared.tools[0].name, "get_commit_diff");
        assert_eq!(prepared.tools[1].name, "get_changed_files");
        assert_eq!(prepared.tools[2].name, "request_user_info");
        assert_eq!(
            prepared.output_schema["required"],
            serde_json::json!(["findings"])
        );
    }

    #[test]
    fn prepared_check_validates_findings_against_range_commits() {
        let prepared = prepared_check();
        let in_range: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "fixup should be squashed",
            "findings": [{
                "commit": "def5678",
                "severity": "medium",
                "message": "This commit only fixes the immediately preceding commit."
            }]
        }))
        .unwrap();
        let outside_range: CheckOutput = serde_json::from_value(serde_json::json!({
            "summary": "invalid target",
            "findings": [{
                "commit": "9876abc",
                "severity": "medium",
                "message": "Outside the reviewed range."
            }]
        }))
        .unwrap();

        assert!(prepared.validate_output(&in_range).is_ok());
        assert!(prepared.validate_output(&outside_range).is_err());
    }
}
