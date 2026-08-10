use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::console::Console;
use crate::git::CommitHash;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckError {
    Exhausted { reason: String },
    ClarificationRequired { questions: Vec<String> },
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted { reason } => f.write_str(reason),
            Self::ClarificationRequired { .. } => f.write_str("clarification required"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileLocation {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Finding {
    pub commit: CommitHash,
    pub severity: Severity,
    pub message: String,
    #[serde(flatten)]
    pub location: Option<FileLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CheckTarget {
    Commit(CommitHash),
    Range { from: CommitHash, to: CommitHash },
}

impl fmt::Display for CheckTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Commit(commit) => commit.fmt(f),
            Self::Range { from, to } => write!(f, "{from}..{to}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LlmModelUsage {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub model: String,
    #[serde(default)]
    pub models: Vec<LlmModelUsage>,
}

impl LlmUsage {
    pub fn from_pi_models(models: Vec<LlmModelUsage>) -> Self {
        let input_tokens = models.iter().map(|usage| usage.input_tokens).sum();
        let output_tokens = models.iter().map(|usage| usage.output_tokens).sum();
        let cache_read_tokens = models.iter().map(|usage| usage.cache_read_tokens).sum();
        let cache_write_tokens = models.iter().map(|usage| usage.cache_write_tokens).sum();
        let cost_usd = models.iter().map(|usage| usage.cost_usd).sum();
        let model = match models.as_slice() {
            [usage] => format!("{}/{}", usage.provider, usage.model),
            [] => "unknown".to_string(),
            _ => "multiple".to_string(),
        };
        Self {
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost_usd,
            model,
            models,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CheckResult {
    pub check: String,
    pub target: CheckTarget,
    /// Target commits in review order, from oldest to newest.
    pub ordered_commits: Vec<CommitHash>,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub iterations: u32,
    pub error: Option<CheckError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<LlmUsage>,
    pub usage: LlmUsage,
}

impl CheckResult {
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

#[cfg_attr(not(test), expect(dead_code))]
async fn validate_per_commit_targets(
    findings: &[Finding],
    target: &CommitHash,
    dir: &Path,
    console: Console,
) -> Result<(), String> {
    for finding in findings {
        let commit = CommitHash::resolve(finding.commit.as_ref(), dir, console)
            .await
            .map_err(|err| {
                console.debug(format_args!(
                    "cannot find commit {}: {err:?}",
                    finding.commit
                ));
                format!(
                    "finding commit {} does not match target {target}",
                    finding.commit
                )
            })?;
        if &commit != target {
            return Err(format!(
                "finding commit {} does not match target {target}",
                finding.commit
            ));
        }
    }
    Ok(())
}

#[cfg_attr(not(test), expect(dead_code))]
async fn validate_range_targets(
    findings: &[Finding],
    commits: &[CommitHash],
    dir: &Path,
    console: Console,
) -> Result<(), String> {
    for finding in findings {
        // TODO: Resolve finding commits concurrently if range validation
        // becomes a performance bottleneck.
        let commit = CommitHash::resolve(finding.commit.as_ref(), dir, console)
            .await
            .map_err(|err| {
                console.debug(format_args!(
                    "cannot find commit {}: {err:?}",
                    finding.commit
                ));
                format!("finding commit {} is not in range commits", finding.commit)
            })?;
        if !commits.contains(&commit) {
            return Err(format!(
                "finding commit {} is not in range commits",
                finding.commit
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::assert_matches;

    fn finding(commit: &str, severity: Severity) -> Finding {
        Finding {
            commit: CommitHash::new(commit).unwrap(),
            severity,
            message: "test finding".to_string(),
            location: None,
        }
    }

    async fn create_repo_with_commit() -> (tempfile::TempDir, CommitHash) {
        let dir = tempfile::tempdir().unwrap();
        let console = Console::default();

        crate::git::run_git(&["init"], dir.path(), console)
            .await
            .unwrap();
        crate::git::run_git(
            &["config", "user.email", "test@example.com"],
            dir.path(),
            console,
        )
        .await
        .unwrap();
        crate::git::run_git(&["config", "user.name", "Test User"], dir.path(), console)
            .await
            .unwrap();
        crate::git::run_git(
            &["commit", "--allow-empty", "-m", "initial commit"],
            dir.path(),
            console,
        )
        .await
        .unwrap();

        let commit = CommitHash::resolve("HEAD", dir.path(), console)
            .await
            .unwrap();
        (dir, commit)
    }

    #[tokio::test]
    async fn per_commit_findings_resolve_abbreviated_hashes() {
        let (dir, target) = create_repo_with_commit().await;
        let abbreviated = target.as_ref()[..7].to_string();

        assert!(
            validate_per_commit_targets(
                &[finding(&abbreviated, Severity::Info)],
                &target,
                dir.path(),
                Console::default(),
            )
            .await
            .is_ok()
        );
        assert!(
            validate_per_commit_targets(
                &[finding("def5678", Severity::Info)],
                &target,
                dir.path(),
                Console::default(),
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn range_findings_resolve_abbreviated_hashes() {
        let (dir, target) = create_repo_with_commit().await;
        let abbreviated = target.as_ref()[..7].to_string();

        assert!(
            validate_range_targets(
                &[finding(&abbreviated, Severity::Low)],
                std::slice::from_ref(&target),
                dir.path(),
                Console::default(),
            )
            .await
            .is_ok()
        );
        assert!(
            validate_range_targets(
                &[finding("def5678", Severity::Low)],
                &[target],
                dir.path(),
                Console::default(),
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn finding_omits_location_when_absent() {
        let value = serde_json::to_value(finding("abc1234", Severity::Info)).unwrap();

        assert_eq!(value["severity"], "info");
        assert!(value.get("file").is_none());
        assert!(value.get("line").is_none());
    }

    #[test]
    fn finding_serializes_file_and_optional_line() {
        let mut with_line = finding("abc1234", Severity::High);
        with_line.location = Some(FileLocation {
            file: "src/main.rs".to_string(),
            line: Some(42),
        });
        let value = serde_json::to_value(with_line).unwrap();

        assert_eq!(value["file"], "src/main.rs");
        assert_eq!(value["line"], 42);
    }

    #[test]
    fn finding_deserializes_without_location() {
        let finding: Finding = serde_json::from_value(serde_json::json!({
            "commit": "abc1234",
            "severity": "info",
            "message": "test finding"
        }))
        .unwrap();

        assert_eq!(finding.location, None);
    }

    #[test]
    fn finding_deserializes_file_without_line() {
        let finding: Finding = serde_json::from_value(serde_json::json!({
            "commit": "abc1234",
            "severity": "info",
            "message": "test finding",
            "file": "src/main.rs"
        }))
        .unwrap();

        assert_eq!(
            finding.location,
            Some(FileLocation {
                file: "src/main.rs".to_string(),
                line: None,
            })
        );
    }

    #[test]
    fn finding_deserializes_line_without_file_as_no_location() {
        let finding: Finding = serde_json::from_value(serde_json::json!({
            "commit": "abc1234",
            "severity": "info",
            "message": "test finding",
            "line": 42
        }))
        .unwrap();

        assert_eq!(finding.location, None);
    }

    #[test]
    fn check_target_serializes_resolved_range_endpoints() {
        let commit = CheckTarget::Commit(CommitHash::new("abc1234").unwrap());
        let range = CheckTarget::Range {
            from: CommitHash::new("abc1234").unwrap(),
            to: CommitHash::new("def5678").unwrap(),
        };

        assert_eq!(serde_json::to_value(commit).unwrap(), "abc1234");
        assert_eq!(
            serde_json::to_value(range).unwrap(),
            serde_json::json!({"from": "abc1234", "to": "def5678"})
        );
    }

    #[test]
    fn check_target_deserializes_commit_and_range() {
        let commit: CheckTarget = serde_json::from_str("\"abc1234\"").unwrap();
        let range: CheckTarget =
            serde_json::from_str(r#"{"from":"abc1234","to":"def5678"}"#).unwrap();

        assert_matches!(commit, CheckTarget::Commit(_));
        assert_eq!(
            range,
            CheckTarget::Range {
                from: CommitHash::new("abc1234").unwrap(),
                to: CommitHash::new("def5678").unwrap(),
            }
        );
    }

    #[test]
    fn check_target_displays_its_revision() {
        let commit = CheckTarget::Commit(CommitHash::new("abc1234").unwrap());
        let range = CheckTarget::Range {
            from: CommitHash::new("abc1234").unwrap(),
            to: CommitHash::new("def5678").unwrap(),
        };

        assert_eq!(commit.to_string(), "abc1234");
        assert_eq!(range.to_string(), "abc1234..def5678");
    }

    #[test]
    fn pi_usage_preserves_cache_tokens_and_model_costs() {
        let usage = LlmUsage::from_pi_models(vec![LlmModelUsage {
            provider: "mistral".to_string(),
            model: "mistral-medium-3-5".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 80,
            cache_write_tokens: 10,
            cost_usd: 0.012,
        }]);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 80);
        assert_eq!(usage.cache_write_tokens, 10);
        assert_eq!(usage.cost_usd, 0.012);
        assert_eq!(usage.model, "mistral/mistral-medium-3-5");
    }

    #[test]
    fn check_result_allows_missing_context_usage() {
        let mut result: CheckResult = serde_json::from_value(serde_json::json!({
            "check": "security",
            "target": "abc1234",
            "ordered_commits": ["abc1234"],
            "summary": "Checked.",
            "findings": [],
            "iterations": 1,
            "error": null,
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cost_usd": 0.001,
                "model": "test-model"
            }
        }))
        .unwrap();

        assert_eq!(result.context_usage, None);
        assert!(result.is_success());
        result.error = Some(CheckError::Exhausted {
            reason: "iteration limit reached".to_string(),
        });
        assert!(!result.is_success());
        assert!(
            serde_json::to_value(result)
                .unwrap()
                .get("context_usage")
                .is_none()
        );
    }
}
