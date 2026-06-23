use serde::{Deserialize, Serialize};

use crate::git::CommitHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct FileLocation {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct Finding {
    pub commit: CommitHash,
    pub severity: Severity,
    pub message: String,
    #[serde(flatten)]
    pub location: Option<FileLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
#[allow(dead_code)]
pub enum CheckTarget {
    Commit(CommitHash),
    Range(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CheckUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct CheckOutput {
    pub summary: String,
    pub findings: Vec<Finding>,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[allow(dead_code)]
pub struct CheckResult {
    pub check: String,
    pub target: CheckTarget,
    pub summary: String,
    pub findings: Vec<Finding>,
    pub confidence: f64,
    pub iterations: u32,
    pub is_exhausted: bool,
    pub exhaustion_reason: Option<String>,
    pub usage: CheckUsage,
}

#[allow(dead_code)]
pub fn validate_confidence(confidence: f64) -> Result<(), String> {
    if (0.0..=1.0).contains(&confidence) {
        Ok(())
    } else {
        Err(format!(
            "confidence {confidence} is outside the range [0.0, 1.0]"
        ))
    }
}

#[allow(dead_code)]
pub fn validate_per_commit_targets(
    findings: &[Finding],
    target: &CommitHash,
) -> Result<(), String> {
    for finding in findings {
        if &finding.commit != target {
            return Err(format!(
                "finding commit {} does not match target {target}",
                finding.commit
            ));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn validate_range_targets(findings: &[Finding], commits: &[CommitHash]) -> Result<(), String> {
    for finding in findings {
        if !commits.contains(&finding.commit) {
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

    fn finding(commit: &str, severity: Severity) -> Finding {
        Finding {
            commit: CommitHash::new(commit).unwrap(),
            severity,
            message: "test finding".to_string(),
            location: None,
        }
    }

    #[test]
    fn confidence_must_be_between_zero_and_one() {
        assert!(validate_confidence(0.0).is_ok());
        assert!(validate_confidence(0.5).is_ok());
        assert!(validate_confidence(1.0).is_ok());
        assert!(validate_confidence(-0.1).is_err());
        assert!(validate_confidence(1.1).is_err());
        assert!(validate_confidence(f64::NAN).is_err());
    }

    #[test]
    fn per_commit_findings_must_match_target() {
        let target = CommitHash::new("abc1234").unwrap();

        assert!(
            validate_per_commit_targets(&[finding("abc1234", Severity::Info)], &target).is_ok()
        );
        assert!(
            validate_per_commit_targets(&[finding("def5678", Severity::Info)], &target).is_err()
        );
    }

    #[test]
    fn range_findings_must_reference_a_commit_in_the_range() {
        let commits = [
            CommitHash::new("abc1234").unwrap(),
            CommitHash::new("def5678").unwrap(),
        ];
        assert!(validate_range_targets(&[finding("def5678", Severity::Low)], &commits).is_ok());
        assert!(validate_range_targets(&[finding("9876abc", Severity::Low)], &commits).is_err());
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

        assert!(matches!(commit, CheckTarget::Commit(_)));
        assert_eq!(range, CheckTarget::Range("HEAD~3..HEAD".to_string()));
    }
}
