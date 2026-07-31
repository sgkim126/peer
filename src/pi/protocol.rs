use serde::{Deserialize, Serialize};

use crate::git::CommitHash;

const TOOL_CONTRACT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/pi/tool-contract-v1.json"
));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Size,
    Intent,
    Quality,
    Security,
    Coherence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadTool {
    GetCommitMessage,
    GetCommitDiff,
    GetChangedFiles,
    GetCommitsInRange,
    GetFileContent,
    GetFileDiff,
    ListTree,
    Grep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalTool {
    SubmitCheckResult,
    RequestClarification,
    SubmitReviewContextDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReviewContext,
    Check {
        check: CheckKind,
        target: String,
        expected_commits: Vec<CommitHash>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(test), expect(dead_code))]
pub struct RunConfig {
    pub tool_contract_digest: String,
    pub operation: Operation,
    pub system_prompt: String,
    pub read_tools: Vec<ReadTool>,
    pub terminal_tools: Vec<TerminalTool>,
    pub max_turns: u32,
}

#[cfg_attr(not(test), expect(dead_code))]
pub fn tool_contract_digest() -> String {
    blake3::hash(TOOL_CONTRACT.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_config_identifies_its_tool_contract() {
        let config = RunConfig {
            tool_contract_digest: tool_contract_digest(),
            operation: Operation::Check {
                check: CheckKind::Quality,
                target: "abc1234".to_string(),
                expected_commits: vec![CommitHash::new("abc1234").unwrap()],
            },
            system_prompt: "Review code.".to_string(),
            read_tools: vec![ReadTool::GetCommitDiff],
            terminal_tools: vec![TerminalTool::SubmitCheckResult],
            max_turns: 4,
        };

        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["operation"]["type"], "check");
        assert_eq!(value["operation"]["check"], "quality");
        assert_eq!(value["read_tools"], serde_json::json!(["get_commit_diff"]));
        assert_eq!(
            value["terminal_tools"],
            serde_json::json!(["submit_check_result"])
        );
        assert_eq!(value["tool_contract_digest"], tool_contract_digest());
    }

    #[test]
    fn tool_types_match_the_tool_contract() {
        let contract: serde_json::Value = serde_json::from_str(TOOL_CONTRACT).unwrap();
        let read_tools = [
            ReadTool::GetCommitMessage,
            ReadTool::GetCommitDiff,
            ReadTool::GetChangedFiles,
            ReadTool::GetCommitsInRange,
            ReadTool::GetFileContent,
            ReadTool::GetFileDiff,
            ReadTool::ListTree,
            ReadTool::Grep,
        ];
        let terminal_tools = [
            TerminalTool::SubmitCheckResult,
            TerminalTool::RequestClarification,
            TerminalTool::SubmitReviewContextDigest,
        ];

        assert_eq!(
            serde_json::to_value(read_tools).unwrap(),
            contract["read_tools"]
        );
        assert_eq!(
            serde_json::to_value(terminal_tools).unwrap(),
            contract["terminal_tools"]
        );
    }

    #[test]
    fn rejects_an_unknown_check_kind() {
        let error = serde_json::from_value::<Operation>(serde_json::json!({
            "type": "check",
            "check": "unknown",
            "target": "abc1234",
            "expected_commits": ["abc1234"],
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown variant"));
    }

    #[test]
    fn rejects_an_unknown_check_field() {
        let error = serde_json::from_value::<Operation>(serde_json::json!({
            "type": "check",
            "check": "quality",
            "target": "abc1234",
            "expected_commits": ["abc1234"],
            "extra_field": "value",
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `extra_field`"));
    }
}
