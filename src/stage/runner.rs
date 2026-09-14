use std::fmt;

use log::{debug, trace};
use serde::{Deserialize, Serialize};

use crate::cache::{CacheKey, CacheKeyError, CacheStore};
use crate::llm::LlmUsage;
use crate::pi::{
    ModelRef, Operation, PiRunError, PiRunFailure, PiRunRequest, PiRuntime, RunConfig,
    StageKind as PiStageKind, TerminalTool, tool_contract_digest,
};
use crate::stage::contract::{
    ClarificationQuestion, ReviewStage, StageKind, StageOutcome, StageRequest, StageRun,
};

pub struct StageRunConfig {
    pub model: ModelRef,
    pub max_iterations: u32,
    pub resume: bool,
}

#[derive(Debug)]
pub enum StageRunError {
    CacheKey(CacheKeyError),
    Pi(Box<PiRunFailure>),
    InvalidOutput {
        source: serde_json::Error,
        usage: LlmUsage,
    },
    InvalidQuestions {
        reason: String,
        usage: LlmUsage,
    },
    InvalidReport {
        reason: String,
        usage: LlmUsage,
    },
}

impl StageRunError {
    pub fn usage(&self) -> Option<&LlmUsage> {
        match self {
            Self::CacheKey(_) => None,
            Self::Pi(failure) => failure.usage.as_deref(),
            Self::InvalidOutput { usage, .. } => Some(usage),
            Self::InvalidQuestions { usage, .. } => Some(usage),
            Self::InvalidReport { usage, .. } => Some(usage),
        }
    }
}

impl fmt::Display for StageRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CacheKey(error) => write!(f, "cannot build stage cache key: {error}"),
            Self::Pi(error) => error.fmt(f),
            Self::InvalidOutput { source, .. } => {
                write!(f, "invalid typed stage output: {source}")
            }
            Self::InvalidQuestions { reason, .. } => {
                write!(f, "invalid clarification questions: {reason}")
            }
            Self::InvalidReport { reason, .. } => {
                write!(f, "invalid typed stage report: {reason}")
            }
        }
    }
}

impl std::error::Error for StageRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CacheKey(error) => Some(error),
            Self::Pi(error) => Some(error),
            Self::InvalidOutput { source, .. } => Some(source),
            Self::InvalidQuestions { .. } => None,
            Self::InvalidReport { .. } => None,
        }
    }
}

impl From<CacheKeyError> for StageRunError {
    fn from(error: CacheKeyError) -> Self {
        Self::CacheKey(error)
    }
}

impl From<PiRunFailure> for StageRunError {
    fn from(failure: PiRunFailure) -> Self {
        Self::Pi(Box::new(failure))
    }
}

#[derive(Serialize)]
struct StageCacheParams<'a> {
    stage: StageKind,
    target: crate::stage::StageTarget,
    expected_commits: &'a [crate::git::CommitHash],
    request: &'a StageRequest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CachedReport<R> {
    report: R,
    iterations: u32,
    usage: LlmUsage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireOutcome<R> {
    Completed {
        report: R,
    },
    Clarification {
        questions: Vec<ClarificationQuestion>,
    },
}

