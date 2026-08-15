use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cache::CacheStore;
use crate::config::Config;
use crate::console::Console;
use crate::context::ReviewContext;
use crate::extract::{ExtractError, Extractor};
use crate::git::CommitHash;
use crate::pi::{ModelRef, ModelRefError, PiRuntime};
use crate::stage::{
    CommitScopeReport, CommitScopeStage, CommitSequenceReport, CommitSequenceStage, IntentReport,
    IntentStage, QualityReport, QualityStage, ReviewContextReport, ReviewContextStage, ReviewStage,
    SecurityReport, SecurityStage, SizeReport, SizeStage, StageKind, StageOutcome, StageRun,
    StageRunConfig, StageRunError, StageTarget,
};

use super::{ReviewInput, ReviewSummary, ReviewTarget};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "stage", content = "result", rename_all = "snake_case")]
pub enum PipelineStageResult {
    ReviewContext(StageRun<ReviewContextReport>),
    CommitScope(StageRun<CommitScopeReport>),
    CommitSequence(StageRun<CommitSequenceReport>),
    Size(StageRun<SizeReport>),
    Intent(StageRun<IntentReport>),
    Quality(StageRun<QualityReport>),
    Security(StageRun<SecurityReport>),
}

impl PipelineStageResult {
    pub fn is_success(&self) -> bool {
        match self {
            Self::ReviewContext(run) => is_complete(run),
            Self::CommitScope(run) => is_complete(run),
            Self::CommitSequence(run) => is_complete(run),
            Self::Size(run) => is_complete(run),
            Self::Intent(run) => is_complete(run),
            Self::Quality(run) => is_complete(run),
            Self::Security(run) => is_complete(run),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PipelineExecutionError {
    pub stage: StageKind,
    pub target: StageTarget,
    pub reason: String,
}

impl fmt::Display for PipelineExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} stage for {} failed: {}",
            self.stage.as_str(),
            self.target,
            self.reason
        )
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PipelineReviewResult {
    pub summary: ReviewSummary,
    pub ordered_commits: Vec<CommitHash>,
    pub stages: Vec<PipelineStageResult>,
    pub errors: Vec<PipelineExecutionError>,
}

impl PipelineReviewResult {
    #[expect(dead_code)]
    pub fn is_success(&self) -> bool {
        self.errors.is_empty() && self.stages.iter().all(PipelineStageResult::is_success)
    }
}

#[derive(Debug)]
pub enum PipelineRunError {
    Extract(ExtractError),
    Model(ModelRefError),
}

impl fmt::Display for PipelineRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Extract(error) => write!(f, "failed to collect review input: {error}"),
            Self::Model(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PipelineRunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Extract(error) => Some(error),
            Self::Model(error) => Some(error),
        }
    }
}

impl From<ExtractError> for PipelineRunError {
    fn from(error: ExtractError) -> Self {
        Self::Extract(error)
    }
}

impl From<ModelRefError> for PipelineRunError {
    fn from(error: ModelRefError) -> Self {
        Self::Model(error)
    }
}

