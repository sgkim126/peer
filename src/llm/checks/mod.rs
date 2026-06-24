use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::provider::{ConversationTurn, ToolSpec};
use crate::llm::result::{
    CheckOutput, CheckTarget, validate_per_commit_targets, validate_range_targets,
};

/// Inputs prepared before the agent loop starts.
#[allow(dead_code)]
pub struct PreparedCheck {
    pub conversation: Vec<ConversationTurn>,
    pub tools: Vec<ToolSpec>,
    pub output_schema: serde_json::Value,
    pub target: PreparedCheckTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PreparedCheckTarget {
    Commit(CommitHash),
    Range {
        revision: String,
        commits: Vec<CommitHash>,
    },
}

#[allow(dead_code)]
impl PreparedCheck {
    pub fn result_target(&self) -> CheckTarget {
        match &self.target {
            PreparedCheckTarget::Commit(commit) => CheckTarget::Commit(commit.clone()),
            PreparedCheckTarget::Range { revision, .. } => CheckTarget::Range(revision.clone()),
        }
    }

    pub fn validate_output(&self, output: &CheckOutput) -> Result<(), String> {
        match &self.target {
            PreparedCheckTarget::Commit(commit) => {
                validate_per_commit_targets(&output.findings, commit)
            }
            PreparedCheckTarget::Range { commits, .. } => {
                validate_range_targets(&output.findings, commits)
            }
        }
    }
}

/// Defines the provider-neutral inputs and validation rules for an LLM check.
#[allow(dead_code)]
pub trait CheckDefinition {
    /// Returns the stable name written to `CheckResult::check`.
    fn name(&self) -> &'static str;

    /// Loads required data and builds the initial agent inputs.
    async fn prepare(&self, extractor: &Extractor) -> Result<PreparedCheck, ExtractError>;
}

#[allow(dead_code)]
fn all_tools() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "get_commit_message".to_string(),
            description: "Returns the full commit message for a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_commit_diff".to_string(),
            description: "Returns the full unified diff for a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_changed_files".to_string(),
            description: "Returns the files changed in a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision resolving to a commit."
                    }
                },
                "required": ["revision"]
            }),
        },
        ToolSpec {
            name: "get_commits_in_range".to_string(),
            description: "Returns commit hashes in a two-dot range, oldest to newest.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "range": {
                        "type": "string",
                        "description": "Git two-dot range."
                    }
                },
                "required": ["range"]
            }),
        },
        ToolSpec {
            name: "get_file_content".to_string(),
            description: "Returns a file's content at a commit.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "revision": {
                        "type": "string",
                        "description": "Git revision at which to read the file."
                    },
                    "path": {
                        "type": "string",
                        "description": "Repository-root-relative path."
                    }
                },
                "required": ["path", "revision"]
            }),
        },
    ]
}

#[allow(dead_code)]
fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "summary": {
                "type": "string",
                "description": "One-sentence summary of the check result."
            },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "commit": {
                            "type": "string"
                        },
                        "severity": {
                            "type": "string",
                            "enum": ["info", "low", "medium", "high", "critical"]
                        },
                        "message": {
                            "type": "string"
                        },
                        "file": {
                            "type": "string"
                        },
                        "line": {
                            "type": "integer",
                            "minimum": 1
                        }
                    },
                    "required": ["commit", "severity", "message"]
                }
            },
            "confidence": {
                "type": "number",
                "minimum": 0.0,
                "maximum": 1.0
            }
        },
        "required": ["summary", "findings", "confidence"]
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use std::path::PathBuf;

    use crate::console::Console;
    use crate::llm::confidence::Confidence;
    use crate::llm::result::{Finding, Severity};

    struct TestCheck {
        target: CommitHash,
    }

    impl CheckDefinition for TestCheck {
        fn name(&self) -> &'static str {
            "test"
        }

        async fn prepare(&self, _extractor: &Extractor) -> Result<PreparedCheck, ExtractError> {
            Ok(PreparedCheck {
                conversation: vec![
                    ConversationTurn::System(format!("Review commit {}.", self.target)),
                    ConversationTurn::User("Commit message:\nAdd check preparation".to_string()),
                ],
                tools: vec![ToolSpec {
                    name: "get_commit_diff".to_string(),
                    description: "Read a commit diff.".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "hash": {
                                "type": "string"
                            }
                        },
                        "required": ["hash"]
                    }),
                }],
                output_schema: json!({
                    "type": "object"
                }),
                target: PreparedCheckTarget::Commit(self.target.clone()),
            })
        }
    }

    fn output(commit: &str) -> CheckOutput {
        CheckOutput {
            summary: "summary".to_string(),
            findings: vec![Finding {
                commit: CommitHash::new(commit).unwrap(),
                severity: Severity::Info,
                message: "message".to_string(),
                location: None,
            }],
            confidence: Confidence::try_from(0.9).unwrap(),
        }
    }

    #[tokio::test]
    async fn check_definition_prepares_required_inputs_before_agent_loop() {
        let target = CommitHash::new("abc1234").unwrap();
        let check = TestCheck {
            target: target.clone(),
        };
        let extractor = Extractor::new(PathBuf::from("/project"), Console::default());

        assert_eq!(check.name(), "test");

        let prepared = check.prepare(&extractor).await.unwrap();
        assert_eq!(
            prepared.conversation,
            vec![
                ConversationTurn::System("Review commit abc1234.".to_string()),
                ConversationTurn::User("Commit message:\nAdd check preparation".to_string()),
            ]
        );
        assert_eq!(prepared.tools[0].name, "get_commit_diff");
        assert_eq!(
            prepared.output_schema,
            json!({
                "type": "object"
            })
        );
        assert_eq!(prepared.result_target(), CheckTarget::Commit(target));
    }

    #[test]
    fn prepared_check_owns_target_validation() {
        let prepared = PreparedCheck {
            conversation: Vec::new(),
            tools: Vec::new(),
            output_schema: json!({}),
            target: PreparedCheckTarget::Commit(CommitHash::new("abc1234").unwrap()),
        };

        assert!(prepared.validate_output(&output("abc1234")).is_ok());
        assert!(prepared.validate_output(&output("def5678")).is_err());
    }

    #[test]
    fn prepared_range_check_validates_against_loaded_commits() {
        let prepared = PreparedCheck {
            conversation: Vec::new(),
            tools: Vec::new(),
            output_schema: json!({}),
            target: PreparedCheckTarget::Range {
                revision: "HEAD~2..HEAD".to_string(),
                commits: vec![
                    CommitHash::new("abc1234").unwrap(),
                    CommitHash::new("def5678").unwrap(),
                ],
            },
        };

        assert!(prepared.validate_output(&output("def5678")).is_ok());
        assert!(prepared.validate_output(&output("9876abc")).is_err());
        assert_eq!(
            prepared.result_target(),
            CheckTarget::Range("HEAD~2..HEAD".to_string())
        );
    }
}
