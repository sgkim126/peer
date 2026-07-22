use std::fmt;

use crate::console::Console;
use crate::extract::{ExtractError, Extractor};
use crate::llm::agent::{Agent, AgentOutcome};
use crate::llm::context::ReviewContext;
use crate::llm::provider::{LlmCallError, ProviderRuntime};
use crate::llm::result::{CheckResult, CheckUsage};
use crate::llm::tools::ExtractToolExecutor;

use super::CheckDefinition;

pub struct CheckRunConfig {
    pub model: String,
    pub max_iterations: u32,
    pub input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
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
        review_context: &ReviewContext,
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
            crate::llm::result::CheckTarget::Commit(commit) => commit.to_string(),
            crate::llm::result::CheckTarget::Range(range) => range.clone(),
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
            AgentOutcome::Completed(done) => {
                if !done.output.findings.iter().all(|finding| {
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
                    done.output.summary,
                    done.output.findings,
                    done.iterations,
                    done.usage,
                    false,
                    None,
                    &self.config,
                ))
            }
            AgentOutcome::ClarificationRequested(request) => Ok(build_result(
                check,
                target,
                format!(
                    "Checker asks:\n{}",
                    request
                        .questions
                        .iter()
                        .map(|question| format!("- {question}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
                Vec::new(),
                request.iterations,
                request.usage,
                true,
                Some("clarification required".to_string()),
                &self.config,
            )),
            AgentOutcome::Error(failure) => Ok(build_result(
                check,
                target,
                format!("Check did not complete: {}", failure.error),
                Vec::new(),
                failure.iterations,
                failure.usage,
                true,
                Some(failure.error.to_string()),
                &self.config,
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_result<C>(
    check: &C,
    target: crate::llm::result::CheckTarget,
    summary: String,
    findings: Vec<crate::llm::result::Finding>,
    iterations: u32,
    usage: crate::llm::provider::RawUsage,
    is_exhausted: bool,
    exhaustion_reason: Option<String>,
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
        is_exhausted,
        exhaustion_reason,
        usage: CheckUsage::from_raw_usage(
            usage,
            &config.model,
            config.input_per_1m_usd,
            config.output_per_1m_usd,
        ),
    }
}
