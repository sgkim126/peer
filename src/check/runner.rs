use std::fmt;

use crate::console::Console;
use crate::context::ReviewContextDigest;
use crate::extract::{ExtractError, Extractor};
use crate::llm::{
    Agent, AgentOutcome, CheckError, CheckOutput, CheckResult, CheckTarget, ExtractToolExecutor,
    Finding, LlmCallError, LlmUsage, ProviderRuntime, RawUsage, request_clarification,
    submit_check_result,
};

use super::CheckDefinition;

pub struct CheckRunConfig {
    pub model: String,
    pub max_iterations: u32,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
    pub context_usage: Option<LlmUsage>,
    pub console: Console,
}

#[derive(Debug)]
#[expect(dead_code)]
pub enum CheckRunError {
    Preparation(ExtractError),
    LlmCall(LlmCallError),
    InvalidOutput(String),
    ClarificationRequested(Vec<String>),
}

impl fmt::Display for CheckRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preparation(e) => write!(f, "failed to prepare check: {e}"),
            Self::LlmCall(e) => e.fmt(f),
            Self::InvalidOutput(e) => write!(f, "invalid check output: {e}"),
            Self::ClarificationRequested(_) => f.write_str("check requested clarification"),
        }
    }
}
impl std::error::Error for CheckRunError {}

pub struct Checker {
    extractor: Extractor,
    runtime: ProviderRuntime,
    config: CheckRunConfig,
}

impl Checker {
    pub fn new(extractor: Extractor, runtime: ProviderRuntime, config: CheckRunConfig) -> Self {
        Self {
            extractor,
            runtime,
            config,
        }
    }

    pub async fn run<C>(
        self,
        check: &C,
        review_context: &ReviewContextDigest,
    ) -> Result<CheckResult, CheckRunError>
    where
        C: CheckDefinition,
    {
        let request = check
            .agent_request(&self.extractor, &self.config.model, review_context)
            .await
            .map_err(CheckRunError::Preparation)?;
        let target = check.target();
        let target_description = match &target {
            CheckTarget::Commit(commit) => commit.to_string(),
            CheckTarget::Range { from, to } => format!("{from}..{to}"),
        };
        self.config.console.debug(format_args!(
            "check {} for {}",
            check.name(),
            target_description
        ));
        let (provider, transport) = self.runtime.into_parts();
        let agent = Agent::new(
            provider,
            transport,
            ExtractToolExecutor::new(self.extractor),
            self.config.console,
        );
        match agent.run_loop(request, self.config.max_iterations).await {
            AgentOutcome::Terminal(done) if done.call.name == submit_check_result().name => {
                let output: CheckOutput = match serde_json::from_value(done.call.arguments) {
                    Ok(output) => output,
                    Err(error) => {
                        let reason = format!("invalid submit_check_result arguments: {error}");
                        return Ok(build_result(
                            check,
                            target,
                            format!("Check did not complete: {reason}"),
                            Vec::new(),
                            done.iterations,
                            done.usage,
                            Some(CheckError::InvalidOutput { reason }),
                            &self.config,
                        ));
                    }
                };
                if !output.findings.iter().all(|finding| {
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
                Ok(build_result(
                    check,
                    target,
                    output.summary,
                    output.findings,
                    done.iterations,
                    done.usage,
                    None,
                    &self.config,
                ))
            }
            AgentOutcome::Terminal(done) if done.call.name == request_clarification().name => {
                let questions = match parse_clarification_questions(done.call.arguments) {
                    Ok(questions) => questions,
                    Err(reason) => {
                        return Ok(build_result(
                            check,
                            target,
                            format!("Check did not complete: {reason}"),
                            Vec::new(),
                            done.iterations,
                            done.usage,
                            Some(CheckError::InvalidOutput { reason }),
                            &self.config,
                        ));
                    }
                };
                Ok(build_result(
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
                    done.iterations,
                    done.usage,
                    Some(CheckError::ClarificationRequired { questions }),
                    &self.config,
                ))
            }
            AgentOutcome::Terminal(done) => {
                let reason = format!("unexpected terminal tool: {}", done.call.name);
                Ok(build_result(
                    check,
                    target,
                    format!("Check did not complete: {reason}"),
                    Vec::new(),
                    done.iterations,
                    done.usage,
                    Some(CheckError::UnexpectedTerminal {
                        tool: done.call.name,
                    }),
                    &self.config,
                ))
            }
            AgentOutcome::Error(failure) => Ok(build_result(
                check,
                target,
                format!("Check did not complete: {}", failure.error),
                Vec::new(),
                failure.checkpoint.iterations,
                failure.usage,
                Some(if failure.exhausted {
                    CheckError::Exhausted {
                        reason: failure.error.to_string(),
                    }
                } else {
                    CheckError::Agent {
                        reason: failure.error.to_string(),
                    }
                }),
                &self.config,
            )),
        }
    }
}

fn parse_clarification_questions(arguments: serde_json::Value) -> Result<Vec<String>, String> {
    #[derive(serde::Deserialize)]
    struct ClarificationArguments {
        questions: Vec<String>,
    }

    let arguments: ClarificationArguments = serde_json::from_value(arguments)
        .map_err(|error| format!("invalid request_clarification arguments: {error}"))?;
    if arguments.questions.is_empty() {
        return Err(
            "invalid request_clarification arguments: questions must not be empty".to_string(),
        );
    }
    Ok(arguments.questions)
}

#[allow(clippy::too_many_arguments)]
fn build_result<C>(
    check: &C,
    target: CheckTarget,
    summary: String,
    findings: Vec<Finding>,
    iterations: u32,
    usage: RawUsage,
    error: Option<CheckError>,
    config: &CheckRunConfig,
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
        context_usage: config.context_usage.clone(),
        usage: LlmUsage::from_raw_usage(
            usage,
            &config.model,
            config.input_per_1m_usd,
            config.output_per_1m_usd,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn parses_non_empty_clarification_questions() {
        assert_eq!(
            parse_clarification_questions(json!({
                "questions": ["Which deployment policy applies?"]
            }))
            .unwrap(),
            ["Which deployment policy applies?"]
        );
    }

    #[test]
    fn rejects_empty_clarification_questions() {
        assert_eq!(
            parse_clarification_questions(json!({ "questions": [] })).unwrap_err(),
            "invalid request_clarification arguments: questions must not be empty"
        );
    }
}