pub async fn run<C>(
    runtime: &mut PiRuntime,
    cache: &CacheStore,
    stage: &C,
    config: StageRunConfig,
) -> Result<StageRun<C::Report>, StageRunError>
where
    C: ReviewStage,
{
    let request = stage.request();
    let params = StageCacheParams {
        stage: stage.kind(),
        target: stage.target(),
        expected_commits: stage.expected_commits(),
        request: &request,
    };
    let cache_key =
        CacheKey::from_params(format!("typed-stage-{}", stage.kind().as_str()), &params)?;
    if let Some(cached) = load_cache::<C>(cache, &cache_key, stage) {
        trace!(
            "typed stage cache hit: {} for {:?}",
            stage.kind().as_str(),
            stage.target()
        );
        return Ok(StageRun {
            stage: stage.kind(),
            target: stage.target(),
            ordered_commits: stage.expected_commits().to_vec(),
            outcome: StageOutcome::Completed {
                report: cached.report,
            },
            iterations: cached.iterations,
            usage: cached.usage,
        });
    }
    let session_key = CacheKey::from_params(
        format!("pi-session-typed-stage-{}", stage.kind().as_str()),
        &params,
    )?;
    trace!(
        "typed stage started: {} for {:?}",
        stage.kind().as_str(),
        stage.target()
    );
    let result = runtime
        .run(PiRunRequest {
            session_key,
            config: RunConfig {
                tool_contract_digest: tool_contract_digest(),
                operation: Operation::Stage {
                    stage: stage.kind().into(),
                    target: stage.target().to_string(),
                    expected_commits: stage.expected_commits().to_vec(),
                },
                system_prompt: request.system_prompt,
                read_tools: request.read_tools,
                terminal_tools: vec![stage.kind().into(), TerminalTool::RequestClarification],
                max_turns: config.max_iterations,
            },
            model: config.model.clone(),
            prompt: request.prompt,
            resume: config.resume,
        })
        .await;
    let result = match result {
        Ok(result) => result,
        Err(PiRunFailure {
            error: PiRunError::Exhausted { turns },
            usage: Some(usage),
        }) => {
            trace!(
                "typed stage exhausted: {} for {:?} turns={turns}",
                stage.kind().as_str(),
                stage.target()
            );
            return Ok(StageRun {
                stage: stage.kind(),
                target: stage.target(),
                ordered_commits: stage.expected_commits().to_vec(),
                outcome: StageOutcome::Exhausted {
                    reason: format!("Pi did not submit an outcome within {turns} turns"),
                },
                iterations: turns,
                usage: *usage,
            });
        }
        Err(error) => {
            debug!(
                "typed stage Pi run failed: {} for {:?}: {error:?}",
                stage.kind().as_str(),
                stage.target()
            );
            return Err(error.into());
        }
    };
    let outcome = match serde_json::from_value(result.outcome) {
        Ok(WireOutcome::Completed { report }) => {
            stage.validate_report(&report).map_err(|reason| {
                debug!(
                    "invalid typed stage report: {} for {:?}: {reason}",
                    stage.kind().as_str(),
                    stage.target()
                );
                StageRunError::InvalidReport {
                    reason,
                    usage: result.usage.clone(),
                }
            })?;
            update_cache(
                cache,
                &cache_key,
                &CachedReport {
                    report: &report,
                    iterations: result.iterations,
                    usage: result.usage.clone(),
                },
            );
            trace!(
                "typed stage completed: {} for {:?} iterations={}",
                stage.kind().as_str(),
                stage.target(),
                result.iterations
            );
            StageOutcome::Completed { report }
        }
        Ok(WireOutcome::Clarification { questions }) => {
            validate_questions(&questions).map_err(|reason| {
                debug!(
                    "invalid typed stage questions: {} for {:?}: {reason}",
                    stage.kind().as_str(),
                    stage.target()
                );
                StageRunError::InvalidQuestions {
                    reason,
                    usage: result.usage.clone(),
                }
            })?;
            trace!(
                "typed stage blocked: {} for {:?} questions={}",
                stage.kind().as_str(),
                stage.target(),
                questions.len()
            );
            StageOutcome::Blocked { questions }
        }
        Err(source) => {
            debug!(
                "invalid typed stage output: {} for {:?}: {source:?}",
                stage.kind().as_str(),
                stage.target()
            );
            return Err(StageRunError::InvalidOutput {
                source,
                usage: result.usage,
            });
        }
    };
    Ok(StageRun {
        stage: stage.kind(),
        target: stage.target(),
        ordered_commits: stage.expected_commits().to_vec(),
        outcome,
        iterations: result.iterations,
        usage: result.usage,
    })
}

