use std::fmt;

use serde::Deserialize;

use crate::cache::{CacheKey, CacheKeyError};
use crate::console::Console;
use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::llm::{CheckError, CheckResult, CheckTarget, ConversationTurn, Finding, LlmUsage};
use crate::pi::{
    CheckKind, ModelRef, ModelRefError, Operation, PiRunError, PiRunRequest, PiRuntime, ReadTool,
    RunConfig, TerminalTool, tool_contract_digest,
};

use super::CheckDefinition;

pub struct CheckRunConfig {
    pub model: ModelRef,
    pub max_iterations: u32,
    pub context_usage: Option<LlmUsage>,
    pub session_key: CacheKey,
    pub resume: bool,
    pub console: Console,
}

#[derive(Debug)]
pub enum CheckRunError {
    Preparation(ExtractError),
    Pi(PiRunError),
    CacheKey(CacheKeyError),
    InvalidModel(ModelRefError),
    InvalidRequest(String),
    InvalidOutput(String),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preparation(e) => write!(f, "failed to prepare check: {e}"),
            Self::Pi(e) => e.fmt(f),
            Self::CacheKey(e) => write!(f, "cannot build Pi session cache key: {e}"),
            Self::InvalidModel(e) => write!(f, "invalid Pi model: {e}"),
            Self::InvalidRequest(e) => write!(f, "invalid check request: {e}"),
            Self::InvalidOutput(e) => write!(f, "invalid check output: {e}"),
        }
    }
}
impl std::error::Error for CheckRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Preparation(error) => Some(error),
            Self::Pi(error) => Some(error),
            Self::CacheKey(error) => Some(error),
            Self::InvalidModel(error) => Some(error),
            Self::InvalidRequest(_) => None,
            Self::InvalidOutput(_) => None,
        }
    }
}

impl From<CacheKeyError> for CheckRunError {
    fn from(error: CacheKeyError) -> Self {
        Self::CacheKey(error)
    }
}

impl From<ModelRefError> for CheckRunError {
    fn from(error: ModelRefError) -> Self {
        Self::InvalidModel(error)
    }
}

pub struct Checker {
    extractor: Extractor,
    config: CheckRunConfig,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum CheckOutcome {
    CheckResult {
        #[serde(default)]
        summary: String,
        findings: Vec<Finding>,
    },
    Clarification {
        questions: Vec<String>,
    },
}

impl Checker {
    pub fn new(extractor: Extractor, config: CheckRunConfig) -> Self {
        Self { extractor, config }
    }

