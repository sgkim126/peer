pub mod runner;
mod size;

use std::fmt;
use std::path::PathBuf;

use crate::cli::CheckCommand;
use crate::config::Config;
use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::llm::checks::runner::{CheckRunConfig, CheckRunError, run_check};
use crate::llm::checks::size::SizeCheck;
use crate::llm::confidence::{Confidence, ConfidenceError};
use crate::llm::provider::{ConversationTurn, ProviderCreationError, ToolSpec, create_provider};
use crate::llm::result::{
    CheckOutput, CheckResult, CheckTarget, validate_per_commit_targets, validate_range_targets,
};
use crate::llm::tool_executor::PeerToolExecutor;

#[derive(Debug)]
pub enum CheckCommandError {
    Config(crate::error::PeerError),
    InvalidConfidence(ConfidenceError),
    Provider(ProviderCreationError),
    Run(CheckRunError),
}

impl From<crate::error::PeerError> for CheckCommandError {
    fn from(err: crate::error::PeerError) -> Self {
        Self::Config(err)
    }
}

impl From<ConfidenceError> for CheckCommandError {
    fn from(err: ConfidenceError) -> Self {
        Self::InvalidConfidence(err)
    }
}

impl From<ProviderCreationError> for CheckCommandError {
    fn from(err: ProviderCreationError) -> Self {
        Self::Provider(err)
    }
}

impl From<CheckRunError> for CheckCommandError {
    fn from(err: CheckRunError) -> Self {
        Self::Run(err)
    }
}

impl fmt::Display for CheckCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(f),
            Self::InvalidConfidence(error) => error.fmt(f),
            Self::Provider(error) => error.fmt(f),
            Self::Run(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CheckCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::InvalidConfidence(error) => Some(error),
            Self::Provider(error) => Some(error),
            Self::Run(error) => Some(error),
        }
    }
}

pub async fn handler(
    console: Console,
    command: CheckCommand,
    config: Config,
    project_root: PathBuf,
) -> Result<CheckResult, CheckCommandError> {
    match command {
        CheckCommand::Size { revision } => {
            run_definition(SizeCheck::new(revision), console, &config, project_root).await
        }
        CheckCommand::Intent { .. } => unimplemented!(),
        CheckCommand::Quality { .. } => unimplemented!(),
        CheckCommand::Security { .. } => unimplemented!(),
        CheckCommand::Coherence { .. } => unimplemented!(),
    }
}

async fn run_definition<C>(
    check: C,
    console: Console,
    config: &Config,
    project_root: PathBuf,
) -> Result<CheckResult, CheckCommandError>
where
    C: CheckDefinition,
{
    let provider_name = config.llm.default_provider.clone();
    let model_name = config.llm.default_model.clone();
    let confidence_threshold = Confidence::try_from(config.llm.confidence_threshold)?;
    let max_iterations = config.llm.max_iterations;
    let (provider_config, model_config) = config.resolve_provider(&provider_name, &model_name)?;

    let provider = create_provider(
        &provider_config.name,
        &provider_config.api_key_env,
        provider_config.base_url.as_deref(),
    )?;
    let extractor = Extractor::new(project_root.clone(), console);
    let tool_executor = PeerToolExecutor::new(Extractor::new(project_root, console));
    let run_config = CheckRunConfig {
        model: &model_config.name,
        confidence_threshold,
        max_iterations,
        input_per_1m_usd: model_config.input_per_1m_usd,
        output_per_1m_usd: model_config.output_per_1m_usd,
        console,
    };

    Ok(run_check(&check, &extractor, &provider, &tool_executor, run_config).await?)
}

/// Inputs prepared before the agent loop starts.
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
pub trait CheckDefinition {
    /// Returns the stable name written to `CheckResult::check`.
    fn name(&self) -> &'static str;

    /// Loads required data and builds the initial agent inputs.
    async fn prepare(&self, extractor: &Extractor) -> Result<PreparedCheck, ExtractError>;
}

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