fn load_cache<C>(cache: &CacheStore, key: &CacheKey, stage: &C) -> Option<CachedReport<C::Report>>
where
    C: ReviewStage,
{
    match cache.read_json::<CachedReport<C::Report>>(key) {
        Ok(Some(cached)) => match stage.validate_report(&cached.report) {
            Ok(()) => Some(cached),
            Err(error) => {
                debug!("ignoring invalid typed stage cache: {error:?}");
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            debug!("ignoring typed stage cache read error: {error:?}");
            None
        }
    }
}

fn update_cache<R>(cache: &CacheStore, key: &CacheKey, report: &CachedReport<R>)
where
    R: Serialize,
{
    if let Err(error) = cache.write_json(key, report) {
        debug!("ignoring typed stage cache write error: {error:?}");
    }
}

fn validate_questions(questions: &[ClarificationQuestion]) -> Result<(), String> {
    if questions.is_empty() {
        return Err("questions must not be empty".to_string());
    }
    if questions
        .iter()
        .any(|question| question.question.trim().is_empty() || question.reason.trim().is_empty())
    {
        return Err("questions and reasons must not be blank".to_string());
    }
    Ok(())
}

impl From<StageKind> for PiStageKind {
    fn from(kind: StageKind) -> Self {
        match kind {
            StageKind::ReviewContext => Self::ReviewContext,
            StageKind::Knowledge => Self::Knowledge,
            StageKind::Quality => Self::Quality,
            StageKind::Security => Self::Security,
        }
    }
}

impl From<StageKind> for TerminalTool {
    fn from(stage: StageKind) -> Self {
        match stage {
            StageKind::ReviewContext => Self::SubmitReviewContext,
            StageKind::Knowledge => Self::SubmitKnowledge,
            StageKind::Quality => Self::SubmitQuality,
            StageKind::Security => Self::SubmitSecurity,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::git::CommitHash;
    use crate::llm::LlmModelUsage;
    use crate::review::{PipelineReviewResult, PipelineStageResult, ReviewSummary};
    use crate::stage::{QualityReport, StageTarget};

    struct TestStage(CommitHash);

    impl ReviewStage for TestStage {
        type Report = QualityReport;

        fn kind(&self) -> StageKind {
            StageKind::Quality
        }

        fn target(&self) -> StageTarget {
            StageTarget::Commit(self.0.clone())
        }

        fn expected_commits(&self) -> &[CommitHash] {
            std::slice::from_ref(&self.0)
        }

        fn request(&self) -> StageRequest {
            StageRequest {
                system_prompt: "Review the change.".into(),
                prompt: "Check the target commit.".into(),
                read_tools: vec![],
            }
        }

        fn validate_report(&self, report: &Self::Report) -> Result<(), String> {
            if report.summary.is_empty() {
                Err("missing summary".into())
            } else {
                Ok(())
            }
        }
    }

    fn cache_key(stage: &TestStage) -> CacheKey {
        CacheKey::from_params(
            "typed-stage-quality",
            &StageCacheParams {
                stage: stage.kind(),
                target: stage.target(),
                expected_commits: stage.expected_commits(),
                request: &stage.request(),
            },
        )
        .unwrap()
    }

    fn model_usage(provider: &str, model: &str, units: u64) -> LlmModelUsage {
        LlmModelUsage {
            provider: provider.into(),
            model: model.into(),
            input_tokens: units * 10,
            output_tokens: units * 2,
            cache_read_tokens: units * 3,
            cache_write_tokens: units * 4,
            cost_usd: units as f64 * 0.125,
        }
    }

    fn original_usage() -> LlmUsage {
        LlmUsage::from(vec![
            model_usage("provider-a", "alpha", 1),
            model_usage("provider-b", "beta", 2),
        ])
    }

    fn cached_report(usage: LlmUsage) -> CachedReport<QualityReport> {
        CachedReport {
            report: QualityReport {
                summary: "Reviewed the change.".into(),
                findings: vec![],
            },
            iterations: 2,
            usage,
        }
    }

    async fn run_without_model(cache: &CacheStore, stage: &TestStage) -> StageRun<QualityReport> {
        let project_root = cache.version_root().join("nonexistent-project");
        assert!(!project_root.exists());
        let mut runtime = PiRuntime::new(project_root, cache.clone());
        run(
            &mut runtime,
            cache,
            stage,
            StageRunConfig {
                model: ModelRef::try_new("unavailable-test-provider", "unavailable-test-model")
                    .unwrap(),
                max_iterations: 1,
                resume: false,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn cache_hits_restore_original_usage_for_every_model() {
        let directory = tempfile::tempdir().unwrap();
        let cache = CacheStore::new(directory.path().join("cache"));
        let stage = TestStage(CommitHash::new("abc1234").unwrap());
        let key = cache_key(&stage);
        let usage = original_usage();
        let cached = cached_report(usage.clone());
        update_cache(&cache, &key, &cached);

        let result = run_without_model(&cache, &stage).await;

        assert_eq!(result.usage, usage);
        assert_eq!(result.iterations, cached.iterations);
        assert_eq!(
            result.outcome,
            StageOutcome::Completed {
                report: cached.report,
            }
        );
        let stored = cache.read_json::<serde_json::Value>(&key).unwrap().unwrap();
        assert_eq!(stored["usage"], serde_json::to_value(&usage).unwrap());
    }

    #[tokio::test]
    async fn final_json_combines_cached_and_new_stage_usage() {
        let directory = tempfile::tempdir().unwrap();
        let cache = CacheStore::new(directory.path().join("cache"));
        let stage = TestStage(CommitHash::new("abc1234").unwrap());
        let cached_usage = original_usage();
        update_cache(
            &cache,
            &cache_key(&stage),
            &cached_report(cached_usage.clone()),
        );
        let cached = run_without_model(&cache, &stage).await;
        let new_commit = CommitHash::new("def5678").unwrap();
        let new_usage = LlmUsage::from(vec![
            model_usage("provider-a", "alpha", 3),
            model_usage("provider-c", "gamma", 4),
        ]);
        let new = StageRun {
            stage: StageKind::Quality,
            target: StageTarget::Commit(new_commit.clone()),
            ordered_commits: vec![new_commit.clone()],
            outcome: StageOutcome::Completed {
                report: QualityReport {
                    summary: "Reviewed another change.".into(),
                    findings: vec![],
                },
            },
            iterations: 1,
            usage: new_usage.clone(),
        };
        let review = PipelineReviewResult {
            summary: ReviewSummary {
                peer_version: env!("CARGO_PKG_VERSION").into(),
            },
            ordered_commits: vec![stage.0.clone(), new_commit],
            stages: vec![
                PipelineStageResult::Quality(cached),
                PipelineStageResult::Quality(new),
            ],
            errors: vec![],
        };

        let json = crate::render::render_pipeline_json(review).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let expected = LlmUsage::from(vec![
            model_usage("provider-a", "alpha", 4),
            model_usage("provider-b", "beta", 2),
            model_usage("provider-c", "gamma", 4),
        ]);
        assert_eq!(value["usage"], serde_json::to_value(expected).unwrap());
        assert_eq!(
            value["stages"][0]["outcome"]["usage"],
            serde_json::to_value(cached_usage).unwrap()
        );
        assert_eq!(
            value["stages"][1]["outcome"]["usage"],
            serde_json::to_value(new_usage).unwrap()
        );
    }

    #[test]
    fn reports_usage_for_invalid_output_errors() {
        let usage = LlmUsage::zero("test-provider", "test-model");
        let error = StageRunError::InvalidOutput {
            source: serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err(),
            usage: usage.clone(),
        };

        assert_eq!(error.usage(), Some(&usage));
    }

    #[test]
    fn reports_usage_for_invalid_question_errors() {
        let usage = LlmUsage::zero("test-provider", "test-model");
        let error = StageRunError::InvalidQuestions {
            reason: "questions must not be empty".to_string(),
            usage: usage.clone(),
        };

        assert_eq!(error.usage(), Some(&usage));
    }

    #[test]
    fn reports_usage_for_invalid_report_errors() {
        let usage = LlmUsage::zero("test-provider", "test-model");
        let error = StageRunError::InvalidReport {
            reason: "missing summary".to_string(),
            usage: usage.clone(),
        };

        assert_eq!(error.usage(), Some(&usage));
    }

    #[test]
    fn omits_usage_for_cache_key_errors() {
        let error = StageRunError::CacheKey(CacheKeyError::from(
            serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err(),
        ));

        assert_eq!(error.usage(), None);
    }

    #[test]
    fn omits_usage_for_pi_errors_without_usage() {
        let error = StageRunError::from(PiRunFailure::from(PiRunError::InvalidState(
            "missing outcome".to_string(),
        )));

        assert_eq!(error.usage(), None);
    }

    #[test]
    fn reports_usage_for_pi_failures_with_usage() {
        let usage = LlmUsage::zero("test-provider", "test-model");
        let error = StageRunError::from(PiRunFailure {
            error: PiRunError::InvalidState("missing outcome".to_string()),
            usage: Some(Box::new(usage.clone())),
        });

        assert_eq!(error.usage(), Some(&usage));
    }

    #[test]
    fn accepts_structured_questions() {
        assert_eq!(
            validate_questions(&[ClarificationQuestion {
                question: "Which behavior is intended?".to_string(),
                reason: "The description and diff disagree.".to_string(),
            }]),
            Ok(())
        );
    }

    #[test]
    fn rejects_empty_questions() {
        assert_eq!(
            validate_questions(&[]),
            Err("questions must not be empty".to_string())
        );
    }

    #[test]
    fn rejects_blank_questions() {
        assert_eq!(
            validate_questions(&[ClarificationQuestion {
                question: " ".to_string(),
                reason: "The question is required.".to_string(),
            }]),
            Err("questions and reasons must not be blank".to_string())
        );
    }

    #[test]
    fn rejects_blank_question_reasons() {
        assert_eq!(
            validate_questions(&[ClarificationQuestion {
                question: "Question".to_string(),
                reason: " ".to_string(),
            }]),
            Err("questions and reasons must not be blank".to_string())
        );
    }

    #[test]
    fn converts_review_context_stage_kind_for_pi_protocol() {
        assert_eq!(
            PiStageKind::from(StageKind::ReviewContext),
            PiStageKind::ReviewContext,
        );
    }

    #[test]
    fn converts_knowledge_stage_kind_for_pi_protocol() {
        assert_eq!(
            PiStageKind::from(StageKind::Knowledge),
            PiStageKind::Knowledge
        );
    }

    #[test]
    fn converts_quality_stage_kind_for_pi_protocol() {
        assert_eq!(PiStageKind::from(StageKind::Quality), PiStageKind::Quality);
    }

    #[test]
    fn converts_security_stage_kind_for_pi_protocol() {
        assert_eq!(
            PiStageKind::from(StageKind::Security),
            PiStageKind::Security
        );
    }

    #[test]
    fn maps_review_context_stage_to_submit_tool() {
        assert_eq!(
            TerminalTool::from(StageKind::ReviewContext),
            TerminalTool::SubmitReviewContext,
        );
    }

    #[test]
    fn maps_knowledge_stage_to_submit_tool() {
        assert_eq!(
            TerminalTool::from(StageKind::Knowledge),
            TerminalTool::SubmitKnowledge,
        );
    }

    #[test]
    fn maps_quality_stage_to_submit_tool() {
        assert_eq!(
            TerminalTool::from(StageKind::Quality),
            TerminalTool::SubmitQuality,
        );
    }

    #[test]
    fn maps_security_stage_to_submit_tool() {
        assert_eq!(
            TerminalTool::from(StageKind::Security),
            TerminalTool::SubmitSecurity,
        );
    }
}
