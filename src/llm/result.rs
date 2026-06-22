use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::console::Console;
use crate::git::CommitHash;
use crate::llm::provider::RawUsage;

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
    Range(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CheckUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CheckOutput {
    #[serde(default)]
    pub summary: String,
    pub findings: Vec<Finding>,
}

impl CheckUsage {
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn from_raw_usage(
        usage: RawUsage,
        model: impl Into<String>,
        input_per_1m_usd: f64,
        output_per_1m_usd: f64,
    ) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: cost_usd(usage, input_per_1m_usd, output_per_1m_usd),
            model: model.into(),
        }
    }
}

fn cost_usd(usage: RawUsage, input_per_1m_usd: f64, output_per_1m_usd: f64) -> f64 {
    (usage.input_tokens as f64 / 1_000_000.0) * input_per_1m_usd
        + (usage.output_tokens as f64 / 1_000_000.0) * output_per_1m_usd
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[expect(dead_code)]
pub struct CheckResult {
    pub check: String,
    pub target: CheckTarget,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub iterations: u32,
    pub is_exhausted: bool,
    pub exhaustion_reason: Option<String>,
    pub usage: CheckUsage,
}

#[cfg_attr(not(test), expect(dead_code))]
pub async fn validate_per_commit_targets(
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
pub async fn validate_range_targets(
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

    const MILLION: f64 = 1_000_000.0;

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
    fn check_target_serializes_as_a_string() {
        let commit = CheckTarget::Commit(CommitHash::new("abc1234").unwrap());
        let range = CheckTarget::Range("HEAD~3..HEAD".to_string());

        assert_eq!(serde_json::to_value(commit).unwrap(), "abc1234");
        assert_eq!(serde_json::to_value(range).unwrap(), "HEAD~3..HEAD");
    }

    #[test]
    fn check_target_deserializes_commit_and_range() {
        let commit: CheckTarget = serde_json::from_str("\"abc1234\"").unwrap();
        let range: CheckTarget = serde_json::from_str("\"HEAD~3..HEAD\"").unwrap();

        assert_matches!(commit, CheckTarget::Commit(_));
        assert_eq!(range, CheckTarget::Range("HEAD~3..HEAD".to_string()));
    }

    #[test]
    fn check_output_allows_a_missing_summary() {
        let output: CheckOutput = serde_json::from_value(serde_json::json!({
            "findings": []
        }))
        .unwrap();

        assert!(output.summary.is_empty());
        assert!(output.findings.is_empty());
    }

    #[test]
    fn check_usage_from_raw_usage_calculates_cost() {
        let input_tokens = 1_000;
        let output_tokens = 500;
        let model = "mistral-large-latest";

        let usage = RawUsage {
            input_tokens,
            output_tokens,
        };
        let check = CheckUsage::from_raw_usage(usage, model, 2.0, 6.0);

        assert_eq!(check.input_tokens, input_tokens);
        assert_eq!(check.output_tokens, output_tokens);
        assert_eq!(check.model, model);

        let expected_cost =
            (input_tokens as f64 / MILLION) * 2.0 + (output_tokens as f64 / MILLION) * 6.0;
        assert!((check.cost_usd - expected_cost).abs() < f64::EPSILON);
    }
}