    pub async fn run<C>(
        self,
        runtime: &mut PiRuntime,
        check: &C,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckResult, CheckRunError>
    where
        C: CheckDefinition,
    {
        let request = check
            .agent_request(&self.extractor, self.config.model.model(), review_context)
            .await
            .map_err(CheckRunError::Preparation)?;
        let target = check.target();
        self.config
            .console
            .debug(format_args!("check {} for {target}", check.name()));
        let (system_prompt, prompt) = prompts(request.conversation)?;
        let read_tools = request
            .tools
            .into_iter()
            .map(|tool| read_tool(&tool.name))
            .collect::<Result<Vec<_>, _>>()?;
        let terminal_tools = request
            .terminal_tools
            .into_iter()
            .map(|tool| terminal_tool(&tool.name))
            .collect::<Result<Vec<_>, _>>()?;
        let result = runtime
            .run(PiRunRequest {
                session_key: self.config.session_key.clone(),
                config: RunConfig {
                    tool_contract_digest: tool_contract_digest(),
                    operation: Operation::Check {
                        check: check_kind(check.name())?,
                        target: target.to_string(),
                        expected_commits: check.expected_commits().to_vec(),
                    },
                    system_prompt,
                    read_tools,
                    terminal_tools,
                    max_turns: self.config.max_iterations,
                },
                model: self.config.model.clone(),
                prompt,
                resume: self.config.resume,
            })
            .await;
        let result = match result {
            Ok(result) => result,
            Err(PiRunError::Exhausted { turns, usage }) => {
                let reason = format!("Pi did not submit an outcome within {turns} turns");
                return Ok(self.build_result(
                    check,
                    target,
                    format!("Check did not complete: {reason}"),
                    Vec::new(),
                    turns,
                    usage,
                    Some(CheckError::Exhausted { reason }),
                ));
            }
            Err(error) => return Err(CheckRunError::Pi(error)),
        };
        let outcome: CheckOutcome = serde_json::from_value(result.outcome)
            .map_err(|error| CheckRunError::InvalidOutput(error.to_string()))?;
        match outcome {
            CheckOutcome::CheckResult { summary, findings } => {
                if !findings.iter().all(|finding| {
                    // Expected commits are full hashes produced while resolving the check
                    // target, but a finding may report an abbreviated commit hash.
                    check
                        .expected_commits()
                        .iter()
                        .any(|expected| expected.matches(&finding.commit))
                }) {
                    return Err(CheckRunError::InvalidOutput(
                        "finding commit is outside the check target".to_string(),
                    ));
                }
                Ok(self.build_result(
                    check,
                    target,
                    summary,
                    findings,
                    result.iterations,
                    result.usage,
                    None,
                ))
            }
            CheckOutcome::Clarification { questions } => {
                validate_questions(&questions).map_err(CheckRunError::InvalidOutput)?;
                Ok(self.build_result(
                    check,
                    target,
                    format!(
                        "Checker asks:\n{}",
                        questions
                            .iter()
                            .map(|question| format!("- {question}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    Vec::new(),
                    result.iterations,
                    result.usage,
                    Some(CheckError::ClarificationRequired { questions }),
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_result<C>(
        &self,
        check: &C,
        target: CheckTarget,
        summary: String,
        findings: Vec<Finding>,
        iterations: u32,
        usage: LlmUsage,
        error: Option<CheckError>,
    ) -> CheckResult
    where
        C: CheckDefinition,
    {
        CheckResult {
            check: check.name().to_string(),
            target,
            ordered_commits: check.expected_commits().to_vec(),
            summary,
            findings,
            iterations,
            error,
            context_usage: self.config.context_usage.clone(),
            usage,
        }
    }
}

fn prompts(conversation: Vec<ConversationTurn>) -> Result<(String, String), CheckRunError> {
    let mut system_prompt = None;
    let mut user_prompts = Vec::new();
    for turn in conversation {
        match turn {
            ConversationTurn::System(prompt) if system_prompt.is_none() => {
                system_prompt = Some(prompt);
            }
            ConversationTurn::User(prompt) => user_prompts.push(prompt),
            ConversationTurn::System(_) => {
                return Err(CheckRunError::InvalidRequest(
                    "multiple system prompts".to_string(),
                ));
            }
            ConversationTurn::AssistantToolCalls(_) | ConversationTurn::ToolResult { .. } => {
                return Err(CheckRunError::InvalidRequest(
                    "tool history is not supported".to_string(),
                ));
            }
        }
    }
    let system_prompt = system_prompt
        .filter(|prompt| !prompt.trim().is_empty())
        .ok_or_else(|| CheckRunError::InvalidRequest("missing system prompt".to_string()))?;
    if user_prompts.is_empty() {
        return Err(CheckRunError::InvalidRequest(
            "missing user prompt".to_string(),
        ));
    }
    Ok((system_prompt, user_prompts.join("\n\n")))
}

fn validate_questions(questions: &[String]) -> Result<(), String> {
    if questions.is_empty() {
        return Err("clarification questions must not be empty".to_string());
    }
    if questions.iter().any(|question| question.trim().is_empty()) {
        return Err("clarification questions must not contain blank values".to_string());
    }
    Ok(())
}

fn check_kind(name: &str) -> Result<CheckKind, CheckRunError> {
    match name {
        "size" => Ok(CheckKind::Size),
        "intent" => Ok(CheckKind::Intent),
        "quality" => Ok(CheckKind::Quality),
        "security" => Ok(CheckKind::Security),
        "coherence" => Ok(CheckKind::Coherence),
        _ => Err(CheckRunError::InvalidRequest(format!(
            "unknown check kind: {name}"
        ))),
    }
}

fn read_tool(name: &str) -> Result<ReadTool, CheckRunError> {
    match name {
        "get_commit_message" => Ok(ReadTool::GetCommitMessage),
        "get_commit_diff" => Ok(ReadTool::GetCommitDiff),
        "get_changed_files" => Ok(ReadTool::GetChangedFiles),
        "get_commits_in_range" => Ok(ReadTool::GetCommitsInRange),
        "get_file_content" => Ok(ReadTool::GetFileContent),
        "get_file_diff" => Ok(ReadTool::GetFileDiff),
        "list_tree" => Ok(ReadTool::ListTree),
        "grep" => Ok(ReadTool::Grep),
        _ => Err(CheckRunError::InvalidRequest(format!(
            "unknown read tool: {name}"
        ))),
    }
}

fn terminal_tool(name: &str) -> Result<TerminalTool, CheckRunError> {
    match name {
        "submit_check_result" => Ok(TerminalTool::SubmitCheckResult),
        "request_clarification" => Ok(TerminalTool::RequestClarification),
        _ => Err(CheckRunError::InvalidRequest(format!(
            "unknown terminal tool: {name}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    #[test]
    fn separates_the_system_prompt_from_user_input() {
        let (system, prompt) = prompts(vec![
            ConversationTurn::System("Review code.".to_string()),
            ConversationTurn::User("Input".to_string()),
            ConversationTurn::User("Context".to_string()),
        ])
        .unwrap();

        assert_eq!(system, "Review code.");
        assert_eq!(prompt, "Input\n\nContext");
    }

    #[test]
    fn rejects_invalid_requests() {
        let Err(CheckRunError::InvalidRequest(message)) =
            prompts(vec![ConversationTurn::User("Input".to_string())])
        else {
            panic!("missing system prompt must be rejected");
        };

        assert_eq!(message, "missing system prompt");
    }

    #[test]
    fn validates_clarification_questions() {
        assert_matches!(validate_questions(&["Which behavior?".to_string()]), Ok(_));
        assert_matches!(validate_questions(&[]), Err(_));
        assert_matches!(validate_questions(&["  ".to_string()]), Err(_));
    }

    #[test]
    fn maps_prepared_tool_names_to_the_protocol() {
        assert_eq!(read_tool("grep").unwrap(), ReadTool::Grep);
        assert_eq!(
            terminal_tool("submit_check_result").unwrap(),
            TerminalTool::SubmitCheckResult
        );
    }
}