#[allow(clippy::too_many_arguments)]
#[expect(dead_code)]
pub async fn run_pipeline(
    target: &ReviewTarget,
    context: ReviewContext,
    console: Console,
    config: &Config,
    project_root: PathBuf,
    cache: &CacheStore,
    runtime: &mut PiRuntime,
    resume: bool,
) -> Result<PipelineReviewResult, PipelineRunError> {
    let input =
        ReviewInput::collect(target, context, &Extractor::new(project_root, console)).await?;
    let model = ModelRef::try_new(
        config.llm.default_provider.as_str(),
        config.llm.default_model.as_str(),
    )?;
    let mut result = PipelineReviewResult {
        summary: ReviewSummary {
            peer_version: env!("CARGO_PKG_VERSION").to_string(),
            provider: config.llm.default_provider.clone(),
            model: config.llm.default_model.clone(),
        },
        ordered_commits: input
            .commits
            .iter()
            .map(|commit| commit.hash.clone())
            .collect(),
        stages: Vec::new(),
        errors: Vec::new(),
    };

    let context_stage = ReviewContextStage::new(input.clone());
    let context_run = match execute(
        cache,
        runtime,
        config,
        &model,
        resume,
        console,
        &context_stage,
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            push_error(&mut result, &context_stage, error);
            return Ok(result);
        }
    };
    let Some(context_report) = cloned_completed_report(&context_run) else {
        result
            .stages
            .push(PipelineStageResult::ReviewContext(context_run));
        return Ok(result);
    };
    result
        .stages
        .push(PipelineStageResult::ReviewContext(context_run));

    let scope_stage = CommitScopeStage::new(input.clone(), context_report.clone());
    let scope_run = match execute(
        cache,
        runtime,
        config,
        &model,
        resume,
        console,
        &scope_stage,
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            push_error(&mut result, &scope_stage, error);
            return Ok(result);
        }
    };
    let Some(scope_report) = cloned_completed_report(&scope_run) else {
        result
            .stages
            .push(PipelineStageResult::CommitScope(scope_run));
        return Ok(result);
    };
    result
        .stages
        .push(PipelineStageResult::CommitScope(scope_run));

    let sequence_stage =
        CommitSequenceStage::new(input.clone(), context_report.clone(), scope_report.clone());
    let sequence_run = match execute(
        cache,
        runtime,
        config,
        &model,
        resume,
        console,
        &sequence_stage,
    )
    .await
    {
        Ok(run) => run,
        Err(error) => {
            push_error(&mut result, &sequence_stage, error);
            return Ok(result);
        }
    };
    let Some(sequence_report) = cloned_completed_report(&sequence_run) else {
        result
            .stages
            .push(PipelineStageResult::CommitSequence(sequence_run));
        return Ok(result);
    };
    result
        .stages
        .push(PipelineStageResult::CommitSequence(sequence_run));

    let review_commits = result.ordered_commits.clone();
    for commit in &input.commits {
        let stage = SizeStage::new(
            commit.clone(),
            review_commits.clone(),
            context_report.clone(),
            scope_report.clone(),
            sequence_report.clone(),
        );
        match execute(cache, runtime, config, &model, resume, console, &stage).await {
            Ok(run) => result.stages.push(PipelineStageResult::Size(run)),
            Err(error) => push_error(&mut result, &stage, error),
        }
    }
    for commit in &input.commits {
        let stage = IntentStage::new(
            commit.clone(),
            context_report.clone(),
            scope_report.clone(),
            sequence_report.clone(),
        );
        match execute(cache, runtime, config, &model, resume, console, &stage).await {
            Ok(run) => result.stages.push(PipelineStageResult::Intent(run)),
            Err(error) => push_error(&mut result, &stage, error),
        }
    }
    for commit in &input.commits {
        let stage = QualityStage::new(
            commit.clone(),
            input.head.clone(),
            context_report.clone(),
            scope_report.clone(),
            sequence_report.clone(),
        );
        match execute(cache, runtime, config, &model, resume, console, &stage).await {
            Ok(run) => result.stages.push(PipelineStageResult::Quality(run)),
            Err(error) => push_error(&mut result, &stage, error),
        }
    }
    for commit in &input.commits {
        let stage = SecurityStage::new(
            commit.clone(),
            input.head.clone(),
            context_report.clone(),
            scope_report.clone(),
            sequence_report.clone(),
        );
        match execute(cache, runtime, config, &model, resume, console, &stage).await {
            Ok(run) => result.stages.push(PipelineStageResult::Security(run)),
            Err(error) => push_error(&mut result, &stage, error),
        }
    }

    Ok(result)
}

async fn execute<C>(
    cache: &CacheStore,
    runtime: &mut PiRuntime,
    config: &Config,
    model: &ModelRef,
    resume: bool,
    console: Console,
    stage: &C,
) -> Result<StageRun<C::Report>, StageRunError>
where
    C: ReviewStage,
{
    crate::stage::run(
        runtime,
        cache,
        stage,
        StageRunConfig {
            model: model.clone(),
            max_iterations: config.max_iterations_for(stage.kind().as_str()).get(),
            resume,
            console,
        },
    )
    .await
}

fn cloned_completed_report<R>(run: &StageRun<R>) -> Option<R>
where
    R: Clone,
{
    match &run.outcome {
        StageOutcome::Completed { report } => Some(report.clone()),
        StageOutcome::Blocked { .. } => None,
        StageOutcome::Exhausted { .. } => None,
    }
}

fn is_complete<R>(run: &StageRun<R>) -> bool {
    matches!(run.outcome, StageOutcome::Completed { .. })
}

fn push_error<C>(result: &mut PipelineReviewResult, stage: &C, error: StageRunError)
where
    C: ReviewStage,
{
    result.errors.push(PipelineExecutionError {
        stage: stage.kind(),
        target: stage.target(),
        reason: error.to_string(),
    });
}
